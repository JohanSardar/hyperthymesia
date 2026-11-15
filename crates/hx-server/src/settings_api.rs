//! Owner-only JSON for model setup, Settings and Runs. Never returns provider output.
use crate::{
    ApiError, ApiResult, auth, bad,
    preferences::{self, Model, Preferences},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use hx_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn router() -> Router<Store> {
    Router::new()
        .route("/v1/settings", get(settings).put(update))
        .route("/v1/models", get(models))
        .route("/v1/model/check", post(check))
        .route("/v1/settings/gateway-key", put(key).delete(remove_key))
        .route("/v1/settings/tokens", get(tokens).post(create_token))
        .route(
            "/v1/settings/tokens/{id}",
            axum::routing::delete(revoke_token),
        )
        .route("/v1/runs", get(runs))
        .layer(DefaultBodyLimit::max(16 * 1024))
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
async fn owner(store: &Store, headers: &HeaderMap) -> ApiResult<()> {
    let token = auth(store, headers, "read_raw").await?;
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
async fn owner_write(store: &Store, headers: &HeaderMap) -> ApiResult<()> {
    owner(store, headers).await?;
    // Cookie authentication also works across same-site ports. Require the
    // app's origin for mutations; non-browser clients send a bearer token.
    if !headers.contains_key(header::AUTHORIZATION) {
        crate::connections::same_origin(headers)?;
    }
    Ok(())
}
fn body<T>(request: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    request
        .map(|Json(v)| v)
        .map_err(|_| bad("invalid request body"))
}
async fn settings(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let preferences = preferences::load(&store).await?;
    let bytes: i64 = sqlx::query_scalar("SELECT COALESCE(sum(bytes),0) FROM blobs WHERE published")
        .fetch_one(&store.pool)
        .await?;
    Ok(Json(
        json!({"settings":preferences,"gateway_key_configured":preferences::gateway_key(&store)?.is_some(),
        "storage":{"state_dir":store.data_dir,"published_blob_bytes":bytes,"automatic_gc":false},
        "api_keys":store.list_tokens().await?}),
    ))
}
async fn update(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Preferences>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner_write(&store, &headers).await?;
    let value = body(request)?;
    if let Some(model) = &value.model {
        validate_model(model)?;
    }
    if !(1..=3650).contains(&value.chunk_retention_days)
        || !["system", "light", "dark"].contains(&value.appearance.as_str())
    {
        return Err(bad("invalid storage or appearance preference"));
    }
    preferences::save(&store, &value).await?;
    Ok(Json(json!({"settings":value})))
}
fn validate_model(model: &Model) -> ApiResult<()> {
    if !["vercel", "codex", "grok"].contains(&model.provider.as_str())
        || model.model.trim().is_empty()
        || model.model.len() > 256
        || model.model.starts_with('-')
        || model.model.chars().any(char::is_control)
    {
        return Err(bad("choose a supported provider and model"));
    }
    if let Some(effort) = &model.effort
        && (effort.is_empty()
            || effort.len() > 64
            || effort.starts_with('-')
            || !effort
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
    {
        return Err(bad("invalid effort"));
    }
    Ok(())
}
#[derive(Deserialize)]
struct Catalog {
    provider: Option<String>,
}
async fn models(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<Catalog>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let providers = json!([
        {"id":"vercel","name":"Vercel AI Gateway","authentication":["api_key"]},
        {"id":"codex","name":"Codex","authentication":["signin"]},
        {"id":"grok","name":"Grok","authentication":["signin"]}
    ]);
    let Some(provider) = query.provider else {
        return Ok(Json(json!({"providers":providers,"selected":null})));
    };
    if !["vercel", "codex", "grok"].contains(&provider.as_str()) {
        return Err(bad("unsupported provider"));
    }
    match crate::fx::inspect(&store, &provider, None).await {
        Ok(value) => Ok(Json(
            json!({"provider":provider,"models":crate::sources::scrubber(&store, preferences::gateway_key(&store)?.as_deref()).await?.scrub_json(&value["models"]),"error":null,"allows_custom_model":true}),
        )),
        Err(_) => Ok(Json(
            json!({"provider":provider,"models":[],"error":"catalog_unavailable","message":"Check provider authentication and try again. You can also enter a model ID.","allows_custom_model":true}),
        )),
    }
}
async fn check(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner_write(&store, &headers).await?;
    let model = preferences::load(&store)
        .await?
        .model
        .ok_or_else(|| bad("choose a provider and model first"))?;
    // An explicit check makes one small provider request. Never expose arbitrary
    // provider output (which can contain credentials or diagnostics).
    let ok = crate::fx::inspect(&store, &model.provider, Some(&model))
        .await
        .is_ok();
    Ok(Json(
        json!({"ok":ok,"message":if ok {"The selected model responded."} else {"The model check failed. Check authentication, model ID and effort, then try again."}}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    key: String,
}
async fn key(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Key>, JsonRejection>,
) -> ApiResult<StatusCode> {
    owner_write(&store, &headers).await?;
    let value = body(request)?;
    if value.key.len() < 4
        || value.key.len() > 8192
        || value.key.chars().any(char::is_control)
        || value.key.trim() != value.key
    {
        return Err(bad("invalid API key"));
    }
    hx_sources::keys::KeyStore::new(&store.data_dir)
        .map_err(anyhow::Error::from)?
        .write("ai-gateway", &value.key)
        .map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn remove_key(State(store): State<Store>, headers: HeaderMap) -> ApiResult<StatusCode> {
    owner_write(&store, &headers).await?;
    hx_sources::keys::KeyStore::new(&store.data_dir)
        .map_err(anyhow::Error::from)?
        .remove("ai-gateway")
        .map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn tokens(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    Ok(Json(json!({"tokens":store.list_tokens().await?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewToken {
    name: String,
    prefixes: Vec<String>,
    scopes: Vec<String>,
}
async fn create_token(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<NewToken>, JsonRejection>,
) -> ApiResult<Response> {
    owner_write(&store, &headers).await?;
    let value = body(request)?;
    if value.name.trim().is_empty()
        || value.name.len() > 128
        || value.prefixes.is_empty()
        || value.prefixes.len() > 100
        || value.prefixes.iter().any(|p| p.is_empty() || p.len() > 256)
        || value.scopes.is_empty()
        || value
            .scopes
            .iter()
            .any(|s| !["ingest", "read_raw", "read_l3"].contains(&s.as_str()))
    {
        return Err(bad("invalid token name, prefixes or scopes"));
    }
    let secret = store
        .create_token(&value.name, value.prefixes, value.scopes)
        .await?;
    // The newly issued /v1 token is shown once, as in the CLI. Provider API keys
    // are never returned by any endpoint.
    Ok((
        StatusCode::CREATED,
        Json(json!({"token":secret,"id":hx_store::hash(secret.as_bytes())})),
    )
        .into_response())
}
async fn revoke_token(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    owner_write(&store, &headers).await?;
    if !hx_store::valid_hash(&id) {
        return Err(bad("invalid token ID"));
    }
    Ok(if store.revoke_token(&id).await? {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
}
#[derive(Deserialize)]
struct Page {
    before: Option<i64>,
    pass_before: Option<i64>,
    pull_before: Option<i64>,
    limit: Option<u32>,
}
async fn runs(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let limit = page.limit.unwrap_or(50).clamp(1, 100);
    let before = page.before.unwrap_or(i64::MAX);
    let passes: Vec<Value> = sqlx::query_scalar("SELECT json_object('id',j.id,'kind',j.kind,'state',j.state,'window_start',j.window_start,'generation',j.generation,'attempts',j.attempts,'available_at',j.available_at,'lease_until',j.lease_until,'has_error',j.last_error IS NOT NULL,'revision_id',w.canonical_revision) FROM jobs j LEFT JOIN windows w ON w.start_at=j.window_start WHERE j.kind='interpret' AND j.id<$1 ORDER BY j.id DESC LIMIT $2")
        .bind(page.pass_before.unwrap_or(before)).bind(limit).fetch_all(&store.pool).await?;
    let pulls: Vec<Value> = sqlx::query_scalar("SELECT json_object('id',id,'recipe_id',recipe_id,'source_id',source_id,'started_at',started_at,'finished_at',finished_at,'status',status,'accepted',accepted,'duplicates',duplicates,'rejected',rejected) FROM pull_history WHERE id<$1 ORDER BY id DESC LIMIT $2")
        .bind(page.pull_before.unwrap_or(before)).bind(limit).fetch_all(&store.pool).await?;
    Ok(Json(
        json!({"passes":passes,"pulls":pulls,"limit":limit,"timestamps":"UTC microseconds","next_pass_before":passes.last().map(|v| &v["id"]),"next_pull_before":pulls.last().map(|v| &v["id"])}),
    ))
}
