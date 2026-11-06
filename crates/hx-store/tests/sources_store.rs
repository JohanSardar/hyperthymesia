//! Below-interface checks for the shared SQLite evidence space and owner switches.
use anyhow::Result;
use hx_store::{Store, Ts};
use serde_json::json;

#[tokio::test]
async fn live_ids_share_allocator_without_changing_window_work() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let mut tx = store.begin_write().await?;
    sqlx::query("INSERT INTO windows(start_at,end_at,generation) VALUES(0,14400000000,7)")
        .execute(&mut *tx)
        .await?;
    let event: i64 = tx.insert_evidence(sqlx::query_scalar("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES((SELECT next_id FROM evidence_seq),'event','synthetic:','event','r1','text.snapshot',0,0,'normal','{}',0) RETURNING id")).await?.unwrap();
    sqlx::query("INSERT INTO sources(id,connector_instance,name,description,connected_at) VALUES('synthetic','source:synthetic','Synthetic','Fake only',0)").execute(&mut *tx).await?;
    let insert = "INSERT INTO live_snapshots(id,run_token,connector_instance,revision,envelope,created_at) VALUES((SELECT next_id FROM evidence_seq),$1,'source:synthetic',$2,$3,$4) ON CONFLICT(run_token,connector_instance,revision) DO NOTHING RETURNING id";
    let envelope = json!({"connector_instance":"source:synthetic", "kind":"source.live", "payload":{"answer":"synthetic"}});
    let run = ulid::Ulid::generate().to_string();
    let revision = hx_store::hash(b"synthetic");
    let live = tx
        .insert_evidence(
            sqlx::query_scalar(insert)
                .bind(&run)
                .bind(&revision)
                .bind(&envelope)
                .bind(Ts(0)),
        )
        .await?
        .unwrap();
    assert_ne!(event, live);
    assert_eq!(
        tx.insert_evidence(
            sqlx::query_scalar(insert)
                .bind(&run)
                .bind(&revision)
                .bind(&envelope)
                .bind(Ts(0))
        )
        .await?,
        None
    );
    let other_run = tx
        .insert_evidence(
            sqlx::query_scalar(insert)
                .bind(ulid::Ulid::generate().to_string())
                .bind(&revision)
                .bind(&envelope)
                .bind(Ts(0)),
        )
        .await?
        .unwrap();
    assert_eq!(other_run, live + 1, "duplicate must not burn an ID");
    let sha = "a".repeat(64);
    sqlx::query("INSERT INTO blobs(sha256,created_at) VALUES($1,0)")
        .bind(&sha)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,available_at) VALUES($1,'synthetic', $2,0)").bind(&sha).bind(event).execute(&mut *tx).await?;
    let ocr = tx.insert_evidence(sqlx::query_scalar("INSERT INTO ocr_evidence(id,source_event_id,sha256,engine_version,observation_id) VALUES((SELECT next_id FROM evidence_seq),$1,$2,'synthetic','ocr') RETURNING id").bind(event).bind(&sha)).await?.unwrap();
    assert_eq!(ocr, other_run + 1);
    // Explicit IDs must be guarded in both directions, for all three tables.
    for id in [event, ocr] {
        assert!(sqlx::query("INSERT INTO live_snapshots(id,run_token,connector_instance,revision,envelope,created_at) VALUES($1,'collision','source:synthetic',$2,'{}',0)").bind(id).bind(&revision).execute(&mut *tx).await.is_err());
    }
    assert!(sqlx::query("INSERT INTO events SELECT $1,'collision',connector_instance,'collision',revision,kind,source_start,source_end,observed_at,device,sensitivity,envelope,received_at FROM events WHERE id=$2").bind(live).bind(event).execute(&mut *tx).await.is_err());
    assert!(sqlx::query("INSERT INTO ocr_evidence(id,source_event_id,sha256,engine_version,observation_id) VALUES($1,$2,$3,'synthetic','collision')").bind(live).bind(event).bind(&sha).execute(&mut *tx).await.is_err());
    for statement in [
        "UPDATE live_snapshots SET envelope='{}' WHERE id=$1",
        "DELETE FROM live_snapshots WHERE id=$1",
        "INSERT OR REPLACE INTO live_snapshots SELECT * FROM live_snapshots WHERE id=$1",
    ] {
        assert!(
            sqlx::query(statement)
                .bind(live)
                .execute(&mut *tx)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("UPDATE ocr_evidence SET id=$1 WHERE id=$2")
            .bind(live)
            .bind(ocr)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.commit().await?;
    let token = ulid::Ulid::generate();
    let payload = json!({"answer":"Fresh synthetic context"});
    let fresh = store
        .save_live_snapshot(token, "synthetic", payload.clone())
        .await?;
    assert_eq!(
        store
            .save_live_snapshot(token, "synthetic", payload.clone())
            .await?,
        fresh
    );
    let live_envelope: hx_schema::Envelope =
        serde_json::from_value(store.evidence(fresh).await?.unwrap()["envelope"].clone())?;
    assert_eq!(
        live_envelope.revision,
        hx_schema::Revision::digest(&serde_json::to_vec(&payload)?)
    );
    assert_eq!(live_envelope.time.source_start, None);
    assert_eq!(
        live_envelope.sensitivity,
        hx_schema::Sensitivity::Restricted
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT generation FROM windows")
            .fetch_one(&store.pool)
            .await?,
        7
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs")
            .fetch_one(&store.pool)
            .await?,
        0
    );
    let resolved = store.evidence(live).await?.unwrap();
    assert_eq!(resolved["envelope"], envelope);
    assert_eq!(resolved["sensitivity"], "restricted");
    assert_eq!(store.source_envelope(live).await?, Some(envelope));
    assert_eq!(
        store.source_namespace(live).await?.as_deref(),
        Some("source:synthetic")
    );
    assert_eq!(store.evidence(other_run).await?.unwrap()["id"], other_run);
    assert!(store.evidence(ocr).await?.is_some());
    assert!(store.evidence(9999).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn resaving_under_new_name_and_setup_cannot_undo_owner_off() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let source = store.connect_source("Synthetic", "Fake source", 1).await?;
    assert!(source.enabled);
    assert!(
        store
            .source_grants()
            .await?
            .contains(&source.connector_instance)
    );
    let mut recipe: hx_sources::recipe::Recipe = serde_json::from_value(json!({
        "source":"agent-selected-namespace", "name":"first name",
        "request":{"kind":"http", "method":"GET", "url":"https://synthetic.invalid/messages", "headers":{}, "body":null},
        "items":"/messages", "kind":"source.email", "map":{"id":"/id"},
        "time_format":"rfc3339"
    }))?;
    let first = store.save_recipe(&source.id, &recipe).await?;
    assert_eq!(first.recipe.source, source.id);
    assert!(!first.enabled, "setup has not succeeded");
    let mut state = store.recipe_state(&first.id).await?;
    state.cursor = Some(json!({"page":2}));
    store.save_recipe_state(&state).await?;
    store.set_recipe_enabled(&first.id, false).await?;
    recipe.name = "renamed by the agent".into();
    let saved = store.save_recipe(&source.id, &recipe).await?;
    assert_eq!(saved.id, first.id, "request identity must ignore the name");
    assert!(saved.owner_off, "a re-save must preserve the owner's off");
    assert!(!saved.enabled);
    assert_eq!(store.recipe_state(&first.id).await?.cursor, state.cursor);
    let addon = store
        .save_addon(&source.id, "instructions", "Synthetic instructions")
        .await?;
    store.set_addon_enabled(&addon.id, false).await?;
    let saved_addon = store
        .save_addon(&source.id, "instructions", "Synthetic instructions")
        .await?;
    assert_eq!(saved_addon.id, addon.id);
    assert!(saved_addon.owner_off);
    assert!(!saved_addon.enabled);
    store.set_source_enabled(&source.id, false).await?;
    let generation = store.request_source_setup(&source.id).await?;
    assert!(
        !store
            .finish_source_setup(&source.id, 1, json!({}), None)
            .await?,
        "stale setup must not activate"
    );
    assert!(
        !store
            .finish_source_setup(&source.id, generation, json!({"status":"ready"}), None)
            .await?
    );
    assert!(!store.source(&source.id).await?.unwrap().enabled);
    assert!(store.source_grants().await?.is_empty());
    let saved = store.recipes(Some(&source.id)).await?.remove(0);
    assert!(saved.owner_off && !saved.enabled);
    let saved_addon = store.addons(Some(&source.id)).await?.remove(0);
    assert!(saved_addon.owner_off && !saved_addon.enabled);
    assert!(store.save_recipe(&source.id, &recipe).await.is_err());
    assert!(
        store
            .save_live_snapshot(ulid::Ulid::generate(), &source.id, json!({}))
            .await
            .is_err()
    );
    let jobs: Vec<(String, i64)> = sqlx::query_as("SELECT kind,max_attempts FROM jobs ORDER BY id")
        .fetch_all(&store.pool)
        .await?;
    assert_eq!(jobs, vec![("setup".into(), 2), ("setup".into(), 2)]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM windows")
            .fetch_one(&store.pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM switch_log")
            .fetch_one(&store.pool)
            .await?,
        3
    );
    let agent = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(dir.path().join("agent.sqlite"))
            .read_only(true),
    )
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sqlite_schema WHERE type='table'")
            .fetch_one(&agent)
            .await?,
        0
    );
    agent.close().await;
    Ok(())
}

#[tokio::test]
async fn source_search_text_is_bounded_utf8_and_includes_nested_strings() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let at: chrono::DateTime<chrono::Utc> = "2026-11-05T12:00:00Z".parse()?;
    let envelope: hx_schema::Envelope = serde_json::from_value(json!({
        "envelope_version":1, "observation_id":hx_schema::ObservationId::generate(),
        "connector_instance":"source:synthetic", "source_key":"message", "revision":"r1",
        "operation":"upsert", "kind":"source.email", "schema_version":1,
        "time":hx_schema::ObservationTime::point(at), "sensitivity":"restricted",
        "payload":{"title":"Orchids", "item":{"nested":["greenhouse",{"body":"🌻".repeat(8000)}],"count":999}}
    }))?;
    let ack = store
        .ingest(
            hx_store::UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![envelope],
            },
            &hx_store::Token {
                prefixes: vec!["source:".into()],
                scopes: vec!["ingest".into()],
            },
        )
        .await?;
    assert_eq!(ack.accepted, 1);
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&store.pool)
        .await?;
    store.normalize(id).await?;
    let text: String = sqlx::query_scalar("SELECT search_text FROM normalized WHERE event_id=$1")
        .bind(id)
        .fetch_one(&store.pool)
        .await?;
    assert!(text.starts_with("Orchids"));
    assert!(text.contains("greenhouse"));
    assert!(text.len() <= 16 * 1024);
    assert!(text.len() >= 16 * 1024 - 4);
    assert!(!text.contains("999"));
    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM normalized_fts WHERE normalized_fts MATCH 'greenhouse'",
    )
    .fetch_one(&store.pool)
    .await?;
    assert_eq!(hits, 1);
    Ok(())
}

