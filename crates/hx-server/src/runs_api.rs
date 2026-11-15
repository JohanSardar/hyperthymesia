//! Owner-only JSON for the Agent and Runs pages: AI runs with their cost and
//! content-free log, re-runs, provider accounts and sign-in. See
//! docs/reference/app-api.md.
use crate::{ApiError, ApiResult, bad};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use chrono::Utc;
use hx_store::{Store, Ts};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn router() -> Router<Store> {
    Router::new()
        .route("/v1/agent-runs", get(list))
        .route("/v1/agent-runs/{id}", get(detail))
        .route("/v1/agent-runs/{id}/rerun", post(rerun))
        .route("/v1/providers", get(providers))
        .route(
            "/v1/providers/{id}/signin",
            get(signin_status).post(signin_start).delete(signin_cancel),
        )
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}

pub(crate) async fn owner(store: &Store, headers: &HeaderMap) -> ApiResult<()> {
    let token = crate::auth(store, headers, "read_raw").await?;
    if !["source:", "macos-agent:", "macos-media:", "companion:"]
        .iter()
        .all(|prefix| token.allows(prefix))
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "owner token required".into(),
        ));
    }
    Ok(())
}
pub(crate) async fn owner_write(store: &Store, headers: &HeaderMap) -> ApiResult<()> {
    owner(store, headers).await?;
    if !headers.contains_key(header::AUTHORIZATION) {
        crate::connections::same_origin(headers)?;
    }
    Ok(())
}

macro_rules! run_fields {
    () => {
        "json_object('id',r.id,'kind',r.kind,'job_id',r.job_id,'window_start',r.window_start,'label',r.label,'provider',r.provider,'model',r.model,'effort',r.effort,'started_at',r.started_at,'finished_at',r.finished_at,'state',r.state,'problem',r.problem,'cost_usd',r.cost_usd,'input_tokens',r.input_tokens,'output_tokens',r.output_tokens,'revision_id',r.revision_id,'steps',json_array_length(r.log))"
    };
}

#[derive(Deserialize)]
struct Page {
    before: Option<i64>,
    limit: Option<u32>,
    kind: Option<String>,
}
async fn list(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let checks = page.kind.as_deref() == Some("check");
    let runs: Vec<Value> = sqlx::query_scalar(concat!(
        "SELECT ",
        run_fields!(),
        " FROM agent_runs r WHERE r.id<$1 AND (r.kind='check')=$2 ORDER BY r.id DESC LIMIT $3"
    ))
    .bind(page.before.unwrap_or(i64::MAX))
    .bind(checks)
    .bind(limit)
    .fetch_all(&store.pool)
    .await?;
    // What runs next: first passes at the end of each window, next-day catch-ups
    // a day later, and the daily sweep.
    let upcoming: Vec<Value> = sqlx::query_scalar("SELECT json_object('job_id',id,'window_start',window_start,'available_at',available_at,'catch_up',json_extract(payload,'$.revision24')=1 OR json_type(payload,'$.sweep_day') IS NOT NULL,'held',last_error='spending limit reached','rerun',json_extract(payload,'$.rerun')=1) FROM jobs WHERE kind='interpret' AND state IN ('ready','running') ORDER BY available_at LIMIT 8")
        .fetch_all(&store.pool).await?;
    let preferences = crate::preferences::load(&store).await?;
    Ok(Json(json!({
        "runs": runs,
        "next_before": runs.last().map(|v| &v["id"]),
        "upcoming": upcoming,
        "month_spend_usd": crate::run_log::month_spend(&store, Utc::now()).await?,
        "spend_limit_usd": preferences.spend_limit_usd,
        "timestamps": "UTC microseconds",
    })))
}

async fn detail(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let run: Option<Value> = sqlx::query_scalar(concat!(
        "SELECT json_set(",
        run_fields!(),
        ",'$.log',json(r.log),'$.job',(SELECT json_object('state',j.state,'attempts',j.attempts,'max_attempts',j.max_attempts,'available_at',j.available_at,'source_id',json_extract(j.payload,'$.source')) FROM jobs j WHERE j.id=r.job_id),'$.window_end',r.window_start+14400000000) FROM agent_runs r WHERE r.id=$1"
    ))
    .bind(id)
    .fetch_optional(&store.pool)
    .await?;
    run.map(Json)
        .ok_or(ApiError(StatusCode::NOT_FOUND, "no such run".into()))
}

