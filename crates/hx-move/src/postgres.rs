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
        source.ensure_no_clients().await?;
        source.preflight().await?;
        source.initial_sequences = source.sequences().await?;
        Ok(source)
    }
    pub async fn ensure_no_clients(&mut self) -> Result<()> {
        // Activity statistics can be cached inside a transaction. Clear only
        // this backend's statistics snapshot, never persistent source data.
        sqlx::query("SELECT pg_stat_clear_snapshot()")
            .execute(&mut self.conn)
            .await?;
        let clients: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datid=(SELECT oid FROM pg_database WHERE datname=current_database()) AND pid<>pg_backend_pid() AND backend_type='client backend'").fetch_one(&mut self.conn).await?;
        ensure!(
            clients == 0,
            "source database has {clients} other client session(s); stop the server and close database clients before moving"
        );
        Ok(())
    }
    pub async fn event_count(&mut self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&mut self.conn)
            .await?)
    }
    async fn preflight(&mut self) -> Result<()> {
        self.schema_check().await?;
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
    async fn schema_check(&mut self) -> Result<()> {
        use std::collections::{BTreeMap, BTreeSet};
        let mut expected: BTreeMap<String, BTreeSet<String>> = tables()
            .into_iter()
            .filter(|t| t.legacy)
            .map(|table| {
                (
                    format!("public.{}", table.name),
                    table.columns.into_iter().map(|c| c.0.to_owned()).collect(),
                )
            })
            .collect();
        expected
            .get_mut("public.normalized")
            .expect("normalized")
            .insert("search_vector".into());
        expected.insert(
            "public._sqlx_migrations".into(),
            [
                "version",
                "description",
                "installed_on",
                "success",
                "checksum",
                "execution_time",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
        let columns: Vec<(String,String)> = sqlx::query_as("SELECT n.nspname || '.' || c.relname,a.attname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped WHERE c.relkind IN ('r','p','f','m') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_%' ORDER BY n.nspname,c.relname,a.attnum").fetch_all(&mut self.conn).await?;
        let mut actual = BTreeMap::<String, BTreeSet<String>>::new();
        // Include zero-column tables too, so even an empty extra table refuses.
        let names: Vec<String> = sqlx::query_scalar("SELECT n.nspname || '.' || c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','f','m') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_%'").fetch_all(&mut self.conn).await?;
        for name in names {
            actual.entry(name).or_default();
        }
        for (table, column) in columns {
            actual.entry(table).or_default().insert(column);
        }
        for (table, columns) in &actual {
            let known = expected.get(table).ok_or_else(|| {
                anyhow::anyhow!("unknown source table {table:?}; refusing schema drift")
            })?;
            ensure!(
                columns == known,
                "source table {table:?} has unexpected or missing columns: extra {:?}, missing {:?}",
                columns.difference(known).collect::<Vec<_>>(),
                known.difference(columns).collect::<Vec<_>>()
            );
        }
        ensure!(
            actual.keys().eq(expected.keys()),
            "source schema has missing tables"
        );
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
        self.load_with(store, Table::postgres_select).await
    }
    // Private seam used only by the ignored unit regression to inject a wrong reader.
    async fn load_with(
        &mut self,
        store: &Store,
        projection: fn(&Table) -> String,
    ) -> Result<Inventory> {
        let mut expected = Inventory::new();
        let mut dependencies_hash = RowHasher::default();
        let mut tx = store.begin_write().await?;
        sqlx::query("PRAGMA defer_foreign_keys=ON")
            .execute(&mut *tx)
            .await?;
        for table in tables().into_iter().filter(|t| t.legacy) {
            let sql = projection(&table);
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
        expected.insert("pair_codes".into(), RowHasher::default().finish());
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
        self.ensure_no_clients().await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audit::SourceAudit, core};

    fn wrong_timestamp(t: &Table) -> String {
        t.postgres_select().replace(
            "(extract(epoch FROM source_start)*1000000)::bigint::text",
            "((extract(epoch FROM source_start)*1000000)::bigint+1)::text",
        )
    }
    fn swapped_columns(t: &Table) -> String {
        t.postgres_select().replace(
            "source_key::text,revision::text",
            "revision::text,source_key::text",
        )
    }
    fn truncated_text(t: &Table) -> String {
        t.postgres_select()
            .replace("source_key::text", "left(source_key,2)::text")
    }
    fn changed_json(t: &Table) -> String {
        if t.name == "normalized" {
            t.postgres_select().replace("payload::text", "'{}'::text")
        } else {
            t.postgres_select()
        }
    }
    fn inverted_bool(t: &Table) -> String {
        t.postgres_select()
            .replace("published::int::text", "(NOT published)::int::text")
    }
    fn dropped_row(t: &Table) -> String {
        t.postgres_select().replace(
            "FROM normalized ORDER BY",
            "FROM normalized WHERE false ORDER BY",
        )
    }

    #[tokio::test]
    #[ignore = "requires a scratch Postgres service"]
    async fn independent_audit_refuses_wrong_reader_even_when_row_hashes_agree() -> Result<()> {
        crate::test_support::with_database(|url| async move {
            let mut pg = PgConnection::connect(&url).await?;
            FROZEN.run(&mut pg).await?;
            sqlx::query("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope) VALUES(41,'synthetic','synthetic:test','source-key','revision-value','synthetic.point','2026-01-02T04:00:00.123456Z','2026-01-02T04:00:00Z','normal','{\"z\":1e24,\"a\":-0.0}')").execute(&mut pg).await?;
            sqlx::query("INSERT INTO normalized(event_id,normalizer_version,payload,search_text) VALUES(41,'synthetic','{\"z\":1e-8,\"a\":2}','syntheticneedle')").execute(&mut pg).await?;
            sqlx::query("INSERT INTO blobs(sha256,bytes,published) VALUES($1,2,true)").bind("a".repeat(64)).execute(&mut pg).await?;
            pg.close().await?;
            let mut source = Snapshot::open(&url).await?;
            let audit = SourceAudit::read(&mut source).await?;
            type Projection = fn(&Table) -> String;
            let variants: [(Projection, &str); 7] = [(Table::postgres_select,""),(wrong_timestamp,"events.source_start"),(swapped_columns,"events.source_key"),(truncated_text,"events.source_key"),(changed_json,"normalized.payload"),(inverted_bool,"blobs.published"),(dropped_row,"normalized row count")];
            for (projection, expected_error) in variants {
                let dir = tempfile::tempdir()?;
                let store = Store::create(dir.path()).await?;
                let hashes = source.load_with(&store, projection).await?;
                core::verify(&store,&hashes).await?;
                let result = audit.verify(&store).await;
                store.pool.close().await;
                if expected_error.is_empty() { result?; }
                else { ensure!(result.is_err_and(|error| error.to_string().contains(expected_error)),"independent audit missed wrong reader: {expected_error}"); }
            }
            source.close().await?;
            Ok(())
        }).await
    }
}
