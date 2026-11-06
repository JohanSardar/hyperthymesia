//! Owner-only source configuration. No broker route can reach these handlers.
use crate::{ApiError, ApiResult, auth, bad};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use hx_sources::{keys::KeyStore, scrub::Scrubber};
use hx_store::{Store, sources::Source};
use serde::Deserialize;
use serde_json::{Value, json};

async fn owner(store: &Store, headers: &HeaderMap) -> ApiResult<()> {
    let token = auth(store, headers, "read_raw").await?;
    if !token.allows("source:") {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "source: access required".into(),
        ));
    }
    Ok(())
}
fn id(value: &str) -> ApiResult<()> {
    if value.len() != 26
        || value != value.to_ascii_lowercase()
        || value.parse::<ulid::Ulid>().is_err()
    {
        return Err(bad("invalid source or configuration id"));
    }
    Ok(())
}
fn body<T>(request: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    request.map(|Json(value)| value).map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body too large".into(),
            )
        } else {
            bad("invalid request body")
        }
    })
}
fn missing() -> ApiError {
    ApiError(
        StatusCode::NOT_FOUND,
        "source or configuration not found".into(),
    )
}
async fn source(store: &Store, value: &str) -> ApiResult<Source> {
    id(value)?;
    store.source(value).await?.ok_or_else(missing)
}
fn keys(store: &Store) -> ApiResult<KeyStore> {
    KeyStore::new(&store.data_dir).map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "source key storage unavailable".into(),
        )
    })
}
async fn scrubber(store: &Store, extra: Option<&str>) -> ApiResult<Scrubber> {
    let keys = keys(store)?;
    let mut secrets = vec![];
    for source in store.sources().await? {
        if let Some(key) = keys.read(&source.id).map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "source key storage unavailable".into(),
            )
        })? {
            secrets.push(key);
        }
    }
    if let Some(key) = extra {
        secrets.push(key.to_owned());
    }
    Scrubber::new(secrets).map_err(|error| match error {
        hx_sources::scrub::ScrubError::KeyTooShort => {
            bad("source key must contain at least 4 bytes")
        }
        hx_sources::scrub::ScrubError::Pattern => ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "source secret filter unavailable".into(),
        ),
    })
}
fn scrub_value(value: &mut Value, scrubber: &Scrubber) {
    match value {
        Value::String(text) => *text = scrubber.scrub(text),
        Value::Array(items) => {
            for item in items {
                scrub_value(item, scrubber);
            }
        }
        Value::Object(items) => {
            *items = std::mem::take(items)
                .into_iter()
                .map(|(key, mut value)| {
                    scrub_value(&mut value, scrubber);
                    (scrubber.scrub(&key), value)
                })
                .collect();
        }
        _ => {}
    }
}
async fn safe(store: &Store, mut value: Value) -> ApiResult<Json<Value>> {
    scrub_value(&mut value, &scrubber(store, None).await?);
    Ok(Json(value))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    name: String,
    description: String,
    key: Option<String>,
    #[serde(default = "history_default")]
    history_days: u8,
}
fn history_default() -> u8 {
    1
}
pub async fn connect(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Connect>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    owner(&store, &headers).await?;
    let request = body(request)?;
    if request.name.trim().is_empty()
        || request.name.len() > 200
        || request.name.chars().any(char::is_control)
        || request.description.len() > 16_384
        || request.history_days > 30
    {
        return Err(bad("invalid name, description or history_days (0..=30)"));
    }
    let scrubber = scrubber(&store, request.key.as_deref()).await?;
    if scrubber.contains_secret(&request.description) || scrubber.contains_secret(&request.name) {
        return Err(bad("name and description must not contain source keys"));
    }
    let source = store
        .connect_source_with_key(
            &request.name,
            &request.description,
            request.history_days,
            request.key.as_deref(),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":source.id,"connector_instance":source.connector_instance,"enabled":true,"status":"setting_up"}),
        ),
    ))
}
pub async fn list(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let keys = keys(&store)?;
    let mut values = vec![];
    for source in store.sources().await? {
        let has_key = keys
            .read(&source.id)
            .map_err(|_| {
                ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "source key storage unavailable".into(),
                )
            })?
            .is_some();
        let mut value = serde_json::to_value(&source).map_err(anyhow::Error::from)?;
        value["has_key"] = json!(has_key);
        values.push(value);
    }
    safe(&store, json!(values)).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcePatch {
    enabled: Option<bool>,
    key: Option<String>,
}
pub async fn patch_source(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(value): Path<String>,
    request: Result<Json<SourcePatch>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let source = source(&store, &value).await?;
    let request = body(request)?;
    if request.enabled.is_none() && request.key.is_none() {
        return Err(bad("enabled or key required"));
    }
    if let Some(key) = &request.key {
        let scrubber = scrubber(&store, Some(key)).await?;
        // A rotation must not turn already saved public configuration into a
        // copy of the new key. No plaintext key ever enters SQLite.
        let recipes = store.recipes(Some(&value)).await?;
        let text = json!({"name":source.name,"description":source.description,"recipes":recipes.iter().map(|r|&r.recipe.0).collect::<Vec<_>>(),"addons":store.addons(Some(&value)).await?}).to_string();
        if scrubber.contains_secret(&text) {
            return Err(bad("saved configuration must not contain source keys"));
        }
    }
    store
        .patch_source(&value, request.enabled, request.key.as_deref())
        .await?;
    Ok(Json(
        json!({"id":value,"enabled":request.enabled.unwrap_or(source.enabled)}),
    ))
}
pub async fn setup(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(value): Path<String>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    owner(&store, &headers).await?;
    source(&store, &value).await?;
    let generation = store.request_source_setup(&value).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id":value,"setup_generation":generation,"status":"setting_up"})),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    source: Option<String>,
}
async fn filter(store: &Store, filter: &Filter) -> ApiResult<()> {
    if let Some(value) = &filter.source {
        source(store, value).await?;
    }
    Ok(())
}
pub async fn recipes(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<Filter>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    filter(&store, &query).await?;
    let mut values = vec![];
    for recipe in store.recipes(query.source.as_deref()).await? {
        let state = store.recipe_state(&recipe.id).await?;
        values.push(json!({"id":recipe.id,"source_id":recipe.source_id,"name":recipe.name,"recipe":recipe.recipe.0,"enabled":recipe.enabled,"owner_off":recipe.owner_off,"next_run_at":state.next_run_at,"last_ok_at":state.last_ok_at,"last_error":state.last_error}));
    }
    safe(&store, json!(values)).await
}
pub async fn addons(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<Filter>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    filter(&store, &query).await?;
    safe(&store, json!(store.addons(query.source.as_deref()).await?)).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    enabled: bool,
}
pub async fn recipe_switch(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(value): Path<String>,
    request: Result<Json<Switch>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    id(&value)?;
    let request = body(request)?;
    if !store.recipe_exists(&value).await? {
        return Err(missing());
    }
    store.set_recipe_enabled(&value, request.enabled).await?;
    Ok(Json(json!({"id":value,"enabled":request.enabled})))
}
pub async fn addon_switch(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(value): Path<String>,
    request: Result<Json<Switch>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    id(&value)?;
    let request = body(request)?;
    if !store.addon_exists(&value).await? {
        return Err(missing());
    }
    store.set_addon_enabled(&value, request.enabled).await?;
    Ok(Json(json!({"id":value,"enabled":request.enabled})))
}
pub async fn agent_tables(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<Filter>,
) -> ApiResult<Json<Value>> {
    use sqlx::Connection;
    owner(&store, &headers).await?;
    filter(&store, &query).await?;
    let sources = store.sources().await?;
    let prefixes: Vec<_> = sources
        .iter()
        .filter(|s| query.source.as_ref().is_none_or(|id| id == &s.id))
        .map(|s| format!("s_{}_", s.id))
        .collect();
    let mut conn = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(store.data_dir.join("agent.sqlite"))
            .read_only(true),
    )
    .await?;
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .fetch_all(&mut conn)
            .await?;
    let mut tables = vec![];
    for name in names
        .into_iter()
        .filter(|name| prefixes.iter().any(|prefix| name.starts_with(prefix)))
    {
        // Quote a database identifier, never interpolate it as SQL syntax.
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM \"{}\"",
            name.replace('"', "\"\"")
        )))
        .fetch_one(&mut conn)
        .await?;
        tables.push(json!({"name":name,"row_count":count}));
    }
    safe(&store, json!(tables)).await
}

pub(crate) fn router() -> axum::Router<Store> {
    use axum::{
        Router,
        extract::DefaultBodyLimit,
        routing::{get, patch, post},
    };
    Router::new()
        .route("/v1/sources", get(list).post(connect))
        .route("/v1/sources/{id}", patch(patch_source))
        .route("/v1/sources/{id}/setup", post(setup))
        .route("/v1/recipes", get(recipes))
        .route("/v1/recipes/{id}", patch(recipe_switch))
        .route("/v1/addons", get(addons))
        .route("/v1/addons/{id}", patch(addon_switch))
        .route("/v1/agent-tables", get(agent_tables))
        .layer(DefaultBodyLimit::max(32 * 1024))
}
