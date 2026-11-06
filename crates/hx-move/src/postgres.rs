//! The production source has no migrations or write queries. Even accidental
//! writes are rejected by the server, from connection startup through close.
use crate::{
    core::*,
    schema::{Table, tables, target_table},
};
use anyhow::{Result, ensure};
use futures_util::TryStreamExt;
use hx_store::{Store, job_window_start, record_revision_dependencies, revision_dependency_ids};
use serde_json::Value;
use sqlx::{
    Connection, Executor, PgConnection, Row,
    postgres::{PgConnectOptions, PgRow},
};
use std::str::FromStr;

pub static FROZEN: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub struct Snapshot {
    pub(crate) conn: PgConnection,
    initial_sequences: std::collections::BTreeMap<String, i64>,
}
impl Snapshot {
    pub async fn open(url: &str) -> Result<Self> {
        let options =
            PgConnectOptions::from_str(url)?.options([("default_transaction_read_only", "on")]);
        let mut conn = PgConnection::connect_with(&options).await?;
        conn.execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await?;
        let mut source = Self {
            conn,
            initial_sequences: Default::default(),
        };
        source.preflight().await?;
        source.initial_sequences = source.sequences().await?;
        Ok(source)
    }
    async fn preflight(&mut self) -> Result<()> {
        let read_only: bool = sqlx::query_scalar("SELECT current_setting('transaction_read_only')='on' AND current_setting('default_transaction_read_only')='on'").fetch_one(&mut self.conn).await?;
        ensure!(read_only, "source connection must be read only");
        let applied: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
            "SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&mut self.conn)
        .await?;
        ensure!(
            applied.len() == FROZEN.iter().len(),
            "source migration version mismatch"
        );
        for ((version, success, checksum), frozen) in applied.iter().zip(FROZEN.iter()) {
            ensure!(
                *success
                    && *version == frozen.version
                    && checksum.as_slice() == frozen.checksum.as_ref(),
                "source migration mismatch"
            );
        }
        let running: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE state='running' AND lease_until>statement_timestamp())").fetch_one(&mut self.conn).await?;
        ensure!(!running, "source has a live lease");
        let collision: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events JOIN ocr_evidence USING(id))")
                .fetch_one(&mut self.conn)
                .await?;
        ensure!(!collision, "overlapping evidence IDs");
        let invalid_sequence: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM watermarks WHERE sequence<0 OR sequence>18446744073709551615 OR sequence<>trunc(sequence))").fetch_one(&mut self.conn).await?;
        ensure!(!invalid_sequence, "watermark sequence outside u64 range");
        Ok(())
    }
    pub async fn hashes(&mut self) -> Result<Inventory> {
        let mut hashes = Inventory::new();
        for table in tables().into_iter().filter(|t| t.legacy) {
            let sql = table.postgres_select();
            let mut stream = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).fetch(&mut self.conn);
            let mut hash = RowHasher::default();
            while let Some(row) = stream.try_next().await? {
                hash.add(&decode(&table, &row)?);
            }
            hashes.insert(table.name.to_owned(), hash.finish());
        }
        Ok(hashes)
    }
    /// Load exact source text and compute expected target hashes independently
    /// of SQLite readback. Only the extra schema columns are derived in Rust.
    pub async fn load(&mut self, store: &Store) -> Result<Inventory> {
        let mut expected = Inventory::new();
        let mut dependencies_hash = RowHasher::default();
        let mut tx = store.begin_write().await?;
        sqlx::query("PRAGMA defer_foreign_keys=ON")
            .execute(&mut *tx)
            .await?;
        for table in tables().into_iter().filter(|t| t.legacy) {
            let sql = table.postgres_select();
            let target = target_table(&table);
            let mut stream = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).fetch(&mut self.conn);
            let mut hash = RowHasher::default();
            while let Some(row) = stream.try_next().await? {
                let mut cells = decode(&table, &row)?;
                if table.name == "revisions" {
                    let content: Value = serde_json::from_str(cells[3].text()?)?;
                    let run: Option<Value> = match &cells[7] {
                        Cell::Null => None,
                        cell => Some(serde_json::from_str(cell.text()?)?),
                    };
                    let deps = revision_dependency_ids(&content, run.as_ref());
                    cells.push(Cell::Integer(i64::from(deps.is_some())));
                    insert(&mut tx, &target, &cells).await?;
                    let id = cells[0].integer()?;
                    record_revision_dependencies(&mut tx, id, deps.as_deref()).await?;
                    for prior in deps.into_iter().flatten() {
                        dependencies_hash.add(&[Cell::Integer(id), Cell::Integer(prior)]);
                    }
                } else {
                    if table.name == "jobs" {
                        let payload: Value = serde_json::from_str(cells[3].text()?)?;
                        cells.push(
                            job_window_start(&payload)?.map_or(Cell::Null, |t| Cell::Integer(t.0)),
                        );
                        cells.push(
                            payload
                                .get("generation")
                                .map(|g| {
                                    g.as_i64()
                                        .map(Cell::Integer)
                                        .ok_or_else(|| anyhow::anyhow!("invalid job generation"))
                                })
                                .transpose()?
                                .unwrap_or(Cell::Null),
                        );
                    }
                    insert(&mut tx, &target, &cells).await?;
                }
                hash.add(&cells);
            }
            expected.insert(table.name.to_owned(), hash.finish());
        }
        expected.insert("revision_dependencies".into(), dependencies_hash.finish());
        // Read sequence state, never nextval/setval. PostgreSQL sequences are
        // not MVCC: the owner must stop writers; a second read fences movement.
        let sequences = self.initial_sequences.clone();
        let max_id: i64 = sqlx::query_scalar("SELECT max(coalesce((SELECT max(id) FROM events),0),coalesce((SELECT max(id) FROM ocr_evidence),0))").fetch_one(&mut *tx).await?;
        let next = sequences["events"]
            .max(max_id)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("evidence sequence exhausted"))?;
        ensure!(next > 0, "invalid evidence sequence");
        sqlx::query("UPDATE evidence_seq SET next_id=$1")
            .bind(next)
            .execute(&mut *tx)
            .await?;
        let mut hash = RowHasher::default();
        hash.add(&[Cell::Integer(next)]);
        expected.insert("evidence_seq".into(), hash.finish());
        let mut hash = RowHasher::default();
        for name in ["entity_assertions", "jobs", "revisions"] {
            let max: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT coalesce(max(id),0) FROM {name}"
            )))
            .fetch_one(&mut *tx)
            .await?;
            let seq = sequences[name].max(max);
            sqlx::query("DELETE FROM sqlite_sequence WHERE name=$1")
                .bind(name)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO sqlite_sequence(name,seq) VALUES($1,$2)")
                .bind(name)
                .bind(seq)
                .execute(&mut *tx)
                .await?;
            hash.add(&[Cell::Text(name.to_owned()), Cell::Integer(seq)]);
        }
        expected.insert("sqlite_sequence".into(), hash.finish());
        // Seed every known engine's progress and all outstanding associations.
        // Completed lineage is excluded; failed/sampled sources stay dormant
        // but can reactivate via the runtime reset trigger. New engine versions
        // intentionally start at zero, as they did before cutover.
        let queries = [
            (
                "ocr_scan_progress",
                "SELECT engine_version,coalesce((SELECT max(id) FROM events WHERE kind='media.still'),0)::text,coalesce((SELECT max(eb.sha256 COLLATE \"C\") FROM event_blobs eb WHERE eb.event_id=(SELECT max(id) FROM events WHERE kind='media.still')),'') FROM (SELECT DISTINCT engine_version FROM ocr_jobs) v ORDER BY engine_version COLLATE \"C\"",
            ),
            (
                "ocr_pending_sources",
                "SELECT v.engine_version,e.id::text,eb.sha256,(CASE WHEN o.status IN ('failed','sampled') THEN 1 ELSE 0 END)::text FROM (SELECT DISTINCT engine_version FROM ocr_jobs) v CROSS JOIN events e JOIN event_blobs eb ON eb.event_id=e.id LEFT JOIN ocr_jobs o ON o.engine_version=v.engine_version AND o.sha256=eb.sha256 WHERE e.kind='media.still' AND NOT EXISTS(SELECT 1 FROM ocr_evidence d WHERE d.source_event_id=e.id AND d.sha256=eb.sha256 AND d.engine_version=v.engine_version) ORDER BY v.engine_version COLLATE \"C\",e.id,eb.sha256 COLLATE \"C\"",
            ),
        ];
        for (name, sql) in queries {
            let table = tables()
                .into_iter()
                .find(|t| t.name == name)
                .expect("derived table");
            let mut stream = sqlx::query(sql).fetch(&mut self.conn);
            let mut hash = RowHasher::default();
            while let Some(row) = stream.try_next().await? {
                let cells = decode(&table, &row)?;
                insert(&mut tx, &table, &cells).await?;
                hash.add(&cells);
            }
            expected.insert(name.into(), hash.finish());
        }
        ensure!(
            self.sequences().await? == sequences,
            "source sequences changed during move"
        );
        tx.commit().await?;
        Ok(expected)
    }
    async fn sequences(&mut self) -> Result<std::collections::BTreeMap<String, i64>> {
        let mut result = std::collections::BTreeMap::new();
        for name in ["events", "revisions", "jobs", "entity_assertions"] {
            let value: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT last_value FROM {name}_id_seq"
            )))
            .fetch_one(&mut self.conn)
            .await?;
            result.insert(name.to_owned(), value);
        }
        Ok(result)
    }
    pub async fn ensure_unchanged(&mut self) -> Result<()> {
        ensure!(
            self.sequences().await? == self.initial_sequences,
            "source sequences changed during move"
        );
        Ok(())
    }
    pub async fn close(self) -> Result<()> {
        self.conn.close().await?;
        Ok(())
    }
}
fn decode(table: &Table, row: &PgRow) -> Result<DataRow> {
    table
        .columns
        .iter()
        .enumerate()
        .map(|(i, (_, kind))| {
            let text: Option<String> = row.try_get(i)?;
            Ok(match text {
                None => Cell::Null,
                Some(text) if matches!(kind, 'i' | 'b' | 't') => Cell::Integer(text.parse()?),
                Some(text) => Cell::Text(text),
            })
        })
        .collect()
}
