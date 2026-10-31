//! Leased Postgres queue. A lease owner is also the completion fencing token.
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: Value,
    pub owner: String,
    pub attempts: i32,
}
pub async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    kind: &str,
    payload: Value,
    backfill: bool,
    at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("INSERT INTO jobs(output_key,kind,payload,queue,available_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(output_key) DO NOTHING")
        .bind(key).bind(kind).bind(payload).bind(if backfill {"backfill"} else {"interactive"}).bind(at).execute(&mut **tx).await?;
    Ok(())
}
pub async fn claim(pool: &PgPool, lease_seconds: i64) -> Result<Option<Job>> {
    let owner = ulid::Ulid::generate().to_string();
    let mut tx = pool.begin().await?;
    // Serialize only the short claim transaction. Without this lock, concurrent
    // snapshots could both see no live job for a window and claim distinct rows.
    sqlx::query("SELECT pg_advisory_xact_lock(6847291630281)")
        .execute(&mut *tx)
        .await?;
    // A crashed worker must not exhaust the attempt budget of newer input.
    sqlx::query("UPDATE jobs SET attempts=0 WHERE state='running' AND lease_until<statement_timestamp() AND request_version>leased_version").execute(&mut *tx).await?;
    sqlx::query("UPDATE jobs SET state='failed',available_at=statement_timestamp()+interval '5 minutes',lease_until=NULL,lease_owner=NULL,last_error=coalesce(last_error,'worker lease expired; attempts exhausted') WHERE attempts>=max_attempts AND (state='ready' OR (state='running' AND lease_until<statement_timestamp()))")
        .execute(&mut *tx).await?;
    // Also repair normalization slots exhausted before this recovery logic existed.
    let exhausted: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE kind='normalize' AND state='failed' FOR UPDATE",
    )
    .fetch_all(&mut *tx)
    .await?;
    quarantine_exhausted(&mut tx, &exhausted).await?;
    let r=sqlx::query("WITH candidate AS (SELECT j.id FROM jobs j WHERE j.attempts<j.max_attempts AND j.available_at<=statement_timestamp() AND (j.state='ready' OR (j.state='running' AND j.lease_until<statement_timestamp())) AND NOT EXISTS (SELECT 1 FROM jobs live WHERE live.id<>j.id AND live.state='running' AND live.lease_until>statement_timestamp() AND (live.payload->'window'->>'start')::timestamptz=(j.payload->'window'->>'start')::timestamptz) ORDER BY CASE j.queue WHEN 'interactive' THEN 0 ELSE 1 END, CASE j.kind WHEN 'normalize' THEN 0 ELSE 1 END,j.available_at,j.id FOR UPDATE OF j SKIP LOCKED LIMIT 1) UPDATE jobs j SET state='running',lease_owner=$1,lease_until=statement_timestamp()+make_interval(secs=>$2::double precision),attempts=attempts+1,leased_version=request_version FROM candidate c WHERE j.id=c.id RETURNING j.id,j.kind,j.payload,j.attempts")
        .bind(&owner).bind(lease_seconds as f64).fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(r.map(|r| Job {
        id: r.get("id"),
        kind: r.get("kind"),
        payload: r.get("payload"),
        owner,
        attempts: r.get("attempts"),
    }))
}
pub async fn heartbeat(pool: &PgPool, job: &Job, seconds: i64) -> Result<bool> {
    Ok(sqlx::query("UPDATE jobs SET lease_until=now()+make_interval(secs=>$3::double precision) WHERE id=$1 AND lease_owner=$2 AND state='running' AND lease_until>now()")
        .bind(job.id).bind(&job.owner).bind(seconds as f64).execute(pool).await?.rows_affected()==1)
}
pub async fn finish(pool: &PgPool, job: &Job, error: Option<&str>) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let state: Option<String> = sqlx::query_scalar("UPDATE jobs SET state=CASE WHEN $3::text IS NULL THEN CASE WHEN request_version>leased_version THEN 'ready' ELSE 'done' END WHEN request_version>leased_version THEN 'ready' WHEN attempts>=max_attempts THEN 'failed' ELSE 'ready' END,attempts=CASE WHEN request_version>leased_version THEN 0 ELSE attempts END,last_error=$3,available_at=CASE WHEN request_version>leased_version THEN now() WHEN $3::text IS NOT NULL AND attempts>=max_attempts THEN now()+interval '5 minutes' ELSE now()+interval '5 seconds' END,lease_until=NULL,lease_owner=NULL WHERE id=$1 AND lease_owner=$2 AND state='running' AND lease_until>now() RETURNING state")
        .bind(job.id).bind(&job.owner).bind(error).fetch_optional(&mut *tx).await?;
    if state.as_deref() == Some("failed") && job.kind == "normalize" {
        quarantine_exhausted(&mut tx, std::slice::from_ref(&job.payload)).await?;
    }
    tx.commit().await?;
    Ok(state.is_some())
}

