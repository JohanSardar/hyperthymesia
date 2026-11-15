//! Deleting stored history: a time range the owner picked, or one type of
//! record older than its keep-for limit. Evidence and write-ups are locked
//! against deletion everywhere else; this is the one place that lifts the lock,
//! inside a single write transaction that puts it back before committing.
//!
//! Callers must take a verified backup first and log the returned counts. This
//! module never keeps any of the removed content.
use crate::{Store, Ts, request_window_job};
use anyhow::{Result, ensure};
use chrono::{DateTime, Duration, Utc};
use hx_schema::Interval;
use serde::Serialize;
use sqlx::Row;

/// What to remove.
#[derive(Clone, Debug)]
pub enum Scope {
    /// Everything recorded in `[start, end)`: every record touching it, the
    /// write-ups of every window it touches, and write-ups that used those as
    /// context. Records from this range are refused if they arrive later.
    Range {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    /// Records of these kinds (or, with `connections`, every connection record)
    /// that ended before `before`. Write-ups are kept.
    Older {
        before: DateTime<Utc>,
        kinds: Vec<String>,
        connections: bool,
    },
}

/// How much a delete removes. `plan` and `erase` return the same shape.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub records: i64,
    pub screen_text: i64,
    pub screen_pictures: i64,
    pub text_read_from_pictures: i64,
    pub apps_and_windows: i64,
    pub activity: i64,
    pub connection_records: i64,
    pub files: i64,
    pub file_bytes: i64,
    pub write_ups: i64,
    pub windows: i64,
    pub lookups: i64,
    /// The span the removed records really cover. Records that began before
    /// or ended after a range are removed whole, so this can be wider.
    pub first: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
}

const LOCKS: [&str; 3] = [
    "immutable_events_delete",
    "immutable_revisions_delete",
    "immutable_live_snapshots_delete",
];

impl Scope {
    fn validate(&self) -> Result<()> {
        match self {
            Scope::Range { start, end } => {
                ensure!(end > start, "the range must end after it starts")
            }
            Scope::Older {
                kinds, connections, ..
            } => ensure!(!kinds.is_empty() || *connections, "nothing to delete"),
        }
        Ok(())
    }
}

