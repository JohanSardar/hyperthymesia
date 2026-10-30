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
    sqlx::query("UPDATE jobs SET state='failed' WHERE attempts>=max_attempts AND (state='ready' OR (state='running' AND lease_until<now()))").execute(pool).await?;
    let r=sqlx::query("WITH candidate AS (SELECT id FROM jobs WHERE attempts<max_attempts AND available_at<=now() AND (state='ready' OR (state='running' AND lease_until<now())) ORDER BY CASE queue WHEN 'interactive' THEN 0 ELSE 1 END, CASE kind WHEN 'normalize' THEN 0 ELSE 1 END,available_at,id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE jobs j SET state='running',lease_owner=$1,lease_until=now()+make_interval(secs=>$2::double precision),attempts=attempts+1 FROM candidate c WHERE j.id=c.id RETURNING j.id,j.kind,j.payload,j.attempts")
        .bind(&owner).bind(lease_seconds as f64).fetch_optional(pool).await?;
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
    Ok(sqlx::query("UPDATE jobs SET state=CASE WHEN $3::text IS NULL THEN 'done' WHEN attempts>=max_attempts THEN 'failed' ELSE 'ready' END,last_error=$3,available_at=now()+interval '5 seconds',lease_until=NULL,lease_owner=NULL WHERE id=$1 AND lease_owner=$2 AND state='running' AND lease_until>now()")
        .bind(job.id).bind(&job.owner).bind(error).execute(pool).await?.rows_affected()==1)
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
    finish(
        pool,
        &job,
        result
            .as_ref()
            .err()
            .map(|_| "job failed; inspect local server logs"),
    )
    .await?;
    result?;
    Ok(true)
}
