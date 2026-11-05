//! Synthetic fixtures and time controls for backend-independent state tests.
//! Never connect these helpers to an owner database.
use super::*;

pub struct Fixture {
    pub store: Store,
    admin: PgPool,
    db: String,
    _dir: tempfile::TempDir,
}
impl Fixture {
    pub async fn new() -> Self {
        Self::through_migration(i64::MAX).await
    }
    /// Exercise an upgrade with synthetic data written by the old schema.
    pub async fn through_migration(version: i64) -> Self {
        let url = std::env::var("HX_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://hx:hx@localhost:55432/hx".into());
        let base = url.rsplit_once('/').expect("test database URL").0;
        let admin = PgPool::connect(&format!("{base}/postgres")).await.unwrap();
        let db = format!(
            "hx_sc_{}",
            ulid::Ulid::generate().to_string().to_lowercase()
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db}")))
            .execute(&admin)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!("{base}/{db}"), dir.path())
            .await
            .unwrap();
        let mut migrations = sqlx::migrate!("./migrations");
        migrations.migrations = migrations
            .iter()
            .filter(|m| m.version <= version)
            .cloned()
            .collect::<Vec<_>>()
            .into();
        migrations.run(&store.pool).await.unwrap();
        Self {
            store,
            admin,
            db,
            _dir: dir,
        }
    }
    pub async fn close(self) {
        self.store.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {}", self.db)))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}
#[derive(Debug, sqlx::FromRow)]
pub struct WindowState {
    pub generation: i64,
    pub dirty: bool,
    pub canonical_revision: Option<i64>,
}
#[derive(Debug, sqlx::FromRow)]
pub struct JobState {
    pub id: i64,
    pub output_key: String,
    pub payload: Value,
    pub kind: String,
    pub state: String,
    pub attempts: i32,
    pub revivals: i32,
}
impl Store {
    pub async fn sweep_reservations(
        &self,
    ) -> Result<Vec<(chrono::NaiveDate, DateTime<Utc>, bool)>> {
        Ok(
            sqlx::query_as("SELECT day,start_at,started FROM daily_sweeps ORDER BY day")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn window_state(&self, w: Interval) -> Result<WindowState> {
        Ok(sqlx::query_as(
            "SELECT generation,dirty,canonical_revision FROM windows WHERE start_at=$1",
        )
        .bind(w.start)
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn job_states(&self) -> Result<Vec<JobState>> {
        Ok(sqlx::query_as(
            "SELECT id,output_key,payload,kind,state,attempts,revivals FROM jobs ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn request_work(&self, kind: &str, w: Interval, generation: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        request_window_job(&mut tx, kind, w, generation).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn request_interpretation(
        &self,
        w: Interval,
        generation: i64,
        revision24: bool,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        request_interpret_job(&mut tx, w, generation, revision24, Utc::now()).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn make_jobs_due(&self, state: &str, kind: Option<&str>) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET available_at=now() WHERE state=$1 AND ($2::text IS NULL OR kind=$2)",
        )
        .bind(state)
        .bind(kind)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn settle_inputs(&self) -> Result<()> {
        sqlx::query("UPDATE windows SET input_changed_at=now()-interval '1 hour'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn change_input(&self, w: Interval) -> Result<()> {
        sqlx::query("UPDATE windows SET generation=generation+1,dirty=true WHERE start_at=$1")
            .bind(w.start)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn expire_lease(&self, id: i64, max_attempts: Option<i32>) -> Result<()> {
        sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second',max_attempts=coalesce($2,max_attempts) WHERE id=$1").bind(id).bind(max_attempts).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn record_count(&self, collection: &str) -> Result<i64> {
        let query = match collection {
            "events" => "SELECT count(*) FROM events",
            "jobs" => "SELECT count(*) FROM jobs",
            "quarantine" => "SELECT count(*) FROM quarantine",
            "normalized" => "SELECT count(*) FROM normalized",
            "revisions" => "SELECT count(*) FROM revisions",
            _ => bail!("unknown collection"),
        };
        Ok(sqlx::query_scalar(query).fetch_one(&self.pool).await?)
    }
    pub async fn quarantine_reasons(&self) -> Result<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT reason FROM quarantine ORDER BY event_id")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn watermark(&self, connector: &str) -> Result<(DateTime<Utc>, String)> {
        Ok(sqlx::query_as(
            "SELECT observed_through,sequence::text FROM watermarks WHERE connector_instance=$1",
        )
        .bind(connector)
        .fetch_one(&self.pool)
        .await?)
    }
    /// Move a synthetic reservation and its request past the UTC-day boundary.
    pub async fn expire_sweep_day(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE daily_sweeps SET day=day-1")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE jobs SET payload=jsonb_set(payload,'{sweep_day}',to_jsonb((payload->>'sweep_day')::date-1)) WHERE payload ? 'sweep_day'")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Deterministically defer a worker at the provider guard, after sweep admission.
    pub async fn hold_interpretation_gate(&self, w: Interval) -> Result<ClaimGate<'_>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(current_database() || ':' || current_schema() || ':interpret:' || $1,0))")
            .bind(w.start.to_rfc3339()).execute(&mut *tx).await?;
        Ok(ClaimGate(tx))
    }
    pub async fn hold_claim_gate(&self) -> Result<ClaimGate<'_>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(6847291630281)")
            .execute(&mut *tx)
            .await?;
        Ok(ClaimGate(tx))
    }
    pub async fn claim_is_waiting(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND database=(SELECT oid FROM pg_database WHERE datname=current_database()))").fetch_one(&self.pool).await?)
    }
    pub async fn enqueue_due_probe(&self) -> Result<()> {
        sqlx::query("INSERT INTO jobs(output_key,kind,payload,available_at) VALUES('became-due','test','{}',clock_timestamp())").execute(&self.pool).await?;
        Ok(())
    }
}
pub struct ClaimGate<'a>(sqlx::Transaction<'a, sqlx::Postgres>);
impl ClaimGate<'_> {
    pub async fn release(self) -> Result<()> {
        self.0.commit().await?;
        Ok(())
    }
}

/// Backend-neutral named values keep existing test assertions independent of driver rows.
pub struct Snapshot(std::collections::BTreeMap<String, Value>);
impl Snapshot {
    fn new<const N: usize>(fields: [(&str, Value); N]) -> Self {
        Self(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }
    pub fn get<T: serde::de::DeserializeOwned, I: AsRef<str>>(&self, name: I) -> T {
        serde_json::from_value(self.0[name.as_ref()].clone()).expect("test snapshot field type")
    }
}
impl Store {
    pub async fn clear_window_dirty_flags(&self) -> Result<()> {
        sqlx::query("UPDATE windows SET dirty=false")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn ordered_window_flags(&self) -> Result<Vec<Snapshot>> {
        let rows = sqlx::query("SELECT dirty,generation FROM windows ORDER BY start_at")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Snapshot::new([
                    ("dirty", serde_json::to_value(row.get::<bool, _>("dirty"))?),
                    (
                        "generation",
                        serde_json::to_value(row.get::<i64, _>("generation"))?,
                    ),
                ]))
            })
            .collect()
    }
    pub async fn observed_watermark(&self) -> Result<DateTime<Utc>> {
        Ok(
            sqlx::query_scalar("SELECT observed_through FROM watermarks")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn age_blobs_for_retention(&self, still: &str) -> Result<()> {
        sqlx::query("UPDATE blobs SET created_at=now()-interval '31 days',retention_class=CASE WHEN sha256=$1 THEN 'keep' ELSE 'chunk' END").bind(still).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn expire_all_leases(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn limit_second_job_attempts(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET max_attempts=1 WHERE output_key='two'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn second_job_state(&self) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT state FROM jobs WHERE output_key='two'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn stored_interpretation(&self) -> Result<Value> {
        Ok(sqlx::query_scalar("SELECT interpretation FROM revisions")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn try_mutate_revision(&self) -> Result<()> {
        sqlx::query("UPDATE revisions SET interpretation='{}'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn try_delete_events(&self) -> Result<()> {
        sqlx::query("DELETE FROM events")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn compiled_window(&self) -> Result<Snapshot> {
        let row = sqlx::query("SELECT timeline,compiled_generation,dirty FROM windows")
            .fetch_one(&self.pool)
            .await?;
        Ok(Snapshot::new([
            (
                "timeline",
                serde_json::to_value(row.get::<Value, _>("timeline"))?,
            ),
            (
                "compiled_generation",
                serde_json::to_value(row.get::<i64, _>("compiled_generation"))?,
            ),
            ("dirty", serde_json::to_value(row.get::<bool, _>("dirty"))?),
        ]))
    }
    pub async fn stored_token_hash(&self) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT hash FROM device_tokens")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn revoke_tokens(&self) -> Result<()> {
        sqlx::query("UPDATE device_tokens SET revoked=true")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn window_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM windows")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn core_event_id(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT id FROM events WHERE kind='web.visit'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn extension_event_id(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT id FROM events WHERE kind='extension.note'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn first_event_id(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT min(id) FROM events")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn interpretation_queues(&self) -> Result<Vec<Snapshot>> {
        let rows =
            sqlx::query("SELECT output_key,queue,available_at FROM jobs WHERE kind='interpret'")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Snapshot::new([
                    (
                        "output_key",
                        serde_json::to_value(row.get::<String, _>("output_key"))?,
                    ),
                    (
                        "queue",
                        serde_json::to_value(row.get::<String, _>("queue"))?,
                    ),
                    (
                        "available_at",
                        serde_json::to_value(row.get::<DateTime<Utc>, _>("available_at"))?,
                    ),
                ]))
            })
            .collect()
    }
    pub async fn event_classes(&self) -> Result<Vec<Snapshot>> {
        let rows = sqlx::query("SELECT id,kind,sensitivity FROM events")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Snapshot::new([
                    ("id", serde_json::to_value(row.get::<i64, _>("id"))?),
                    ("kind", serde_json::to_value(row.get::<String, _>("kind"))?),
                    (
                        "sensitivity",
                        serde_json::to_value(row.get::<String, _>("sensitivity"))?,
                    ),
                ]))
            })
            .collect()
    }
    pub async fn served_revision(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT canonical_revision FROM windows")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn pending_free_job_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind IN ('normalize','compile') AND state!='done'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn unfinished_job_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM jobs WHERE state!='done' AND NOT (kind='interpret' AND payload->>'revision24'='true' AND state='ready' AND available_at>now())").fetch_one(&self.pool).await?)
    }
    pub async fn current_revision_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE r.input_generation=w.generation").fetch_one(&self.pool).await?)
    }
    pub async fn served_window_count(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM windows WHERE canonical_revision IS NOT NULL")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn stale_compilation_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM windows WHERE compiled_generation IS DISTINCT FROM generation",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn limit_job_attempts(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE jobs SET max_attempts=1 WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn expire_job_lease(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second' WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn postpone_all_jobs(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET available_at=now()+interval '1 hour'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn limit_pending_interpretation_attempts(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET max_attempts=1 WHERE output_key='pending-interpret'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn pending_interpretation_job(&self) -> Result<Snapshot> {
        let row = sqlx::query(
            "SELECT state,attempts,last_error FROM jobs WHERE output_key='pending-interpret'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Snapshot::new([
            (
                "state",
                serde_json::to_value(row.get::<String, _>("state"))?,
            ),
            (
                "attempts",
                serde_json::to_value(row.get::<i32, _>("attempts"))?,
            ),
            (
                "last_error",
                serde_json::to_value(row.get::<String, _>("last_error"))?,
            ),
        ]))
    }
    pub async fn make_pending_interpretation_due(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET available_at=now() WHERE output_key='pending-interpret'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn pending_interpretation_state(&self) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT state FROM jobs WHERE output_key='pending-interpret'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn canonical_revision_at(&self, start: DateTime<Utc>) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT canonical_revision FROM windows WHERE start_at=$1")
                .bind(start)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn job_error(&self) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT last_error FROM jobs")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn delete_jobs(&self) -> Result<()> {
        sqlx::query("DELETE FROM jobs").execute(&self.pool).await?;
        Ok(())
    }
    pub async fn only_event_id(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT id FROM events")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn remove_request_versions(&self) -> Result<()> {
        sqlx::query("ALTER TABLE jobs DROP COLUMN request_version, DROP COLUMN leased_version")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn apply_coalesced_work_migration(&self) -> Result<()> {
        sqlx::raw_sql(include_str!("../migrations/0004_coalesced_work.sql"))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn live_compile_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind='compile' AND state IN ('ready','running')",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn ordered_event_ids(&self) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar("SELECT id FROM events ORDER BY id")
            .fetch_all(&self.pool)
            .await?)
    }
    pub async fn event_ids(&self) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar("SELECT id FROM events")
            .fetch_all(&self.pool)
            .await?)
    }
    pub async fn derived_ocr_records(&self) -> Result<Vec<Snapshot>> {
        let rows = sqlx::query(
            "SELECT id,source_event_id,sensitivity,envelope FROM ocr_derived ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Snapshot::new([
                    ("id", serde_json::to_value(row.get::<i64, _>("id"))?),
                    (
                        "source_event_id",
                        serde_json::to_value(row.get::<i64, _>("source_event_id"))?,
                    ),
                    (
                        "sensitivity",
                        serde_json::to_value(row.get::<String, _>("sensitivity"))?,
                    ),
                    (
                        "envelope",
                        serde_json::to_value(row.get::<Value, _>("envelope"))?,
                    ),
                ]))
            })
            .collect()
    }
    pub async fn sampled_ocr_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM ocr_jobs WHERE status='sampled' AND result IS NULL AND sampled_from IS NOT NULL").fetch_one(&self.pool).await?)
    }
    pub async fn ocr_evidence_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM ocr_evidence")
            .fetch_one(&self.pool)
            .await?)
    }
    pub async fn make_broken_ocr_due(&self) -> Result<()> {
        sqlx::query("UPDATE ocr_jobs SET available_at=now() WHERE engine_version='broken/v1'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn broken_ocr_status(&self) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT status FROM ocr_jobs WHERE engine_version='broken/v1'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn complete_ocr_count(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM ocr_jobs WHERE status='complete'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn dirty_window_count(&self, starts: Vec<DateTime<Utc>>) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM windows WHERE dirty AND start_at=ANY($1)")
                .bind(starts)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn fx_digest_interpretation(&self) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT interpretation FROM revisions WHERE output_key='synthetic-fx-digest'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn fx_digest_agent_run(&self) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT agent_run FROM revisions WHERE output_key='synthetic-fx-digest'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn attention_revision(&self) -> Result<Snapshot> {
        let row = sqlx::query(
            "SELECT interpretation,agent_run FROM revisions WHERE output_key='synthetic-attention'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Snapshot::new([
            (
                "interpretation",
                serde_json::to_value(row.get::<Value, _>("interpretation"))?,
            ),
            (
                "agent_run",
                serde_json::to_value(row.get::<Value, _>("agent_run"))?,
            ),
        ]))
    }
    pub async fn fx_late_interpretation(&self) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT interpretation FROM revisions WHERE output_key='synthetic-fx-late'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn revision_contents(&self, id: i64) -> Result<Snapshot> {
        let row = sqlx::query("SELECT interpretation,agent_run FROM revisions WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(Snapshot::new([
            (
                "interpretation",
                serde_json::to_value(row.get::<Value, _>("interpretation"))?,
            ),
            (
                "agent_run",
                serde_json::to_value(row.get::<Value, _>("agent_run"))?,
            ),
        ]))
    }
    pub async fn required_canonical_revision_at(&self, start: DateTime<Utc>) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT canonical_revision FROM windows WHERE start_at=$1")
                .bind(start)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn advance_window_generation(&self, start: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE windows SET generation=generation+1 WHERE start_at=$1")
            .bind(start)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn stale_agent_revision_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM revisions WHERE output_key='synthetic-stale-agent'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn fx_repaired_interpretation(&self) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT interpretation FROM revisions WHERE output_key='synthetic-fx-repaired'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn fx_empty_repair_interpretation(&self) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT interpretation FROM revisions WHERE output_key='synthetic-fx-empty-repair'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn fx_merge_repair_interpretation(&self) -> Result<Value> {
        Ok(sqlx::query_scalar(
            "SELECT interpretation FROM revisions WHERE output_key='synthetic-fx-merge-repair'",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn finish_free_jobs(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET state='done' WHERE kind IN ('compile','normalize')")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn settle_test_inputs(&self) -> Result<()> {
        sqlx::query("UPDATE windows SET input_changed_at=now()-interval '1 hour'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn ready_interpretation_count(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar(
                "SELECT count(*) FROM jobs WHERE kind='interpret' AND state='ready'",
            )
            .fetch_one(&self.pool)
            .await?,
        )
    }
    pub async fn interpretation_generations(&self) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT (payload->>'generation')::bigint FROM jobs WHERE kind='interpret'",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn make_ready_interpretations_due(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET available_at=now() WHERE kind='interpret' AND state='ready'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn interpretation_attempts(&self) -> Result<i32> {
        Ok(
            sqlx::query_scalar("SELECT attempts FROM jobs WHERE kind='interpret'")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn lease_versions(&self, id: i64) -> Result<Snapshot> {
        let row = sqlx::query("SELECT state,request_version,leased_version FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(Snapshot::new([
            (
                "state",
                serde_json::to_value(row.get::<String, _>("state"))?,
            ),
            (
                "request_version",
                serde_json::to_value(row.get::<i64, _>("request_version"))?,
            ),
            (
                "leased_version",
                serde_json::to_value(row.get::<i64, _>("leased_version"))?,
            ),
        ]))
    }
    pub async fn remove_interpretation_slots_schema(&self) -> Result<()> {
        sqlx::raw_sql("DROP TRIGGER window_input_changed ON windows; DROP FUNCTION track_window_input(); ALTER TABLE windows DROP COLUMN input_changed_at; DROP INDEX revisions_input_identity;").execute(&self.pool).await?;
        Ok(())
    }
    pub async fn live_job_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE state IN ('ready','running','failed')",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn ready_output_keys(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT output_key FROM jobs WHERE state='ready' ORDER BY output_key",
        )
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn interpretation_requests(&self) -> Result<Vec<Snapshot>> {
        let rows =
            sqlx::query("SELECT payload,available_at,attempts FROM jobs WHERE kind='interpret'")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Snapshot::new([
                    (
                        "payload",
                        serde_json::to_value(row.get::<Value, _>("payload"))?,
                    ),
                    (
                        "available_at",
                        serde_json::to_value(row.get::<DateTime<Utc>, _>("available_at"))?,
                    ),
                    (
                        "attempts",
                        serde_json::to_value(row.get::<i32, _>("attempts"))?,
                    ),
                ]))
            })
            .collect()
    }
    pub async fn ready_interpretation_generations(&self) -> Result<Vec<i64>> {
        Ok(sqlx::query_scalar("SELECT (payload->>'generation')::bigint FROM jobs WHERE kind='interpret' AND state='ready'").fetch_all(&self.pool).await?)
    }
    pub async fn make_all_interpretations_due(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET available_at=now() WHERE kind='interpret'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn published_generation(&self, start: DateTime<Utc>) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT r.input_generation FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE w.start_at=$1").bind(start).fetch_one(&self.pool).await?)
    }
    pub async fn fail_legacy_job(&self, key: String) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET state='failed',attempts=5,available_at=now() WHERE output_key=$1",
        )
        .bind(key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn reset_revision24(&self, start: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE windows SET revision24_finished=false WHERE start_at=$1")
            .bind(start)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn finish_all_jobs(&self) -> Result<()> {
        sqlx::query("UPDATE jobs SET state='done'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
impl StoreTransaction {
    pub async fn database_now(&mut self) -> Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT now()")
            .fetch_one(&mut *self.0)
            .await?)
    }
    pub async fn apply_interpret_slots_migration(&mut self) -> Result<()> {
        sqlx::raw_sql(include_str!("../migrations/0007_interpret_slots.sql"))
            .execute(&mut *self.0)
            .await?;
        Ok(())
    }
}
