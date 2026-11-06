//! One end-to-end proof: an online backup restores the same served citations.
use anyhow::{Context, Result, ensure};
use axum::{body::Body, http::Request};
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use hx_schema::{
    BlobRef, ConnectorInstance, Envelope, Kind, ObservationId, ObservationTime, Operation,
    Revision, Sensitivity, SourceKey,
};
use hx_server::disclosure::DisclosurePolicy;
use hx_store::{Store, Token, UploadBatch};
use serde_json::{Value, json};
use sqlx::Connection;
use std::{path::Path, time::Duration};
use tokio::process::Command;
use tower::ServiceExt;

fn event(n: u32, sha: &str) -> Envelope {
    let at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::seconds(i64::from(n));
    Envelope {
        envelope_version: 1,
        observation_id: ObservationId::generate(),
        connector_instance: ConnectorInstance::new("test:backup"),
        source_key: SourceKey::new(format!("record-{n}")),
        revision: Revision::append_only(),
        operation: Operation::Observe,
        sequence: Some(u64::from(n)),
        kind: Kind::new("web.visit"),
        schema_version: 1,
        time: ObservationTime::point(at),
        device: None,
        actor: None,
        subject: None,
        resource: None,
        links: vec![],
        coverage: None,
        sensitivity: if n == 1 {
            Sensitivity::Restricted
        } else {
            Sensitivity::Normal
        },
        payload: json!({"url":"https://example.invalid","title":format!("synthetic nebula {n}")}),
        blobs: vec![BlobRef {
            sha256: sha.into(),
            media_type: "text/plain".into(),
            bytes: 21,
        }],
    }
}
async fn ingest(store: &Store, n: u32, sha: &str) -> Result<()> {
    let ack = store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![event(n, sha)],
            },
            &Token {
                prefixes: vec!["test:".into()],
                scopes: vec!["ingest".into()],
            },
        )
        .await?;
    ensure!(ack.accepted == 1 && ack.rejected.is_empty(), "{ack:?}");
    Ok(())
}
async fn cli(state: &Path, action: &str, flag: &str, path: &Path) -> Result<std::process::Output> {
    Ok(tokio::time::timeout(
        Duration::from_secs(15),
        Command::new(env!("CARGO_BIN_EXE_hx-server"))
            .arg("--state-dir")
            .arg(state)
            .arg(action)
            .arg(flag)
            .arg(path)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("backup/restore waited for the live writer")??)
}
async fn served(store: &Store, secret: &str, path: &str) -> Result<Value> {
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {secret}"))
                .body(Body::empty())?,
        )
        .await?;
    ensure!(
        response.status().is_success(),
        "{}: {}",
        path,
        response.status()
    );
    Ok(serde_json::from_slice(
        &response.into_body().collect().await?.to_bytes(),
    )?)
}

