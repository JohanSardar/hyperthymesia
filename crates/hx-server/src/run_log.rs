//! AI run records for the Runs and Agent pages: state, duration, cost and a
//! content-free log. Never stores provider output, prompts or raw errors; a
//! failure is kept as a short `problem` code the app explains in plain words.
use anyhow::Result;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use hx_store::{Store, Ts};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

const LOG_LIMIT: i64 = 300;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Usage {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Whether fx reported any generation at all.
    pub reported: bool,
}
impl Usage {
    fn add(&mut self, other: Usage) {
        self.cost_usd += other.cost_usd;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.reported |= other.reported;
    }
}

/// Sums the generation facts fx writes to `usage.jsonl` in a run's private profile.
pub fn read_usage(profile: &Path) -> Usage {
    let Ok(text) = std::fs::read_to_string(profile.join("usage.jsonl")) else {
        return Usage::default();
    };
    let mut facts: HashMap<String, Value> = HashMap::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["kind"] == "generation" {
            let fact = value["fact"].clone();
            let id = fact["id"].as_str().unwrap_or_default().to_owned();
            facts.insert(id, fact);
        }
    }
    let mut usage = Usage::default();
    for fact in facts.values() {
        usage.reported = true;
        usage.cost_usd += fact["total_cost"].as_f64().unwrap_or(0.0).max(0.0);
        usage.input_tokens += fact["input_tokens"].as_i64().unwrap_or(0).max(0);
        usage.output_tokens += fact["output_tokens"].as_i64().unwrap_or(0).max(0);
    }
    usage
}

pub struct Start<'a> {
    pub kind: &'a str,
    pub job_id: Option<i64>,
    pub window_start: Option<DateTime<Utc>>,
    pub label: Option<&'a str>,
    pub provider: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
}

#[derive(Clone)]
pub struct RunLog(Arc<Inner>);
struct Inner {
    store: Store,
    id: i64,
    usage: Mutex<Usage>,
}
impl RunLog {
    pub async fn start(store: &Store, start: Start<'_>) -> Result<Self> {
        let provider = if start.provider == "gateway" {
            "vercel"
        } else {
            start.provider
        };
        let id: i64 = hx_store::retry_busy("start run record", |_| async {
            let mut tx = store.begin_write().await?;
            let id = sqlx::query_scalar("INSERT INTO agent_runs(kind,job_id,window_start,label,provider,model,effort,started_at,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'running') RETURNING id")
                .bind(start.kind).bind(start.job_id).bind(start.window_start.map(Ts::from)).bind(start.label)
                .bind(provider).bind(start.model).bind(Some(start.effort).filter(|e| !e.is_empty()))
                .bind(Ts::now()).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            Ok(id)
        })
        .await?;
        Ok(Self(Arc::new(Inner {
            store: store.clone(),
            id,
            usage: Mutex::new(Usage::default()),
        })))
    }
    pub fn id(&self) -> i64 {
        self.0.id
    }
    pub fn add_usage(&self, usage: Usage) {
        self.0.usage.lock().expect("run usage").add(usage);
    }
    fn usage(&self) -> Usage {
        *self.0.usage.lock().expect("run usage")
    }
    /// One plain, content-free line. A failed write never fails the run.
    pub async fn note(&self, text: impl Into<String>) {
        let text = text.into();
        let usage = self.usage();
        let result = hx_store::retry_busy("note run step", |_| async {
            let mut tx = self.0.store.begin_write().await?;
            sqlx::query("UPDATE agent_runs SET log=CASE WHEN json_array_length(log)<$5 THEN json_insert(log,'$[#]',json_object('at',$2,'text',$3)) ELSE log END,cost_usd=CASE WHEN $4 THEN $6 ELSE cost_usd END,input_tokens=CASE WHEN $4 THEN $7 ELSE input_tokens END,output_tokens=CASE WHEN $4 THEN $8 ELSE output_tokens END WHERE id=$1")
                .bind(self.0.id).bind(Ts::now()).bind(&text).bind(usage.reported).bind(LOG_LIMIT)
                .bind(usage.cost_usd).bind(usage.input_tokens).bind(usage.output_tokens)
                .execute(&mut *tx).await?;
            tx.commit().await
        })
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "run step not recorded");
        }
    }
    pub async fn finish(&self, state: &str, problem: Option<&str>, revision: Option<i64>) {
        let usage = self.usage();
        let result = hx_store::retry_busy("finish run record", |_| async {
            let mut tx = self.0.store.begin_write().await?;
            sqlx::query("UPDATE agent_runs SET state=$2,problem=$3,revision_id=$4,finished_at=$5,cost_usd=CASE WHEN $6 THEN $7 ELSE cost_usd END,input_tokens=CASE WHEN $6 THEN $8 ELSE input_tokens END,output_tokens=CASE WHEN $6 THEN $9 ELSE output_tokens END WHERE id=$1")
                .bind(self.0.id).bind(state).bind(problem).bind(revision).bind(Ts::now())
                .bind(usage.reported).bind(usage.cost_usd).bind(usage.input_tokens).bind(usage.output_tokens)
                .execute(&mut *tx).await?;
            tx.commit().await
        })
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "run result not recorded");
        }
    }
    /// Records the outcome of a failed run from its error.
    pub async fn fail(&self, error: &anyhow::Error) {
        let problem = problem(error);
        let state = match problem {
            "superseded" => "stopped",
            "usage_limit" | "spending_limit" | "busy" | "waiting" => "waiting",
            _ => "failed",
        };
        self.note(match state {
            "stopped" => "Stopped: new activity came in, so this window runs again",
            "waiting" => "Paused; it tries again by itself",
            _ => "Stopped with a problem",
        })
        .await;
        self.finish(state, Some(problem), None).await;
    }
}

