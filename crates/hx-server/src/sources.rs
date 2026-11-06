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

pub(crate) async fn owner(store: &Store, headers: &HeaderMap) -> ApiResult<()> {
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
pub(crate) async fn scrubber(store: &Store, extra: Option<&str>) -> ApiResult<Scrubber> {
    let mut secrets = keys(store)?.redaction_keys().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "source key storage unavailable".into(),
        )
    })?;
    if let Some(key) = extra {
        secrets.extend(hx_sources::oauth::redaction_values(key));
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
async fn safe(store: &Store, value: Value) -> ApiResult<Json<Value>> {
    Ok(Json(scrubber(store, None).await?.scrub_json(&value)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) key: Option<String>,
    #[serde(default)]
    probe: Option<hx_sources::request::Request>,
    #[serde(default)]
    probe_json: Option<String>,
    #[serde(default)]
    cli_home: Option<String>,
    #[serde(default = "history_default")]
    pub(crate) history_days: u8,
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
    let mut request = body(request)?;
    request.key = request.key.filter(|key| !key.is_empty());
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
    if let Some(json) = request.probe_json.filter(|s| !s.is_empty()) {
        request.probe =
            Some(serde_json::from_str(&json).map_err(|_| bad("Invalid connection check"))?);
    }
    if let Some(home) = request.cli_home.filter(|h| !h.is_empty()) {
        if !std::path::Path::new(&home).is_absolute()
            || home.chars().any(char::is_control)
            || scrubber.contains_secret(&home)
        {
            return Err(bad("CLI HOME must be an absolute non-secret path"));
        }
        if let Some(hx_sources::request::Request::Cli { env, .. }) = &mut request.probe {
            env.insert("HOME".into(), home.clone());
        }
        request.description.push_str(&format!(
            " Use HOME {home} for all CLI requests (env HOME in recipes)."
        ));
    }
    if let Some(probe) = &request.probe {
        probe
            .authorize(&request.description)
            .map_err(|_| bad("Connection check destination is not authorized"))?;
        if scrubber.scrub_json(&serde_json::to_value(probe).map_err(anyhow::Error::from)?)
            != serde_json::to_value(probe).map_err(anyhow::Error::from)?
        {
            return Err(bad("Connection check must not contain credentials"));
        }
    }
    let error = if let Some(probe) = &request.probe {
        check_connection(&store, probe, request.key.as_deref())
            .await
            .err()
            .map(|e| e.to_string())
    } else {
        None
    };
    let source = store
        .connect_source_checked(
            &request.name,
            &request.description,
            request.history_days,
            request.key.as_deref(),
            request.probe.as_ref(),
            error.as_deref(),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({"id":source.id,"connector_instance":source.connector_instance,"enabled":true,"status":if error.is_some() {"needs_reconnect"} else {"setting_up"}}),
        ),
    ))
}
async fn check_connection(
    store: &Store,
    request: &hx_sources::request::Request,
    key: Option<&str>,
) -> Result<(), hx_sources::request::RequestError> {
    let work = tempfile::tempdir().map_err(|_| hx_sources::request::RequestError::HostIo)?;
    let host =
        hx_sources::host::HostRunner::from_env(store.data_dir.clone(), work.path().to_owned());
    request
        .execute(
            hx_sources::request::Placeholders { key, cursor: None },
            std::time::Duration::from_secs(30),
            &host,
        )
        .await
        .map(|_| ())
}

pub(crate) async fn reconnect_source(store: &Store, id: &str, key: Option<&str>) -> ApiResult<()> {
    let source = source(store, id).await?;
    if !source.enabled {
        return Err(bad("Turn this source on before reconnecting"));
    }
    if let Some(probe) = store.source_check(id).await? {
        probe
            .authorize(&source.description)
            .map_err(|_| bad("Connection check destination is not authorized"))?;
        let saved = keys(store)?
            .read(id)
            .map_err(|_| bad("Source key unavailable"))?;
        if let Err(error) = check_connection(store, &probe, key.or(saved.as_deref())).await {
            store
                .record_source_pull(id, chrono::Utc::now(), true, Some(&error.to_string()))
                .await?;
            return Err(bad(
                "Login still unavailable. Sign in to the CLI on the server with the configured HOME, then reconnect.",
            ));
        }
    }
    if let Some(key) = key {
        store.patch_source(id, None, Some(key)).await?;
    }
    store.resume_source(id).await?;
    Ok(())
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
        value["oauth"] = json!(
            keys.read(&source.id)
                .ok()
                .flatten()
                .as_deref()
                .and_then(|v| hx_sources::oauth::Credential::parse(v).ok().flatten())
                .is_some()
        );
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
    pub(crate) source: Option<String>,
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
    owner(&store, &headers).await?;
    filter(&store, &query).await?;
    safe(
        &store,
        crate::agent_sql::tables(&store, query.source.as_deref()).await?,
    )
    .await
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