/// Fills `temp.erase_ids` with the matching record IDs.
async fn select(conn: &mut sqlx::SqliteConnection, scope: &Scope) -> Result<()> {
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS erase_ids(id INTEGER PRIMARY KEY)")
        .execute(&mut *conn)
        .await?;
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS erase_ocr(id INTEGER PRIMARY KEY)")
        .execute(&mut *conn)
        .await?;
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS erase_blobs(sha256 TEXT PRIMARY KEY, dead INTEGER NOT NULL DEFAULT 0)").execute(&mut *conn).await?;
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS erase_windows(start_at INTEGER PRIMARY KEY)")
        .execute(&mut *conn)
        .await?;
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS erase_revs(id INTEGER PRIMARY KEY)")
        .execute(&mut *conn)
        .await?;
    for table in [
        "erase_ids",
        "erase_ocr",
        "erase_blobs",
        "erase_windows",
        "erase_revs",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM temp.{table}")))
            .execute(&mut *conn)
            .await?;
    }
    match scope {
        Scope::Range { start, end } => {
            // Every record overlapping the range, however long ago it began.
            sqlx::query("INSERT INTO temp.erase_ids SELECT id FROM events INDEXED BY events_time WHERE source_start < $2 AND ((source_end IS NULL AND source_start >= $1) OR source_end > $1)")
                .bind(Ts::from(*start))
                .bind(Ts::from(*end))
                .execute(&mut *conn)
                .await?;
        }
        Scope::Older {
            before,
            kinds,
            connections,
        } => {
            sqlx::query("INSERT INTO temp.erase_ids SELECT id FROM events INDEXED BY events_time WHERE source_start < $1 AND COALESCE(source_end,source_start) < $1 AND (kind IN (SELECT value FROM json_each($2)) OR ($3 AND connector_instance LIKE 'source:%'))")
                .bind(Ts::from(*before))
                .bind(serde_json::to_string(kinds)?)
                .bind(*connections)
                .execute(&mut *conn)
                .await?;
        }
    }
    sqlx::query("INSERT INTO temp.erase_ocr SELECT id FROM ocr_evidence WHERE source_event_id IN (SELECT id FROM temp.erase_ids)").execute(&mut *conn).await?;
    sqlx::query("INSERT INTO temp.erase_blobs(sha256) SELECT DISTINCT sha256 FROM event_blobs WHERE event_id IN (SELECT id FROM temp.erase_ids)").execute(&mut *conn).await?;
    sqlx::query("UPDATE temp.erase_blobs SET dead=1 WHERE NOT EXISTS(SELECT 1 FROM event_blobs b WHERE b.sha256=erase_blobs.sha256 AND b.event_id NOT IN (SELECT id FROM temp.erase_ids))").execute(&mut *conn).await?;

    let (first, last): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT min(source_start), max(COALESCE(source_end,source_start)) FROM events WHERE id IN (SELECT id FROM temp.erase_ids)")
            .fetch_one(&mut *conn)
            .await?;
    if let Scope::Range { start, end } = scope {
        // Every window the range or its removed records touch loses its write-up.
        let from = first.map_or(Ts::from(*start).0, |f| f.min(Ts::from(*start).0));
        let to = last.map_or(Ts::from(*end).0, |l| l.max(Ts::from(*end).0));
        sqlx::query("INSERT INTO temp.erase_windows SELECT start_at FROM windows WHERE start_at < $2 AND end_at > $1")
            .bind(from)
            .bind(to)
            .execute(&mut *conn)
            .await?;
        // Write-ups that took one of these as context could repeat what it said.
        sqlx::query("INSERT OR IGNORE INTO temp.erase_revs WITH RECURSIVE d(id) AS (SELECT id FROM revisions WHERE start_at IN (SELECT start_at FROM temp.erase_windows) UNION SELECT rd.revision_id FROM revision_dependencies rd JOIN d ON rd.prior_revision_id=d.id) SELECT id FROM d")
            .execute(&mut *conn)
            .await?;
        sqlx::query("INSERT OR IGNORE INTO temp.erase_windows SELECT start_at FROM revisions WHERE id IN (SELECT id FROM temp.erase_revs)").execute(&mut *conn).await?;
    } else if let (Some(first), Some(last)) = (first, last) {
        // Cached timelines of these windows still hold the removed records.
        sqlx::query("INSERT INTO temp.erase_windows SELECT start_at FROM windows WHERE start_at <= $2 AND end_at > $1")
            .bind(first)
            .bind(last)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

async fn summarize(conn: &mut sqlx::SqliteConnection, scope: &Scope) -> Result<Summary> {
    let row = sqlx::query(
        "SELECT count(*) AS records,
          COALESCE(sum(kind='text.snapshot'),0) AS screen_text,
          COALESCE(sum(kind IN ('media.still','media.chunk')),0) AS pictures,
          COALESCE(sum(kind IN ('focus.span','presence.span','ui.click')),0) AS apps,
          COALESCE(sum(kind='input.bucket'),0) AS activity,
          COALESCE(sum(connector_instance LIKE 'source:%'),0) AS connections,
          min(source_start) AS first, max(COALESCE(source_end,source_start)) AS last
         FROM events WHERE id IN (SELECT id FROM temp.erase_ids)",
    )
    .fetch_one(&mut *conn)
    .await?;
    let ocr: i64 = sqlx::query_scalar("SELECT count(*) FROM temp.erase_ocr")
        .fetch_one(&mut *conn)
        .await?;
    let (files, file_bytes): (i64, i64) = sqlx::query_as("SELECT count(*), COALESCE(sum(b.bytes),0) FROM temp.erase_blobs e JOIN blobs b USING(sha256) WHERE e.dead")
        .fetch_one(&mut *conn)
        .await?;
    let write_ups: i64 = sqlx::query_scalar("SELECT count(*) FROM temp.erase_revs")
        .fetch_one(&mut *conn)
        .await?;
    let windows: i64 = sqlx::query_scalar("SELECT count(*) FROM temp.erase_windows")
        .fetch_one(&mut *conn)
        .await?;
    let lookups: i64 =
        match scope {
            Scope::Range { start, end } => sqlx::query_scalar(
                "SELECT count(*) FROM live_snapshots WHERE created_at >= $1 AND created_at < $2",
            )
            .bind(Ts::from(*start))
            .bind(Ts::from(*end))
            .fetch_one(&mut *conn)
            .await?,
            Scope::Older { .. } => 0,
        };
    let time = |v: Option<i64>| v.and_then(DateTime::<Utc>::from_timestamp_micros);
    Ok(Summary {
        records: row.get("records"),
        screen_text: row.get("screen_text"),
        screen_pictures: row.get("pictures"),
        text_read_from_pictures: ocr,
        apps_and_windows: row.get("apps"),
        activity: row.get("activity"),
        connection_records: row.get("connections"),
        files,
        file_bytes,
        write_ups,
        windows,
        lookups,
        first: time(row.get("first")),
        last: time(row.get("last")),
    })
}

impl Store {
    /// Counts what `erase` would remove, without changing anything.
    pub async fn erase_plan(&self, scope: &Scope) -> Result<Summary> {
        scope.validate()?;
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN").execute(&mut *conn).await?;
        let result = async {
            select(&mut conn, scope).await?;
            summarize(&mut conn, scope).await
        }
        .await;
        sqlx::query("ROLLBACK").execute(&mut *conn).await?;
        result
    }

    /// Removes everything `scope` covers in one transaction and returns what
    /// went. Afterwards the freed pages are wiped, the search index is
    /// compacted and the files nothing refers to any more are removed.
    pub async fn erase(&self, scope: &Scope) -> Result<Summary> {
        scope.validate()?;
        let mut tx = self.begin_write().await?;
        sqlx::query("PRAGMA secure_delete=ON")
            .execute(&mut *tx)
            .await?;
        select(&mut tx, scope).await?;
        let summary = summarize(&mut tx, scope).await?;

        let mut locks = Vec::new();
        for name in LOCKS {
            let sql: Option<String> = sqlx::query_scalar(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=$1",
            )
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(sql) = sql {
                sqlx::query(sqlx::AssertSqlSafe(format!("DROP TRIGGER {name}")))
                    .execute(&mut *tx)
                    .await?;
                locks.push(sql);
            }
        }
        ensure!(
            locks.len() == LOCKS.len(),
            "the deletion locks are not as expected"
        );

        let ids = "(SELECT id FROM temp.erase_ids)";
        let dead = "(SELECT sha256 FROM temp.erase_blobs WHERE dead)";
        for sql in [
            format!("DELETE FROM normalized WHERE event_id IN {ids} OR event_id IN (SELECT id FROM temp.erase_ocr)"),
            format!("DELETE FROM quarantine WHERE event_id IN {ids}"),
            format!("DELETE FROM entity_assertions WHERE EXISTS(SELECT 1 FROM json_each(evidence) j WHERE j.value IN {ids})"),
            format!("DELETE FROM ocr_pending_sources WHERE source_event_id IN {ids} OR sha256 IN {dead}"),
            "DELETE FROM ocr_evidence WHERE id IN (SELECT id FROM temp.erase_ocr)".into(),
            format!("DELETE FROM event_blobs WHERE event_id IN {ids}"),
            "UPDATE blobs SET refcount=(SELECT count(*) FROM event_blobs b WHERE b.sha256=blobs.sha256) WHERE sha256 IN (SELECT sha256 FROM temp.erase_blobs)".into(),
            // A picture shared with a kept record keeps its text; point it there.
            format!("UPDATE ocr_jobs SET source_event_id=(SELECT min(event_id) FROM event_blobs b WHERE b.sha256=ocr_jobs.sha256) WHERE source_event_id IN {ids} AND sha256 NOT IN {dead}"),
            format!("UPDATE ocr_jobs SET sampled_from=NULL WHERE sampled_from IN {dead}"),
            format!("DELETE FROM ocr_jobs WHERE sha256 IN {dead} OR source_event_id IN {ids}"),
            format!("DELETE FROM jobs WHERE kind='normalize' AND json_extract(payload,'$.event_id') IN {ids}"),
            format!("DELETE FROM events WHERE id IN {ids}"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&mut *tx).await?;
        }
        let files: Vec<(String, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT sha256, published FROM blobs WHERE sha256 IN {dead}"
        )))
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM blobs WHERE sha256 IN {dead}"
        )))
        .execute(&mut *tx)
        .await?;

        let windows: Vec<(i64, i64)> = sqlx::query_as("SELECT start_at,end_at FROM windows WHERE start_at IN (SELECT start_at FROM temp.erase_windows)")
            .fetch_all(&mut *tx)
            .await?;
        if let Scope::Range { start, end } = scope {
            let revs = "(SELECT id FROM temp.erase_revs)";
            for sql in [
                format!("UPDATE windows SET canonical_revision=NULL WHERE canonical_revision IN {revs}"),
                format!("DELETE FROM revision_dependencies WHERE revision_id IN {revs} OR prior_revision_id IN {revs}"),
                format!("UPDATE agent_runs SET revision_id=NULL WHERE revision_id IN {revs}"),
                format!("DELETE FROM revisions WHERE id IN {revs}"),
                // The window is read again from what is left; its next write-up is new.
                "UPDATE windows SET generation=generation+1,dirty=1,timeline=NULL,timeline_hash=NULL,compiled_generation=NULL,empty_generation=NULL,revision24_finished=0 WHERE start_at IN (SELECT start_at FROM temp.erase_windows)".into(),
            ] {
                sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&mut *tx).await?;
            }
            sqlx::query("DELETE FROM live_snapshots WHERE created_at >= $1 AND created_at < $2")
                .bind(Ts::from(*start))
                .bind(Ts::from(*end))
                .execute(&mut *tx)
                .await?;
            let first = summary.first.map_or(*start, |f| f.min(*start));
            let last = summary.last.map_or(*end, |l| l.max(*end));
            sqlx::query("INSERT INTO erased_ranges(start_at,end_at,created_at) VALUES($1,$2,$3)")
                .bind(Ts::from(first))
                .bind(Ts::from(if last > first {
                    last
                } else {
                    first + Duration::microseconds(1)
                }))
                .bind(Ts::now())
                .execute(&mut *tx)
                .await?;
            let now = Ts::now();
            for (start_at, end_at) in &windows {
                let window = Interval {
                    start: DateTime::<Utc>::from_timestamp_micros(*start_at).unwrap_or_default(),
                    end: DateTime::<Utc>::from_timestamp_micros(*end_at).unwrap_or_default(),
                };
                let generation: i64 =
                    sqlx::query_scalar("SELECT generation FROM windows WHERE start_at=$1")
                        .bind(start_at)
                        .fetch_one(&mut *tx)
                        .await?;
                request_window_job(&mut tx, "compile", window, generation, now).await?;
            }
        } else {
            // Keep each window's write-up and generation; only its cached
            // timeline goes, and is rebuilt from what is left when next read.
            sqlx::query("UPDATE windows SET timeline=NULL,timeline_hash=NULL,compiled_generation=NULL WHERE start_at IN (SELECT start_at FROM temp.erase_windows)")
                .execute(&mut *tx)
                .await?;
        }

        for sql in locks {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("PRAGMA secure_delete=OFF")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        // Old words stay in the search index's segments until it is merged.
        let mut tx = self.begin_write().await?;
        sqlx::query("PRAGMA secure_delete=ON")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO normalized_fts(normalized_fts) VALUES('optimize')")
            .execute(&mut *tx)
            .await?;
        sqlx::query("PRAGMA secure_delete=OFF")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await?;

        for (sha, published) in files {
            if !crate::valid_hash(&sha) {
                continue;
            }
            let path = self.data_dir.join("blobs").join(&sha[..2]).join(&sha);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !published => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(summary)
    }

    /// Whether a record from this time falls in a range the owner deleted.
    pub async fn erased(&self, start: DateTime<Utc>, end: Option<DateTime<Utc>>) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM erased_ranges WHERE start_at < $2 AND end_at > $1)",
        )
        .bind(Ts::from(start))
        .bind(Ts::from(end.unwrap_or(start) + Duration::microseconds(1)))
        .fetch_one(&self.pool)
        .await?)
    }
}