#[tokio::test]
async fn online_backup_restores_identical_citations_and_search() -> Result<()> {
    let scratch = tempfile::tempdir()?;
    let source = scratch.path().join("source");
    let backup = scratch.path().join("backup");
    let target = scratch.path().join("restored");
    let store = Store::create(&source).await?;
    let bytes = b"synthetic backup blob";
    let sha = hx_store::hash(bytes);
    store.upload_blob(&sha, bytes).await?;
    for n in 0..3 {
        ingest(&store, n, &sha).await?;
    }
    store
        .normalize_window(hx_store::window_at(event(0, &sha).time.observed_at))
        .await?;
    let secret = store
        .create_token(
            "synthetic",
            vec!["test:".into()],
            vec!["read_l3".into(), "read_raw".into()],
        )
        .await?;
    let window = hx_store::window_at(event(0, &sha).time.observed_at);
    let generation = store.generation(window).await?;
    store
        .publish_revision(
            window,
            generation,
            json!({"summary":"synthetic", "evidence_ids":[1,2,3]}),
        )
        .await?;
    // Advance the high-water mark past existing rows. MAX(id) alone would miss it.
    let mut tx = store.begin_write().await?;
    sqlx::query("UPDATE sqlite_sequence SET seq=500 WHERE name='revisions'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut agent = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(source.join("agent.sqlite")),
    )
    .await?;
    sqlx::query("CREATE TABLE pending(id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT)")
        .execute(&mut agent)
        .await?;
    sqlx::query("INSERT INTO pending VALUES(700,'synthetic outbox')")
        .execute(&mut agent)
        .await?;
    sqlx::query("DELETE FROM pending")
        .execute(&mut agent)
        .await?;
    sqlx::query("INSERT INTO pending(body) VALUES('synthetic pending')")
        .execute(&mut agent)
        .await?;
    sqlx::query("UPDATE sqlite_sequence SET seq=900 WHERE name='pending'")
        .execute(&mut agent)
        .await?;

    let expected_search = served(&store, &secret, "/v1/search?q=nebula").await?;
    let mut expected_evidence = Vec::new();
    for id in 1..=3 {
        expected_evidence.push(served(&store, &secret, &format!("/v1/evidence/{id}")).await?);
    }

    // A live write transaction and an in-flight ingest must not prevent VACUUM
    // INTO from taking its WAL read snapshot. A locking backup hits cli's timeout.
    let gate = store.begin_write().await?;
    let ingesting = {
        let store = store.clone();
        let sha = sha.clone();
        tokio::spawn(async move { ingest(&store, 3, &sha).await })
    };
    let result = cli(&source, "backup", "--to", &backup).await;
    gate.rollback().await?;
    ingesting.await??;
    let result = result?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(backup.join("manifest.json").is_file());
    let result = cli(&target, "restore", "--from", &backup).await?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let restored = Store::open(&target).await?;
    assert_eq!(
        served(&restored, &secret, "/v1/search?q=nebula").await?,
        expected_search
    );
    for (id, expected) in (1..=3).zip(expected_evidence) {
        assert_eq!(
            served(&restored, &secret, &format!("/v1/evidence/{id}")).await?,
            expected
        );
    }
    assert!(
        restored.evidence(4).await?.is_none(),
        "in-flight ingest was outside the snapshot"
    );
    assert_eq!(std::fs::read(restored.blob_path(&sha)?)?, bytes);
    let next = restored
        .publish_revision(window, generation, json!({"summary":"after restore"}))
        .await?
        .0;
    assert_eq!(
        next, 501,
        "VACUUM must preserve sqlite_sequence, including deleted IDs"
    );
    let mut restored_agent = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(target.join("agent.sqlite")),
    )
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT body FROM pending")
            .fetch_one(&mut restored_agent)
            .await?,
        "synthetic pending"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("INSERT INTO pending(body) VALUES('next') RETURNING id")
            .fetch_one(&mut restored_agent)
            .await?,
        901
    );
    assert!(
        !cli(&target, "restore", "--from", &backup)
            .await?
            .status
            .success(),
        "restore must refuse existing history"
    );
    assert!(
        !cli(&source, "backup", "--to", &target)
            .await?
            .status
            .success(),
        "backup must not overwrite another state directory"
    );
    assert!(restored.evidence(1).await?.is_some());

    // Corruption and incomplete backups fail without leaving a nonempty target.
    let retry = scratch.path().join("retry");
    std::fs::create_dir(&retry)?;
    let blob = backup.join("blobs").join(&sha[..2]).join(&sha);
    std::fs::write(&blob, b"damaged")?;
    assert!(
        !cli(&retry, "restore", "--from", &backup)
            .await?
            .status
            .success()
    );
    assert_eq!(std::fs::read_dir(&retry)?.count(), 0);
    std::fs::write(&blob, bytes)?;
    let manifest = std::fs::read(backup.join("manifest.json"))?;
    std::fs::remove_file(backup.join("manifest.json"))?;
    assert!(
        !cli(&retry, "restore", "--from", &backup)
            .await?
            .status
            .success()
    );
    assert_eq!(std::fs::read_dir(&retry)?.count(), 0);
    std::fs::write(backup.join("manifest.json"), manifest)?;
    assert!(
        cli(&retry, "restore", "--from", &backup)
            .await?
            .status
            .success()
    );
    // Even a matching file hash cannot certify a broken FTS index. An edited
    // manifest must also describe every published blob and stay within its root.
    let rejected = scratch.path().join("rejected");
    let marker = backup.join("manifest.json");
    let manifest = std::fs::read(&marker)?;
    let core_path = backup.join("core.sqlite");
    let core = std::fs::read(&core_path)?;
    let mut conn = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&core_path),
    )
    .await?;
    sqlx::query("INSERT INTO normalized_fts(normalized_fts) VALUES('delete-all')")
        .execute(&mut conn)
        .await?;
    conn.close().await?;
    let broken_core = std::fs::read(&core_path)?;
    let mut edited: Value = serde_json::from_slice(&manifest)?;
    edited["files"]["core.sqlite"] =
        json!({"bytes":broken_core.len(),"sha256":hx_store::hash(&broken_core)});
    std::fs::write(&marker, serde_json::to_vec(&edited)?)?;
    assert!(
        !cli(&rejected, "restore", "--from", &backup)
            .await?
            .status
            .success()
    );
    assert!(!rejected.exists());
    std::fs::write(&core_path, &core)?;
    let mut edited: Value = serde_json::from_slice(&manifest)?;
    edited["files"]
        .as_object_mut()
        .unwrap()
        .remove(&format!("blobs/{}/{sha}", &sha[..2]));
    std::fs::write(&marker, serde_json::to_vec(&edited)?)?;
    assert!(
        !cli(&rejected, "restore", "--from", &backup)
            .await?
            .status
            .success()
    );
    assert!(!rejected.exists());
    edited["files"]["../escape"] = json!({"bytes":0,"sha256":hx_store::hash(b"")});
    std::fs::write(&marker, serde_json::to_vec(&edited)?)?;
    assert!(
        !cli(&rejected, "restore", "--from", &backup)
            .await?
            .status
            .success()
    );
    assert!(!rejected.exists());
    std::fs::write(&marker, &manifest)?;
    assert!(
        !cli(&source, "backup", "--to", &backup)
            .await?
            .status
            .success()
    );
    assert_eq!(
        std::fs::read(&marker)?,
        manifest,
        "completed backups are immutable"
    );
    // Retry an interrupted backup using its blob cache. A cached blob must not
    // be copied again, even if the live source no longer has those bytes.
    let resumable = scratch.path().join("resumable");
    let cache = resumable.join("blobs").join(&sha[..2]);
    std::fs::create_dir_all(&cache)?;
    std::fs::copy(&blob, cache.join(&sha))?;
    let agent_path = source.join("agent.sqlite");
    std::fs::rename(&agent_path, source.join("agent.aside"))?;
    assert!(
        !cli(&source, "backup", "--to", &resumable)
            .await?
            .status
            .success()
    );
    assert!(!resumable.join("manifest.json").exists());
    std::fs::rename(source.join("agent.aside"), &agent_path)?;
    let live_blob = store.blob_path(&sha)?;
    std::fs::rename(&live_blob, source.join("blob.aside"))?;
    let result = cli(&source, "backup", "--to", &resumable).await?;
    std::fs::rename(source.join("blob.aside"), &live_blob)?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    // Also take a snapshot while ingest commits continuously. The first phase
    // proved non-blocking with a writer held; this phase proves a real stream.
    let streaming_backup = scratch.path().join("streaming-backup");
    let streaming_target = scratch.path().join("streaming-restored");
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (started, ready) = tokio::sync::oneshot::channel();
    let streaming = {
        let store = store.clone();
        let sha = sha.clone();
        tokio::spawn(async move {
            ingest(&store, 4, &sha).await?;
            let _ = started.send(());
            for n in 5..10_000 {
                if *stopped.borrow() {
                    break;
                }
                ingest(&store, n, &sha).await?;
                store.normalize_window(window).await?;
            }
            Ok::<_, anyhow::Error>(())
        })
    };
    ready.await?;
    let result = cli(&source, "backup", "--to", &streaming_backup).await;
    let _ = stop.send(true);
    streaming.await??;
    let result = result?;
    ensure!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        cli(&streaming_target, "restore", "--from", &streaming_backup)
            .await?
            .status
            .success()
    );
    let streaming_restored = Store::open(&streaming_target).await?;
    let search = served(&streaming_restored, &secret, "/v1/search?q=nebula").await?;
    // Each restored hit must have the same citation and search result in the
    // still-live source, even though it now includes later committed events.
    let hits = search["results"].as_array().context("search results")?;
    assert!(!hits.is_empty());
    for hit in hits {
        let id = hit["id"].as_i64().context("search evidence ID")?;
        let path = format!("/v1/evidence/{id}");
        assert_eq!(
            served(&streaming_restored, &secret, &path).await?,
            served(&store, &secret, &path).await?
        );
        let n = hit["text"]
            .as_str()
            .context("search text")?
            .split_whitespace()
            .nth(2)
            .context("synthetic event number")?;
        let path = format!("/v1/search?q=nebula%20{n}");
        assert_eq!(
            served(&streaming_restored, &secret, &path).await?,
            served(&store, &secret, &path).await?
        );
    }
    Ok(())
}
