//! Deliberately below the store interface: these prove the SQLite substrate
//! before any production ingest, publishing, or search path is ported to it.
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use hx_store::{
    Ts,
    sqlite::{Store, WriteTransaction, search_query},
};
use sqlx::Row;

const EVENT: &str = "INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES((SELECT next_id FROM evidence_seq),$1,'synthetic:',$1,'r1','text.snapshot',$2,$2,'normal','{}',$2) ON CONFLICT(connector_instance,source_key,revision) DO NOTHING RETURNING id";
const OCR: &str = "INSERT INTO ocr_evidence(id,observation_id,source_event_id,sha256,engine_version) VALUES((SELECT next_id FROM evidence_seq),$1,$2,$3,'test/v1') ON CONFLICT DO NOTHING RETURNING id";

async fn event(tx: &mut WriteTransaction, key: &str, time: Ts) -> Result<Option<i64>> {
    tx.insert_evidence(sqlx::query_scalar(EVENT).bind(key).bind(time))
        .await
}

async fn ocr(tx: &mut WriteTransaction, key: &str, source: i64, hash: &str) -> Result<Option<i64>> {
    tx.insert_evidence(sqlx::query_scalar(OCR).bind(key).bind(source).bind(hash))
        .await
}

async fn search(store: &Store, text: &str) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT rowid FROM normalized_fts WHERE normalized_fts MATCH $1 ORDER BY rowid",
    )
    .bind(search_query(text))
    .fetch_all(&store.pool)
    .await?)
}

#[tokio::test]
async fn timestamps_preserve_exact_window_boundaries() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    for boundary in ["2026-11-05T04:00:00Z", "1970-01-01T00:00:00Z"] {
        let start: DateTime<Utc> = boundary.parse()?;
        let end = start + Duration::hours(4);
        let mut tx = store.begin_write().await?;
        for (key, time) in [
            ("before", start - Duration::microseconds(1)),
            ("start", start),
            ("inside", start + Duration::nanoseconds(1_999)),
            ("last", end - Duration::microseconds(1)),
            ("end", end),
        ] {
            let encoded = Ts::from(time);
            assert_eq!(encoded.0, time.timestamp_micros());
            let decoded: Ts = sqlx::query_scalar("SELECT $1")
                .bind(encoded)
                .fetch_one(&mut *tx)
                .await?;
            assert_eq!(decoded, encoded);
            event(&mut tx, &format!("{boundary}:{key}"), encoded).await?;
        }
        let keys: Vec<String> = sqlx::query_scalar("SELECT source_key FROM events WHERE source_start >= $1 AND source_start < $2 ORDER BY source_start")
            .bind(Ts::from(start)).bind(Ts::from(end)).fetch_all(&mut *tx).await?;
        assert_eq!(
            keys,
            ["start", "inside", "last"].map(|key| format!("{boundary}:{key}"))
        );
        // A fresh row tests STRICT, independently of the immutability trigger.
        assert!(
            sqlx::query("INSERT INTO windows(start_at,end_at) VALUES('not an integer',1)")
                .execute(&mut *tx)
                .await
                .is_err()
        );
        tx.rollback().await?;
    }
    Ok(())
}

#[tokio::test]
async fn evidence_ids_are_shared_without_collisions_or_burns() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let mut tx = store.begin_write().await?;
    assert_eq!(event(&mut tx, "first", Ts(1)).await?, Some(1));
    assert_eq!(event(&mut tx, "first", Ts(1)).await?, None);
    let hash = "a".repeat(64);
    sqlx::query("INSERT INTO blobs(sha256,created_at) VALUES($1,1)")
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,available_at) VALUES($1,'test/v1',1,1)").bind(&hash).execute(&mut *tx).await?;
    assert_eq!(ocr(&mut tx, "ocr", 1, &hash).await?, Some(2));
    assert_eq!(ocr(&mut tx, "ocr", 1, &hash).await?, None);
    // Both directions, including explicit IDs as used by the future mover.
    let error = sqlx::query("INSERT INTO events SELECT 2,'collision',connector_instance,'collision',revision,kind,source_start,source_end,observed_at,device,sensitivity,envelope,received_at FROM events WHERE id=1").execute(&mut *tx).await.expect_err("cross-table event ID collision");
    assert!(
        error
            .to_string()
            .contains("evidence ID already exists in ocr_evidence")
    );
    let error = sqlx::query("INSERT INTO ocr_evidence(id,observation_id,source_event_id,sha256,engine_version) VALUES(1,'collision',1,$1,'test/v1')").bind(&hash).execute(&mut *tx).await.expect_err("cross-table OCR ID collision");
    assert!(
        error
            .to_string()
            .contains("evidence ID already exists in events")
    );
    assert!(ocr(&mut tx, "bad-source", 999, &hash).await.is_err());
    assert_eq!(event(&mut tx, "second", Ts(2)).await?, Some(3));
    tx.commit().await?;
    let mut tx = store.begin_write().await?;
    assert_eq!(event(&mut tx, "rolled-back", Ts(3)).await?, Some(4));
    tx.rollback().await?;
    let mut tx = store.begin_write().await?;
    assert_eq!(event(&mut tx, "dropped", Ts(3)).await?, Some(4));
    drop(tx);
    // Two pools exercising the same on-disk allocator and BEGIN IMMEDIATE.
    let other = Store::open(dir.path()).await?;
    let write = |store: Store, key: &'static str| async move {
        let mut tx = store.begin_write().await?;
        let id = event(&mut tx, key, Ts(4)).await?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>(id)
    };
    let (a, b) = tokio::try_join!(write(store, "concurrent-a"), write(other, "concurrent-b"))?;
    let mut ids = [a.unwrap(), b.unwrap()];
    ids.sort();
    assert_eq!(ids, [4, 5]);
    Ok(())
}