/// Short, stable codes for what went wrong. The app maps each to words and a fix.
pub fn problem(error: &anyhow::Error) -> &'static str {
    let text = format!("{error:#}").to_lowercase();
    // A missing or rejected login can arrive as a retry later; it still needs a sign-in.
    if text.contains("login rejected")
        || text.contains("credential unavailable")
        || text.contains("fx login missing")
        || text.contains("run fx login")
    {
        return "signin";
    }
    if let Some(deferred) = error.downcast_ref::<hx_jobs::Deferred>() {
        return match deferred {
            hx_jobs::Deferred::RateLimited => "usage_limit",
            hx_jobs::Deferred::InputChanged => "superseded",
            hx_jobs::Deferred::SpendingLimit(_) => "spending_limit",
            hx_jobs::Deferred::ProviderBusy => "busy",
            hx_jobs::Deferred::NormalizationPending | hx_jobs::Deferred::Until(_) => "waiting",
        };
    }
    if text.contains("ai_gateway_api_key") || text.contains("key rejected") {
        "key"
    } else if text.contains("choose a provider and model") {
        "no_model"
    } else if text.contains("model rejected") || text.contains("model mismatch") {
        "model"
    } else if text.contains("timeout") {
        "timeout"
    } else if text.contains("budget exhausted") {
        "too_long"
    } else if text.contains("invalid fx") || text.contains("final output") {
        "output"
    } else if text.contains("changed during run") || text.contains("input generation changed") {
        "superseded"
    } else {
        "other"
    }
}

/// What a provider's failure text points at, without keeping the text.
pub fn provider_failure(text: &str, provider: &str) -> Option<&'static str> {
    let text = text.to_lowercase();
    let auth = [
        "401",
        "403",
        "unauthorized",
        "unauthenticated",
        "invalid_grant",
        "refresh token",
        "refresh_token",
        "token expired",
        "token_expired",
        "expired token",
        "not logged in",
        "not signed in",
        "log in",
        "login",
        "sign in",
        "reauth",
        "authentication",
        "invalid api key",
        "invalid_api_key",
    ];
    let model = [
        "model_not_found",
        "model not found",
        "unknown model",
        "unsupported model",
        "does not exist",
        "not a valid model",
        "invalid model",
    ];
    if model.iter().any(|m| text.contains(m)) {
        Some("model rejected")
    } else if auth.iter().any(|a| text.contains(a)) {
        Some(if provider == "gateway" || provider == "vercel" {
            "key rejected"
        } else {
            "login rejected"
        })
    } else {
        None
    }
}

