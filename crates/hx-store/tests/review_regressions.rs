//! Review regressions use only disposable SQLite state and synthetic records.
use anyhow::Result;
use chrono::{Duration, Utc};
use hx_store::Store;
use serde_json::json;
use sqlx::Connection;

async fn event(store: &Store, key: &str, at: i64, still: bool) -> Result<i64> {
    let mut tx = store.begin_write().await?;
    let id = tx.insert_evidence(sqlx::query_scalar("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES((SELECT next_id FROM evidence_seq),$1,'test:',$1,'1',$2,$3,$3,'normal','{}',$3) RETURNING id").bind(key).bind(if still {"media.still"} else {"text.snapshot"}).bind(at)).await?.unwrap();
    tx.commit().await?;
    Ok(id)
}
#[tokio::test]
async fn punctuation_only_search_terms_are_ignored() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let id = event(&store, "report", 1, false).await?;
    sqlx::query("INSERT INTO normalized VALUES($1,'test','{}','quarterly report final orchid')")
        .bind(id)
        .execute(&store.pool)
        .await?;
    for q in [
        "quarterly - report",
        "report & final",
        "report -- final",
        "- report",
        "orchid \" report",
    ] {
        assert_eq!(
            store.search(q, false, &["test:".into()]).await?.len(),
            1,
            "{q}"
        );
    }
    for q in ["-", "& --", "\""] {
        assert!(store.search(q, false, &["test:".into()]).await?.is_empty());
    }
    Ok(())
}
#[tokio::test]
async fn opening_foreign_core_does_not_change_its_journal() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("core.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options).await?;
    sqlx::query("CREATE TABLE foreign_history(id INTEGER)")
        .execute(&mut conn)
        .await?;
    conn.close().await?;
    assert!(Store::open(dir.path()).await.is_err());
    let mut conn = sqlx::SqliteConnection::connect_with(&options).await?;
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(mode, "delete", "refusing a foreign core must not modify it");
    Ok(())
}
#[tokio::test]
async fn creation_refuses_orphaned_blob_history() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("blobs"))?;
    std::fs::write(dir.path().join("blobs/synthetic"), b"synthetic history")?;
    let result = Store::create(dir.path()).await;
    assert!(
        result.is_err(),
        "must refuse non-empty blobs without a core"
    );
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("non-empty blobs")
    );
    assert!(!dir.path().join("core.sqlite").exists());
    Ok(())
}
#[tokio::test]
async fn late_still_and_late_blob_survive_discovery_watermark() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    event(&store, "newer-time", 1000, true).await?;
    store.discover_ocr("test/v1").await?;
    let id = event(&store, "late-time", 1, true).await?;
    let sha = hx_store::hash(b"synthetic late image");
    sqlx::query("INSERT INTO blobs(sha256,created_at) VALUES($1,1)")
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    sqlx::query("INSERT INTO event_blobs VALUES($1,$2)")
        .bind(id)
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    store.discover_ocr("test/v1").await?;
    store.pool.close().await;
    let store = Store::open(dir.path()).await?;
    store.upload_blob(&sha, b"synthetic late image").await?;
    store.discover_ocr("test/v1").await?;
    let claim = store
        .claim_ocr("test/v1")
        .await?
        .expect("late still must be discovered after reopening");
    assert_eq!(claim.candidate.source_event_id, id);
    claim
        .complete("test/v1", 0, None, Some(json!({"lines":[]})))
        .await?;
    store.publish_ocr_lineage("test/v1").await?;
    assert_eq!(store.ocr_evidence_for_sources(&[id]).await?.len(), 1);
    // Idle discovery must not wait for an unrelated writer, regardless of history.
    let gate = store.begin_write().await?;
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        store.discover_ocr("test/v1"),
    )
    .await;
    gate.rollback().await?;
    assert!(result.is_ok(), "caught-up discovery must be a plain read");
    result??;
    Ok(())
}
#[tokio::test]
async fn withdrawal_invalidates_consumers_of_older_revisions_without_buying() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let w1 = hx_store::window_at(Utc::now() - Duration::days(30));
    let w2 = hx_store::window_at(w1.end);
    let g1 = store.generation(w1).await?;
    let tasks = json!([{"id":"synthetic-task","open":true}]);
    let r1 = store
        .publish_revision(
            w1,
            g1,
            json!({"tasks":tasks,"input_manifest":{"prior_revision_ids":[]}}),
        )
        .await?
        .0;
    let g2 = store.generation(w2).await?;
    store
        .publish_revision(
            w2,
            g2,
            json!({"tasks":[],"input_manifest":{"prior_revision_ids":[r1]}}),
        )
        .await?;
    store
        .publish_revision(
            w1,
            g1,
            json!({"tasks":tasks,"input_manifest":{"prior_revision_ids":[]}}),
        )
        .await?;
    store
        .publish_revision(
            w1,
            g1,
            json!({"tasks":[],"input_manifest":{"prior_revision_ids":[]}}),
        )
        .await?;
    assert_eq!(
        store.generation(w2).await?,
        g2 + 1,
        "withdrawal must stale consumers of R1 even when canonical was R2"
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='interpret'")
        .fetch_one(&store.pool)
        .await?;
    assert_eq!(jobs, 0, "invalidation must never purchase a run");
    Ok(())
}
#[tokio::test]
async fn publication_invalidation_stays_within_sixteen_hours() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let w1 = hx_store::window_at(Utc::now() - Duration::days(30));
    let far = hx_store::window_at(w1.end + Duration::hours(16));
    let g = store.generation(w1).await?;
    let r1 = store
        .publish_revision(w1, g, json!({"tasks":[{"open":true}]}))
        .await?
        .0;
    let gf = store.generation(far).await?;
    store
        .publish_revision(
            far,
            gf,
            json!({"tasks":[],"input_manifest":{"prior_revision_ids":[r1]}}),
        )
        .await?;
    store.publish_revision(w1, g, json!({"tasks":[]})).await?;
    assert_eq!(
        store.generation(far).await?,
        gf,
        "publish must retain main's 16h invalidation bound"
    );
    Ok(())
}
#[tokio::test]
async fn empty_sweep_selection_does_not_take_writer() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let gate = store.begin_write().await?;
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        store.schedule_daily_sweep(Utc::now()),
    )
    .await;
    gate.rollback().await?;
    assert!(result.is_ok(), "empty sweep must select outside the writer");
    result??;
    Ok(())
}