// Exhaustion is a durable classification outcome, including pre-handler crashes.
// Keep L0 immutable and persist a bounded reason rather than an arbitrary error.
async fn quarantine_exhausted(
    tx: &mut Transaction<'_, Postgres>,
    payloads: &[Value],
) -> Result<()> {
    if payloads.is_empty() {
        return Ok(());
    }
    sqlx::query("INSERT INTO quarantine(event_id,reason) SELECT e.id,'normalization attempts exhausted' FROM events e WHERE EXISTS (SELECT 1 FROM jsonb_array_elements($1) p WHERE (p->>'event_id')::bigint=e.id OR (e.source_start >= (p->'window'->>'start')::timestamptz AND e.source_start < (p->'window'->>'end')::timestamptz)) AND NOT EXISTS (SELECT 1 FROM normalized n WHERE n.event_id=e.id) ON CONFLICT(event_id) DO NOTHING")
        .bind(serde_json::json!(payloads)).execute(&mut **tx).await?;
    Ok(())
}

/// Each failed window job gets another bounded attempt round after five minutes.
/// Existing output keys keep recovery idempotent and the error history bounded.
pub async fn revive_window_jobs(pool: &PgPool) -> Result<()> {
    sqlx::query("UPDATE jobs SET state='ready',attempts=0,request_version=request_version+1,lease_until=NULL,lease_owner=NULL WHERE id IN (SELECT id FROM jobs WHERE state='failed' AND kind IN ('compile','interpret') AND available_at<=now() ORDER BY available_at,id LIMIT 1000 FOR UPDATE SKIP LOCKED)")
        .execute(pool).await?;
    Ok(())
}

pub trait Handler: Send + Sync {
    fn handle(&self, job: &Job) -> impl std::future::Future<Output = Result<()>> + Send;
}
pub async fn run_one<H: Handler>(pool: &PgPool, handler: &H) -> Result<bool> {
    let Some(job) = claim(pool, 120).await? else {
        return Ok(false);
    };
    let future = handler.handle(&job);
    tokio::pin!(future);
    let result = loop {
        tokio::select! {
            result=&mut future=>break result,
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>{
                if !heartbeat(pool,&job,120).await? {return Ok(true)}
            }
        }
    };
    if let Err(error) = &result
        && let Some(deferred) = error.downcast_ref::<Deferred>()
    {
        defer(pool, &job, deferred).await?;
        return Ok(true);
    }
    let description = result.as_ref().err().map(safe_error);
    finish(pool, &job, description.as_deref()).await?;
    result?;
    Ok(true)
}

/// Dependency readiness is not a processing failure and does not spend an attempt.
#[derive(Debug)]
pub enum Deferred {
    NormalizationPending,
    InputChanged,
}
impl std::fmt::Display for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NormalizationPending => "normalization pending",
            Self::InputChanged => "input generation changed",
        })
    }
}
impl std::error::Error for Deferred {}
pub async fn defer(pool: &PgPool, job: &Job, reason: &Deferred) -> Result<bool> {
    Ok(sqlx::query("UPDATE jobs SET state='ready',attempts=CASE WHEN request_version>leased_version THEN 0 ELSE greatest(attempts-1,0) END,last_error=$3,available_at=now()+interval '1 second',lease_until=NULL,lease_owner=NULL WHERE id=$1 AND lease_owner=$2 AND state='running' AND lease_until>now()")
        .bind(job.id).bind(&job.owner).bind(reason.to_string()).execute(pool).await?.rows_affected()==1)
}
// Never persist arbitrary Display strings: SQL/provider/serde errors can contain source text.
#[derive(Debug)]
pub enum Failure {
    Provider,
    ModelOutput,
    EvidenceChecks,
    UnsupportedJob,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Provider => "interpretation provider failed",
            Self::ModelOutput => "invalid interpretation model output",
            Self::EvidenceChecks => "interpretation failed evidence checks",
            Self::UnsupportedJob => "unsupported job kind",
        })
    }
}
impl std::error::Error for Failure {}
fn safe_error(error: &anyhow::Error) -> String {
    if let Some(failure) = error.downcast_ref::<Failure>() {
        return failure.to_string();
    }
    if let Some(error) = error.downcast_ref::<sqlx::Error>() {
        if let sqlx::Error::Database(database) = error {
            let code = database.code().unwrap_or_default();
            if code.len() == 5 && code.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return format!("database operation failed (SQLSTATE {code})");
            }
        }
        return "database operation failed".into();
    }
    if error.downcast_ref::<serde_json::Error>().is_some() {
        return "invalid job or derived JSON".into();
    }
    "processing failed (validation, provider, or unsupported job); inspect local logs".into()
}
