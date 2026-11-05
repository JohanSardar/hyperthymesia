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
        store.migrate().await.unwrap();
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
        Ok(
            sqlx::query_as(
                "SELECT id,output_key,payload,kind,state,attempts FROM jobs ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await?,
        )
    }
    pub async fn request_work(&self, kind: &str, w: Interval, generation: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        request_window_job(&mut tx, kind, w, generation).await?;
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
