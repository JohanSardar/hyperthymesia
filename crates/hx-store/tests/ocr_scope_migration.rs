//! Upgrades a synthetic core through the real migration files, as an installed
//! store would be, because a table rebuild silently drops whatever hangs off it.
use anyhow::Result;
use hx_store::Store;
use sqlx::{
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{borrow::Cow, collections::BTreeMap};

static CURRENT: Migrator = sqlx::migrate!("./migrations-sqlite");
const SCOPE_MIGRATION: i64 = 4;

async fn schema_objects(store: &Store) -> Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name,sql FROM sqlite_schema WHERE type IN ('trigger','view','index') AND sql IS NOT NULL",
    )
    .fetch_all(&store.pool)
    .await?;
    Ok(rows.into_iter().collect())
}

async fn refused(store: &Store, sql: &str, reason: &str) -> Result<()> {
    let error = sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&store.pool)
        .await
        .expect_err(sql)
        .to_string();
    assert!(error.contains(reason), "{sql}: {error}");
    Ok(())
}

#[tokio::test]
async fn scope_migration_keeps_rows_and_every_dependent_guard() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(dir.path().join("core.sqlite"))
                .create_if_missing(true)
                .foreign_keys(true)
                .pragma("recursive_triggers", "ON"),
        )
        .await?;
    let store = Store::new(pool, dir.path());
    let previous = Migrator {
        migrations: Cow::Owned(
            CURRENT
                .iter()
                .filter(|m| m.version < SCOPE_MIGRATION)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    let mut tx = store.begin_write().await?;
    previous.run(&mut *tx).await?;
    tx.commit().await?;

    let sha = "a".repeat(64);
    let event = "INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES($1,$2,'test:still',$2,'1','media.still',1,1,'normal','{\"kind\":\"media.still\"}',1)";
    for (id, observation) in [(10, "synthetic-event-10"), (11, "synthetic-event-11")] {
        sqlx::query(event)
            .bind(id)
            .bind(observation)
            .execute(&store.pool)
            .await?;
    }
    sqlx::query("INSERT INTO blobs(sha256,created_at) VALUES($1,1)")
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    sqlx::query("INSERT INTO event_blobs VALUES(10,$1)")
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    // The second row sorts first by key, so its rowid would not survive a plain copy.
    sqlx::query("INSERT INTO ocr_jobs(rowid,sha256,engine_version,source_event_id,available_at,status,result) VALUES(7,$1,'vision:b',10,1,'complete','{\"lines\":[]}'),(9,$1,'vision:a',11,1,'failed',NULL)")
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    sqlx::query("INSERT INTO ocr_evidence(id,observation_id,source_event_id,sha256,engine_version) VALUES(100,'synthetic-evidence',10,$1,'vision:b')")
        .bind(&sha)
        .execute(&store.pool)
        .await?;
    sqlx::query("INSERT INTO sources(id,connector_instance,name,description,connected_at) VALUES('s','source:s','synthetic','synthetic',1)")
        .execute(&store.pool)
        .await?;
    sqlx::query("INSERT INTO live_snapshots(id,run_token,connector_instance,revision,envelope,created_at) VALUES(200,'run','source:s','1','{}',1)")
        .execute(&store.pool)
        .await?;
    let before = schema_objects(&store).await?;

    let mut tx = store.begin_write().await?;
    CURRENT.run(&mut *tx).await?;
    tx.commit().await?;

    // Only the view reads a new join key. Every other object keeps its exact text.
    let mut after = schema_objects(&store).await?;
    let mut expected = before;
    assert!(expected.remove("ocr_derived").is_some());
    assert!(after.remove("ocr_derived").is_some());
    assert_eq!(after, expected);

    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&store.pool)
        .await?;
    assert_eq!(foreign_keys, 1);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&store.pool)
            .await?
            .is_empty()
    );
    let jobs: Vec<(i64, String, String, String)> =
        sqlx::query_as("SELECT rowid,engine_version,scope,status FROM ocr_jobs ORDER BY rowid")
            .fetch_all(&store.pool)
            .await?;
    assert_eq!(
        jobs,
        [
            (7, "vision:b".into(), String::new(), "complete".into()),
            (9, "vision:a".into(), String::new(), "failed".into())
        ]
    );
    let derived: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id,json_extract(envelope,'$.payload.engine_version') FROM ocr_derived",
    )
    .fetch_all(&store.pool)
    .await?;
    assert_eq!(derived, [(100, "vision:b".to_owned())]);

    // Evidence IDs stay unique in every direction, for all three evidence tables.
    let live = "evidence ID already exists in live_snapshots";
    let ocr = "evidence ID already exists in ocr_evidence";
    let events = "evidence ID already exists in events";
    let insert_ocr = |id: i64| {
        format!(
            "INSERT INTO ocr_evidence(id,observation_id,source_event_id,sha256,engine_version) VALUES({id},'synthetic-new',11,'{sha}','vision:a')"
        )
    };
    let insert_live = |id: i64| {
        format!(
            "INSERT INTO live_snapshots(id,run_token,connector_instance,revision,envelope,created_at) VALUES({id},'run-new','source:s','2','{{}}',1)"
        )
    };
    let insert_event = |id: i64| {
        format!(
            "INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES({id},'synthetic-new','test:still','new','1','media.still',1,1,'normal','{{}}',1)"
        )
    };
    refused(&store, &insert_ocr(200), live).await?;
    refused(&store, "UPDATE ocr_evidence SET id=200 WHERE id=100", live).await?;
    refused(&store, &insert_ocr(10), events).await?;
    refused(&store, &insert_live(100), ocr).await?;
    refused(&store, &insert_live(10), events).await?;
    refused(&store, &insert_event(100), ocr).await?;
    refused(&store, &insert_event(200), live).await?;
    store.pool.close().await;
    Ok(())
}