#[tokio::test]
async fn resetting_failed_ocr_keeps_its_source_lineage() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let id = event(&store, "retry-still", 1, true).await?;
    let sha = hx_store::hash(b"synthetic retry image");
    store.upload_blob(&sha, b"synthetic retry image").await?;
    sqlx::query("INSERT INTO event_blobs VALUES($1,$2)")
        .bind(id)
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    store.discover_ocr("test/v1").await?;
    sqlx::query("UPDATE ocr_jobs SET status='failed'")
        .execute(&store.pool)
        .await?;
    store.publish_ocr_lineage("test/v1").await?;
    sqlx::query("UPDATE ocr_jobs SET status='pending',attempts=0")
        .execute(&store.pool)
        .await?;
    let claim = store.claim_ocr("test/v1").await?.unwrap();
    claim
        .complete("test/v1", 0, None, Some(json!({"lines":[]})))
        .await?;
    store.publish_ocr_lineage("test/v1").await?;
    assert_eq!(
        store.ocr_evidence_for_sources(&[id]).await?.len(),
        1,
        "operator reset must preserve lineage"
    );
    Ok(())
}

#[tokio::test]
async fn revision_containment_matches_jsonb_types_and_numbers() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let w = hx_store::window_at(Utc::now());
    let g = store.generation(w).await?;
    store.publish_revision(w,g,json!({"input_manifest":{"model":["test"],"effort":1.0,"disclosure_policy":{"digest":"d","extra":true}}})).await?;
    assert!(
        !store
            .revision_exists(w.start, g, &json!({"model":"test"}))
            .await?,
        "an object value must retain its JSON type"
    );
    assert!(
        store
            .revision_exists(
                w.start,
                g,
                &json!({"model":["test","test"],"effort":1,"disclosure_policy":{"digest":"d"}})
            )
            .await?
    );
    assert!(
        !store
            .revision_exists(w.start, g, &json!({"absent":null}))
            .await?
    );
    assert!(
        !store
            .revision_exists(w.start, g, &json!({"effort":"1"}))
            .await?
    );
    Ok(())
}