/// Start of the current calendar month, UTC.
pub fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}
pub fn next_month(now: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

pub async fn month_spend(store: &Store, now: DateTime<Utc>) -> Result<f64> {
    Ok(sqlx::query_scalar::<_, f64>(
        "SELECT COALESCE(sum(cost_usd),0.0) FROM agent_runs WHERE started_at>=$1",
    )
    .bind(Ts::from(month_start(now)))
    .fetch_one(&store.pool)
    .await?)
}

/// Refuses a new paid run once this month's spend reached the owner's limit.
pub async fn check_spending(store: &Store) -> Result<()> {
    let Some(limit) = crate::preferences::load(store).await?.spend_limit_usd else {
        return Ok(());
    };
    let now = Utc::now();
    if month_spend(store, now).await? >= limit {
        return Err(hx_jobs::Deferred::SpendingLimit(next_month(now)).into());
    }
    Ok(())
}

/// Keeps one "held by the spending limit" row per window instead of one per retry.
pub async fn note_spending_hold(
    store: &Store,
    job_id: Option<i64>,
    window: hx_schema::Interval,
    provider: &str,
    model: &str,
) -> Result<()> {
    let held: Option<i64> = sqlx::query_scalar("SELECT id FROM agent_runs WHERE kind='pass' AND window_start=$1 AND state='waiting' AND problem='spending_limit' ORDER BY id DESC LIMIT 1")
        .bind(Ts::from(window.start)).fetch_optional(&store.pool).await?;
    if held.is_some() {
        return Ok(());
    }
    let run = RunLog::start(
        store,
        Start {
            kind: "pass",
            job_id,
            window_start: Some(window.start),
            label: None,
            provider,
            model,
            effort: "",
        },
    )
    .await?;
    run.note("Held: this month's spending limit is reached")
        .await;
    run.finish("waiting", Some("spending_limit"), None).await;
    Ok(())
}

/// The job behind an output key such as `job:12:01J…` or `rerun:12:01J…`.
pub fn job_id(output_key: &str) -> Option<i64> {
    let mut parts = output_key.split(':');
    match parts.next()? {
        "job" | "rerun" => parts.next()?.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_counts_each_generation_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("usage.jsonl"),
            concat!(
                r#"{"schema_version":1,"kind":"pending","id":"a"}"#, "\n",
                r#"{"schema_version":1,"kind":"generation","fact":{"id":"a","input_tokens":100,"output_tokens":10,"total_cost":0.25}}"#, "\n",
                r#"{"schema_version":1,"kind":"generation","fact":{"id":"a","input_tokens":100,"output_tokens":10,"total_cost":0.25}}"#, "\n",
                r#"{"schema_version":1,"kind":"generation","fact":{"id":"b","input_tokens":5,"output_tokens":1,"total_cost":0.5}}"#, "\n",
                "not json\n"
            ),
        )
        .unwrap();
        let usage = read_usage(dir.path());
        assert_eq!(usage.input_tokens, 105);
        assert_eq!(usage.output_tokens, 11);
        assert!((usage.cost_usd - 0.75).abs() < 1e-9);
        assert!(usage.reported);
        assert!(!read_usage(&dir.path().join("missing")).reported);
    }

    #[test]
    fn provider_failures_name_the_fix() {
        assert_eq!(
            provider_failure("HTTP 401 Unauthorized", "codex"),
            Some("login rejected")
        );
        assert_eq!(
            provider_failure("refresh_token expired", "grok"),
            Some("login rejected")
        );
        assert_eq!(
            provider_failure("invalid api key", "gateway"),
            Some("key rejected")
        );
        assert_eq!(
            provider_failure("model_not_found", "codex"),
            Some("model rejected")
        );
        assert_eq!(provider_failure("connection reset", "codex"), None);
        let error = anyhow::anyhow!("fx codex login rejected; sign in again");
        assert_eq!(problem(&error), "signin");
    }
}