fn review_recipe() -> hx_sources::recipe::Recipe {
    serde_json::from_value(json!({
        "source":"synthetic", "name":"mail",
        "request":{"kind":"http", "method":"GET", "url":"https://synthetic.invalid/m"},
        "items":"/messages", "kind":"source.email", "map":{"id":"/id"},
        "time_format":"rfc3339"
    }))
    .unwrap()
}

#[tokio::test]
async fn query_and_headers_cannot_replace_an_owner_disabled_pull() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let source = store.connect_source("Synthetic", "Fake only", 1).await?;
    let mut recipe = review_recipe();
    let original = store.save_recipe(&source.id, &recipe).await?;
    store.set_recipe_enabled(&original.id, false).await?;
    for url in [
        "https://synthetic.invalid/m?",
        "https://synthetic.invalid/m?page=2",
        "https://synthetic.invalid/a/../%6d?new=1",
        "https://synthetic.invalid/m/",
    ] {
        recipe.request = serde_json::from_value(
            json!({"kind":"http", "method":"get", "url":url, "headers":{"Accept":"application/json"}}),
        )?;
        let saved = store.save_recipe(&source.id, &recipe).await?;
        assert_eq!(
            saved.id, original.id,
            "method/path identity must ignore query and headers"
        );
        assert!(saved.owner_off && !saved.enabled);
    }
    store
        .finish_source_setup(&source.id, 1, json!({}), None)
        .await?;
    let row = store.recipes(Some(&source.id)).await?.remove(0);
    assert!(row.owner_off && !row.enabled);
    Ok(())
}