/// Runs a window's pass again now. Spends a model run only when something
/// changed since the last account (inputs, model or settings).
async fn rerun(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    owner_write(&store, &headers).await?;
    let row: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT kind,window_start FROM agent_runs WHERE id=$1")
            .bind(id)
            .fetch_optional(&store.pool)
            .await?;
    let Some((kind, Some(start))) = row else {
        return Err(ApiError(StatusCode::NOT_FOUND, "no such pass".into()));
    };
    if kind != "pass" {
        return Err(bad("only passes can be re-run here"));
    }
    if crate::preferences::load(&store).await?.model.is_none() {
        return Err(bad("choose a provider and model first"));
    }
    let start = chrono::DateTime::<Utc>::from(Ts(start));
    let window = hx_store::window_at(start);
    let generation = store.generation(window).await?;
    let key = format!(
        "interpret-rerun:{}:{}",
        window.start,
        ulid::Ulid::generate()
    );
    hx_jobs::enqueue(
        &store,
        &key,
        "interpret",
        json!({"window":window,"generation":generation,"rerun":true}),
        false,
        Utc::now(),
    )
    .await?;
    Ok(Json(
        json!({"queued":true,"window_start":Ts::from(window.start).0}),
    ))
}

/// Lets held passes go now: after a raised or removed spending limit, or a new
/// model (which also retries passes waiting on errors).
pub(crate) async fn release_held(store: &Store, raised: bool) -> anyhow::Result<()> {
    let mut tx = store.begin_write().await?;
    sqlx::query("UPDATE jobs SET available_at=$1 WHERE kind='interpret' AND state='ready' AND available_at>$1 AND (last_error='spending limit reached' OR ($2=0 AND last_error IS NOT NULL))")
        .bind(Ts::now()).bind(raised).execute(&mut *tx).await?;
    tx.commit().await
}

async fn providers(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let chosen = crate::preferences::load(&store).await?.model;
    let key = crate::preferences::gateway_key(&store)?.is_some();
    let mut list = vec![];
    for id in ["codex", "grok", "vercel"] {
        let last = |kind: &'static str| {
            let store = store.clone();
            async move {
                sqlx::query_scalar::<_, Value>("SELECT json_object('id',id,'at',coalesce(finished_at,started_at),'state',state,'problem',problem,'model',model) FROM agent_runs WHERE provider=$1 AND (kind='check')=$2 AND state IN ('done','failed') ORDER BY id DESC LIMIT 1")
                    .bind(id).bind(kind == "check").fetch_optional(&store.pool).await
            }
        };
        let signed_in = if id == "vercel" {
            key
        } else {
            crate::fx::login_present(id)
        };
        list.push(json!({
            "id": id,
            "method": if id == "vercel" {"key"} else {"signin"},
            "signed_in": signed_in,
            "in_use": chosen.as_ref().is_some_and(|m| m.provider == id),
            "last_check": last("check").await?,
            "last_run": last("run").await?,
            "signin": if id == "vercel" { Value::Null } else { crate::signin::status(id)["state"].clone() },
        }));
    }
    Ok(Json(json!({"providers": list})))
}

fn provider_id(id: &str) -> ApiResult<&str> {
    if crate::signin::supported(id) {
        Ok(id)
    } else {
        Err(bad("this provider signs in with a key"))
    }
}
async fn signin_status(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let id = provider_id(&id)?.to_owned();
    Ok(Json(
        tokio::task::spawn_blocking(move || crate::signin::status(&id))
            .await
            .map_err(anyhow::Error::from)?,
    ))
}
async fn signin_start(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    owner_write(&store, &headers).await?;
    let id = provider_id(&id)?.to_owned();
    if !crate::signin::supported(&id) {
        return Err(bad("this provider uses a key, not a sign-in"));
    }
    Ok(Json(
        tokio::task::spawn_blocking(move || crate::signin::start(&id))
            .await
            .map_err(anyhow::Error::from)?
            .map_err(|error| {
                tracing::warn!(error = %format!("{error:#}"), "provider sign-in did not start");
                ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "fx, the tool that runs your AI, didn't start here. Reinstalling Hyperthymesia puts it back.".into(),
                )
            })?,
    ))
}
async fn signin_cancel(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    owner_write(&store, &headers).await?;
    let id = provider_id(&id)?.to_owned();
    tokio::task::spawn_blocking(move || crate::signin::cancel(&id))
        .await
        .map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