#[tokio::test]
async fn immutable_rows_reject_updates_deletes_and_replacement() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let mut tx = store.begin_write().await?;
    event(&mut tx, "original", Ts(1)).await?;
    sqlx::query("INSERT INTO windows(start_at,end_at) VALUES(0,14400000000)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO revisions(start_at,input_generation,interpretation,created_at,output_key) VALUES(0,1,'{}',1,'original')").execute(&mut *tx).await?;
    for statement in [
        "UPDATE events SET revision='changed' WHERE id=1",
        "DELETE FROM events WHERE id=1",
        "INSERT OR REPLACE INTO events SELECT * FROM events WHERE id=1",
        "INSERT OR REPLACE INTO events SELECT 99,observation_id,connector_instance,source_key,revision,kind,source_start,source_end,observed_at,device,sensitivity,envelope,received_at FROM events WHERE id=1",
        "UPDATE revisions SET interpretation='null' WHERE id=1",
        "DELETE FROM revisions WHERE id=1",
        "INSERT OR REPLACE INTO revisions SELECT * FROM revisions WHERE id=1",
        "INSERT OR REPLACE INTO revisions(start_at,input_generation,interpretation,created_at,output_key) VALUES(0,1,'null',2,'original')",
    ] {
        let error = sqlx::query(statement)
            .execute(&mut *tx)
            .await
            .expect_err(statement);
        assert!(
            error.to_string().contains("immutable"),
            "{statement}: {error}"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
            .fetch_one(&mut *tx)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT interpretation FROM revisions")
            .fetch_one(&mut *tx)
            .await?,
        "{}"
    );
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn search_token_table_matches_literal_terms() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let mut tx = store.begin_write().await?;
    for (key, text) in [
        ("one", "Alpha example.com/path café"),
        ("two", "alpha beta cafe"),
        ("three", "beta \"quoted\""),
    ] {
        let id = event(&mut tx, key, Ts(1)).await?.unwrap();
        sqlx::query("INSERT INTO normalized(event_id,normalizer_version,payload,search_text) VALUES($1,'test/v1','{}',$2)")
            .bind(id).bind(text).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    for (query, expected) in [
        ("alpha", vec![1, 2]),
        ("ALPHA beta", vec![2]),
        ("example.com", vec![1]),
        ("example", vec![1]),
        ("path", vec![1]),
        ("café", vec![1]),
        ("cafe", vec![2]),
        ("\"quoted\"", vec![3]),
        ("", vec![]),
        (" \t\n", vec![]),
        ("!!!", vec![]),
        ("OR", vec![]),
        ("AND", vec![]),
        ("alpha OR beta", vec![]),
        ("alpha*", vec![1, 2]),
    ] {
        assert_eq!(search(&store, query).await?, expected, "{query:?}");
    }
    sqlx::query("INSERT INTO normalized_fts(normalized_fts,rank) VALUES('integrity-check',1)")
        .execute(&store.pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn reopen_preserves_schema_ids_and_paid_run_state() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let state = dir.path().join("state");
    assert!(Store::open(&state).await.is_err());
    assert!(!state.exists(), "open must never create an empty state");
    let store = Store::create(&state).await?;
    assert!(Store::create(&state).await.is_err());
    // Hold every connection so settings cannot accidentally be checked on the
    // same connection eight times. These settings are connection-local.
    let mut connections = Vec::new();
    for _ in 0..8 {
        let mut conn = store.pool.acquire().await?;
        for (pragma, expected) in [
            ("PRAGMA foreign_keys", 1),
            ("PRAGMA recursive_triggers", 1),
            ("PRAGMA synchronous", 2),
            ("PRAGMA busy_timeout", 30_000),
        ] {
            let actual: i64 = sqlx::query_scalar(pragma).fetch_one(&mut *conn).await?;
            assert_eq!(actual, expected, "{pragma}");
        }
        #[cfg(target_os = "macos")]
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA fullfsync")
                .fetch_one(&mut *conn)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
                .fetch_one(&mut *conn)
                .await?,
            "wal"
        );
        connections.push(conn);
    }
    drop(connections);
    let mut tx = store.begin_write().await?;
    event(&mut tx, "persistent", Ts(42)).await?;
    sqlx::query("INSERT INTO normalized(event_id,normalizer_version,payload,search_text) VALUES(1,'test/v1','{}','persistent')").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO windows(start_at,end_at,input_changed_at,timeline_hash,revision24_finished,empty_generation) VALUES(0,14400000000,1,'hash',1,1)").execute(&mut *tx).await?;
    sqlx::query("UPDATE windows SET generation=2 WHERE start_at=0")
        .execute(&mut *tx)
        .await?;
    let changed: Ts = sqlx::query_scalar("SELECT input_changed_at FROM windows")
        .fetch_one(&mut *tx)
        .await?;
    assert!(changed.0 > 1);
    sqlx::query("UPDATE windows SET dirty=0,generation=2 WHERE start_at=0")
        .execute(&mut *tx)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, Ts>("SELECT input_changed_at FROM windows")
            .fetch_one(&mut *tx)
            .await?,
        changed
    );
    sqlx::query("INSERT INTO revisions(start_at,input_generation,interpretation,created_at,has_dependency_record) VALUES(0,2,'{}',42,1),(0,1,'{}',41,0)").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO revision_dependencies(revision_id,prior_revision_id) VALUES(1,2)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE windows SET canonical_revision=1 WHERE start_at=0")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO daily_sweeps(day,start_at,generation,started) VALUES('2026-11-05',0,2,1)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO jobs(output_key,kind,payload,available_at,window_start,generation,attempts,revivals,request_version,leased_version) VALUES('test','interpret','{}',42,0,2,4,3,9,8)").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO watermarks(connector_instance,observed_through,sequence) VALUES('synthetic:',42,'18446744073709551615')").execute(&mut *tx).await?;
    // WAL readers keep working while an uncommitted BEGIN IMMEDIATE is held.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
            .fetch_one(&store.pool)
            .await?,
        0
    );
    tx.commit().await?;
    store.pool.close().await;
    let store = Store::open(&state).await?;
    assert_eq!(search(&store, "persistent").await?, [1]);
    let row = sqlx::query("SELECT timeline_hash,revision24_finished,canonical_revision,empty_generation,input_changed_at FROM windows").fetch_one(&store.pool).await?;
    assert_eq!(row.get::<String, _>("timeline_hash"), "hash");
    assert!(row.get::<bool, _>("revision24_finished"));
    assert_eq!(row.get::<i64, _>("canonical_revision"), 1);
    assert_eq!(row.get::<i64, _>("empty_generation"), 1);
    assert_eq!(row.get::<Ts, _>("input_changed_at"), changed);
    let paid: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT window_start,generation,attempts,revivals,request_version,leased_version FROM jobs",
    )
    .fetch_one(&store.pool)
    .await?;
    assert_eq!(paid, (0, 2, 4, 3, 9, 8));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT sequence FROM watermarks")
            .fetch_one(&store.pool)
            .await?,
        "18446744073709551615"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT started FROM daily_sweeps")
            .fetch_one(&store.pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT prior_revision_id FROM revision_dependencies WHERE revision_id=1"
        )
        .fetch_one(&store.pool)
        .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&store.pool)
            .await?,
        "ok"
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&store.pool)
            .await?
            .is_empty()
    );
    let mut tx = store.begin_write().await?;
    assert_eq!(event(&mut tx, "after-reopen", Ts(43)).await?, Some(2));
    let revision: i64 = sqlx::query_scalar("INSERT INTO revisions(start_at,input_generation,interpretation,created_at) VALUES(0,2,'{}',43) RETURNING id").fetch_one(&mut *tx).await?;
    assert_eq!(revision, 3);
    tx.commit().await?;
    Ok(())
}