#[tokio::test]
async fn setup_completion_on_off_source_cannot_activate_pending_configuration() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let source = store.connect_source("Synthetic", "Fake only", 1).await?;
    store.save_recipe(&source.id, &review_recipe()).await?;
    store
        .save_addon(&source.id, "note", "Synthetic note")
        .await?;
    store.set_source_enabled(&source.id, false).await?;
    assert!(
        !store
            .finish_source_setup(&source.id, 1, json!({}), None)
            .await?
    );
    assert_ne!(store.source(&source.id).await?.unwrap().status, "ready");
    assert!(!store.recipes(Some(&source.id)).await?[0].enabled);
    assert!(!store.addons(Some(&source.id)).await?[0].enabled);
    Ok(())
}

#[tokio::test]
async fn oversized_live_payload_is_rejected_before_waiting_for_writer_or_allocating() -> Result<()>
{
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let source = store.connect_source("Synthetic", "Fake only", 1).await?;
    let before: i64 = sqlx::query_scalar("SELECT next_id FROM evidence_seq")
        .fetch_one(&store.pool)
        .await?;
    let writer = store.begin_write().await?;
    let oversized = json!("x".repeat(hx_sources::request::MAX_HTTP_RESPONSE_BYTES - 1));
    let rejected = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        store.save_live_snapshot(ulid::Ulid::generate(), &source.id, oversized),
    )
    .await;
    assert!(
        matches!(rejected, Ok(Err(_))),
        "oversize must be refused before acquiring a transaction"
    );
    writer.rollback().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT next_id FROM evidence_seq")
            .fetch_one(&store.pool)
            .await?,
        before
    );
    let exact = json!("x".repeat(hx_sources::request::MAX_HTTP_RESPONSE_BYTES - 2));
    assert_eq!(
        serde_json::to_vec(&exact)?.len(),
        hx_sources::request::MAX_HTTP_RESPONSE_BYTES
    );
    assert_eq!(
        store
            .save_live_snapshot(ulid::Ulid::generate(), &source.id, exact)
            .await?,
        before
    );
    Ok(())
}

#[tokio::test]
async fn rewritten_addon_keeps_the_owner_switch() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let source = store.connect_source("Synthetic", "Fake only", 1).await?;
    let first = store.save_addon(&source.id, "context", "Note").await?;
    store.set_addon_enabled(&first.id, false).await?;
    let rewritten = store.save_addon(&source.id, " context ", "Note ").await?;
    assert_eq!(
        rewritten.id, first.id,
        "rewriting a named add-on must not create a new switch"
    );
    assert!(rewritten.owner_off && !rewritten.enabled);
    let changed = store
        .save_addon(&source.id, "context", "Completely rewritten note")
        .await?;
    assert_eq!(changed.id, first.id);
    assert_eq!(changed.text, "Completely rewritten note");
    assert!(changed.owner_off && !changed.enabled);
    store
        .finish_source_setup(&source.id, 1, json!({}), None)
        .await?;
    assert!(store.addons(Some(&source.id)).await?[0].owner_off);
    assert!(!store.addons(Some(&source.id)).await?[0].enabled);
    Ok(())
}
