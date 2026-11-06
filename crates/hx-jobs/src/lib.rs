//! Leased SQLite queue. A lease owner is also the completion fencing token.
use anyhow::Result;
use chrono::{DateTime, Utc};
use hx_store::{Ts, WriteTransaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: Value,
    pub owner: String,
    pub attempts: i32,
}
pub async fn enqueue(
    store: &hx_store::Store,
    key: &str,
    kind: &str,
    payload: Value,
    backfill: bool,
    at: DateTime<Utc>,
) -> Result<()> {
    let mut tx = store.transaction().await?;
    tx.enqueue(key, kind, payload, backfill, at).await?;
    tx.commit().await
}
pub async fn claim(store: &hx_store::Store, lease_seconds: i64) -> Result<Option<Job>> {
    let owner = ulid::Ulid::generate().to_string();
    let mut tx = store.begin_write().await?;
    // Read the clock after acquiring the single writer, including any busy wait.
    let now = Ts::now();
    sqlx::query("UPDATE jobs SET attempts=0 WHERE kind<>'interpret' AND state='running' AND lease_until<$1 AND request_version>leased_version")
        .bind(now).execute(&mut *tx).await?;
    sqlx::query("UPDATE jobs SET state='failed',available_at=$1+300000000,lease_until=NULL,lease_owner=NULL,last_error=coalesce(last_error,'worker lease expired; attempts exhausted') WHERE attempts>=max_attempts AND (state='ready' OR (state='running' AND lease_until<$1))")
        .bind(now).execute(&mut *tx).await?;
    let exhausted: Vec<Value> =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind='normalize' AND state='failed'")
            .fetch_all(&mut *tx)
            .await?;
    quarantine_exhausted(&mut tx, &exhausted, now).await?;
    let r = sqlx::query("WITH candidate AS (SELECT j.id FROM jobs j WHERE j.attempts<j.max_attempts AND j.available_at<=$3 AND (j.state='ready' OR (j.state='running' AND j.lease_until<$3)) AND NOT EXISTS (SELECT 1 FROM jobs live WHERE live.id<>j.id AND live.state='running' AND live.lease_until>$3 AND live.window_start=j.window_start) ORDER BY CASE j.queue WHEN 'interactive' THEN 0 ELSE 1 END, CASE j.kind WHEN 'normalize' THEN 0 ELSE 1 END,j.available_at,j.id LIMIT 1) UPDATE jobs AS j SET state='running',lease_owner=$1,lease_until=$2,attempts=attempts+1,leased_version=request_version FROM candidate c WHERE j.id=c.id RETURNING id,kind,payload,attempts")
        .bind(&owner).bind(Ts(now.0+lease_seconds*1_000_000)).bind(now).fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    Ok(r.map(|r| Job {
        id: r.get("id"),
        kind: r.get("kind"),
        payload: r.get("payload"),
        owner,
        attempts: r.get("attempts"),
    }))
}
pub async fn heartbeat(store: &hx_store::Store, job: &Job, seconds: i64) -> Result<bool> {
    hx_store::retry_busy("heartbeat", |waited| {
        heartbeat_attempt(store, job, seconds, waited)
    })
    .await
}
async fn heartbeat_attempt(
    store: &hx_store::Store,
    job: &Job,
    seconds: i64,
    waited: bool,
) -> Result<bool> {
    let mut tx = store.begin_write().await?;
    let now = Ts::now();
    let changed = sqlx::query("UPDATE jobs SET lease_until=$3 WHERE id=$1 AND lease_owner=$2 AND state='running' AND (lease_until>$4 OR $5)")
        .bind(job.id).bind(&job.owner).bind(Ts(now.0+seconds*1_000_000)).bind(now).bind(waited).execute(&mut *tx).await?.rows_affected()==1;
    tx.commit().await?;
    Ok(changed)
}
pub async fn finish(store: &hx_store::Store, job: &Job, error: Option<&str>) -> Result<bool> {
    hx_store::retry_busy("finish job", |waited| {
        finish_attempt(store, job, error, waited)
    })
    .await
}
async fn finish_attempt(
    store: &hx_store::Store,
    job: &Job,
    error: Option<&str>,
    waited: bool,
) -> Result<bool> {
    let mut tx = store.begin_write().await?;
    let now = Ts::now();
    let state: Option<String> = sqlx::query_scalar("UPDATE jobs SET state=CASE WHEN $3 IS NULL THEN CASE WHEN request_version>leased_version THEN 'ready' ELSE 'done' END WHEN request_version>leased_version THEN 'ready' WHEN attempts>=max_attempts THEN 'failed' ELSE 'ready' END,attempts=CASE WHEN request_version>leased_version AND kind<>'interpret' THEN 0 ELSE attempts END,last_error=$3,available_at=CASE WHEN request_version>leased_version THEN $4 WHEN $3 IS NOT NULL AND attempts>=max_attempts THEN $4+300000000 ELSE $4+5000000 END,lease_until=NULL,lease_owner=NULL WHERE id=$1 AND lease_owner=$2 AND state='running' AND (lease_until>$4 OR $5) RETURNING state")
        .bind(job.id).bind(&job.owner).bind(error).bind(now).bind(waited).fetch_optional(&mut *tx).await?;
    if state.as_deref() == Some("failed") && job.kind == "normalize" {
        quarantine_exhausted(&mut tx, std::slice::from_ref(&job.payload), now).await?;
    }
    tx.commit().await?;
    Ok(state.is_some())
}

// Exhaustion is a durable classification outcome, including pre-handler crashes.
async fn quarantine_exhausted(
    tx: &mut WriteTransaction,
    payloads: &[Value],
    now: Ts,
) -> Result<()> {
    for payload in payloads {
        let window = payload
            .get("window")
            .map(|v| serde_json::from_value::<hx_schema::Interval>(v.clone()))
            .transpose()?;
        sqlx::query("INSERT INTO quarantine(event_id,reason,created_at) SELECT e.id,'normalization attempts exhausted',$4 FROM events e WHERE (e.id=$1 OR (e.source_start>=$2 AND e.source_start<$3)) AND NOT EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id) ON CONFLICT(event_id) DO NOTHING")
            .bind(payload["event_id"].as_i64()).bind(window.map(|w| Ts::from(w.start))).bind(window.map(|w| Ts::from(w.end))).bind(now).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Paid budgets survive generation changes and get at most three revivals.
pub async fn revive_window_jobs(store: &hx_store::Store) -> Result<()> {
    let mut tx = store.begin_write().await?;
    let now = Ts::now();
    sqlx::query("UPDATE jobs SET state='ready',attempts=0,revivals=revivals+1,request_version=request_version+1,lease_until=NULL,lease_owner=NULL WHERE id IN (SELECT id FROM jobs WHERE state='failed' AND revivals<3 AND json_type(payload,'$.sweep_day') IS NULL AND kind IN ('compile','interpret') AND available_at<=$1 ORDER BY available_at,id LIMIT 1000)")
        .bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub trait Handler: Send + Sync {
    fn handle(&self, job: &Job) -> impl std::future::Future<Output = Result<()>> + Send;
}
pub async fn run_one<H: Handler>(store: &hx_store::Store, handler: &H) -> Result<bool> {
    run_one_interval(store, handler, std::time::Duration::from_secs(30)).await
}
#[cfg(feature = "test-support")]
pub async fn run_one_with_test_heartbeat<H: Handler>(
    store: &hx_store::Store,
    handler: &H,
    interval: std::time::Duration,
) -> Result<bool> {
    run_one_interval(store, handler, interval).await
}
async fn run_one_interval<H: Handler>(
    store: &hx_store::Store,
    handler: &H,
    interval: std::time::Duration,
) -> Result<bool> {
    let Some(job) = claim(store, 120).await? else {
        return Ok(false);
    };
    let future = handler.handle(&job);
    tokio::pin!(future);
    let result = loop {
        tokio::select! {
            result=&mut future=>break result,
            _=tokio::time::sleep(interval)=>{
                let renewal = heartbeat(store, &job, 120);
                tokio::pin!(renewal);
                // Keep draining provider I/O and polling its deadlines while a
                // renewal waits for the writer. Retain completed output until
                // this renewal finishes, so finish sees a fresh lease too.
                tokio::select! {
                    owned = &mut renewal => if !owned? { return Ok(true) },
                    result = &mut future => {
                        if !renewal.await? { return Ok(true) }
                        break result;
                    }
                }
            }
        }
    };
    if let Err(error) = &result
        && let Some(deferred) = error.downcast_ref::<Deferred>()
    {
        defer(store, &job, deferred).await?;
        return Ok(true);
    }
    let description = result.as_ref().err().map(safe_error);
    finish(store, &job, description.as_deref()).await?;
    result?;
    Ok(true)
}

/// Pre-provider readiness refunds attempts; rate limits after launch spend them.
#[derive(Debug)]
pub enum Deferred {
    NormalizationPending,
    InputChanged,
    RateLimited,
    ProviderBusy,
    Until(DateTime<Utc>),
}
impl std::fmt::Display for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NormalizationPending => "normalization pending",
            Self::InputChanged => "input generation changed",
            Self::RateLimited => "subscription rate limited",
            Self::ProviderBusy => "subscription credential busy",
            Self::Until(_) => "window input settling",
        })
    }
}
impl std::error::Error for Deferred {}
pub async fn defer(store: &hx_store::Store, job: &Job, reason: &Deferred) -> Result<bool> {
    hx_store::retry_busy("defer job", |_| defer_attempt(store, job, reason)).await
}
async fn defer_attempt(store: &hx_store::Store, job: &Job, reason: &Deferred) -> Result<bool> {
    let mut tx = store.begin_write().await?;
    let now = Ts::now();
    let at = match reason {
        Deferred::Until(at) => Ts::from(*at),
        Deferred::RateLimited => Ts(now.0 + 1_800_000_000),
        Deferred::ProviderBusy => Ts(now.0 + 5_000_000),
        _ => Ts(now.0 + 1_000_000),
    };
    let changed = sqlx::query("UPDATE jobs SET state=CASE WHEN $5 AND attempts>=max_attempts THEN 'failed' ELSE 'ready' END,attempts=CASE WHEN $5 THEN attempts WHEN request_version>leased_version AND kind<>'interpret' THEN 0 ELSE max(attempts-1,0) END,last_error=$3,available_at=$4,lease_until=NULL,lease_owner=NULL WHERE id=$1 AND lease_owner=$2 AND state='running' AND lease_until>$6")
        .bind(job.id).bind(&job.owner).bind(reason.to_string()).bind(at).bind(matches!(reason,Deferred::RateLimited)).bind(now).execute(&mut *tx).await?.rows_affected()==1;
    tx.commit().await?;
    Ok(changed)
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
