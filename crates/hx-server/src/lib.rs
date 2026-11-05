//! Authenticated ingest, evidence API, and a local, escaped day view.
pub mod dashboard;
pub mod disclosure;
pub mod fx;
pub mod mcp;
pub mod ocr;
pub mod processing;
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use disclosure::DisclosurePolicy;
use hx_schema::{Envelope, Interval};
use hx_store::{Store, Token, UploadBatch};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug)]
pub struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(_: anyhow::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error".into(),
        )
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(_: sqlx::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database unavailable".into(),
        )
    }
}
type ApiResult<T> = Result<T, ApiError>;
fn bad(reason: impl ToString) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, reason.to_string())
}
pub async fn auth(store: &Store, headers: &HeaderMap, scope: &str) -> ApiResult<Token> {
    let secret = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| {
                    s.split(';')
                        .map(str::trim)
                        .find_map(|s| s.strip_prefix("hx_token="))
                })
        })
        .ok_or(ApiError(
            StatusCode::UNAUTHORIZED,
            "sign in at /login or send a bearer token".into(),
        ))?;
    let token = store
        .authenticate(secret)
        .await?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "invalid token".into()))?;
    if !token.has(scope) && !(scope == "read_l3" && token.has("read_raw")) {
        return Err(ApiError(StatusCode::FORBIDDEN, "scope required".into()));
    }
    Ok(token)
}
pub fn router(store: Store, policy: DisclosurePolicy) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/login", get(dashboard::login).post(dashboard::sign_in))
        .route("/v1/events", post(events))
        .route("/v1/device", get(device))
        .route("/v1/blobs/{sha256}", put(upload_blob).get(download_blob))
        .route("/v1/timeline", get(timeline))
        .route("/v1/search", get(search))
        .route("/v1/evidence/{id}", get(evidence))
        .route("/v1/entities", get(entities))
        .route("/v1/now", get(now))
        .route("/", get(dashboard::today))
        .route("/day/{day}", get(dashboard::day))
        .route("/evidence/{id}", get(dashboard::evidence_page))
        .route(
            "/revision/{revision}/evidence/{id}",
            get(dashboard::revision_evidence),
        )
        .route("/revision/{revision}/run", get(dashboard::run_page))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(Extension(policy))
        .with_state(store)
}
async fn device(
    State(store): State<Store>,
    headers: HeaderMap,
) -> ApiResult<Json<hx_schema::protocol::DeviceResponse>> {
    use hx_schema::protocol::{DeviceResponse, OcrMode};
    auth(&store, &headers, "ingest").await?;
    let available =
        tokio::task::spawn_blocking(|| hx_visual::VisionCliEngine::default().executable().is_ok())
            .await
            .unwrap_or(false);
    Ok(Json(DeviceResponse {
        ocr: if available {
            OcrMode::Server
        } else {
            OcrMode::Device
        },
    }))
}

async fn events(
    State(store): State<Store>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<hx_store::UploadAck>> {
    let token = auth(&store, &headers, "ingest").await?;
    let bytes = match headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
    {
        None | Some("identity") => body.to_vec(),
        Some("zstd") => {
            use std::io::Read;
            let decoder = zstd::stream::read::Decoder::new(body.as_ref()).map_err(bad)?;
            let mut bytes = Vec::new();
            decoder
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(bad)?;
            if bytes.len() > 64 * 1024 * 1024 {
                return Err(bad("decoded batch too large"));
            }
            bytes
        }
        Some(_) => return Err(bad("unsupported content encoding")),
    };
    // Decode each item separately so one malformed envelope cannot discard its siblings.
    let value: Value = serde_json::from_slice(&bytes).map_err(bad)?;
    let items = value
        .get("envelopes")
        .and_then(Value::as_array)
        .ok_or(bad("envelopes array required"))?;
    let batch_id = serde_json::from_value(
        value
            .get("batch_id")
            .cloned()
            .ok_or(bad("batch_id required"))?,
    )
    .map_err(bad)?;
    let mut batch = UploadBatch {
        batch_id,
        envelopes: vec![],
    };
    let mut rejected = vec![];
    for item in items {
        match serde_json::from_value::<Envelope>(item.clone()) {
            Ok(e) => batch.envelopes.push(e),
            Err(e) => {
                let observation_id =
                    serde_json::from_value(item.get("observation_id").cloned().ok_or(bad(
                        format!("malformed envelope without observation_id: {e}"),
                    ))?)
                    .map_err(bad)?;
                rejected.push(hx_store::Rejected {
                    observation_id,
                    reason: e.to_string(),
                });
            }
        }
    }
    let mut ack = store.ingest(batch, &token).await?;
    ack.rejected.extend(rejected);
    Ok(Json(ack))
}
async fn upload_blob(
    State(store): State<Store>,
    Path(sha): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    auth(&store, &headers, "ingest").await?;
    if !hx_store::valid_hash(&sha) || hx_store::hash(&body) != sha {
        return Err(bad("sha256 mismatch"));
    }
    Ok(if store.upload_blob(&sha, &body).await? {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    })
}
async fn download_blob(
    State(store): State<Store>,
    Path(sha): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let token = auth(&store, &headers, "read_raw").await?;
    if !hx_store::valid_hash(&sha) {
        return Err(bad("invalid hash"));
    }
    let allowed: bool = store.blob_allowed(&sha, &token.prefixes).await?;
    if !allowed {
        return Err(ApiError(StatusCode::NOT_FOUND, "blob not found".into()));
    }
    let published: bool = store.blob_published(&sha).await?;
    if !published {
        return Err(ApiError(StatusCode::GONE, "evidence_expired".into()));
    }
    let bytes = tokio::fs::read(store.blob_path(&sha)?)
        .await
        .map_err(|_| ApiError(StatusCode::GONE, "evidence_expired".into()))?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (axum::http::header::CONTENT_DISPOSITION, "attachment"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Range {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub level: Option<String>,
}
impl Range {
    pub fn window(&self) -> ApiResult<Interval> {
        if self.to <= self.from || self.to - self.from > chrono::Duration::days(31) {
            return Err(bad("range must be positive and at most 31 days"));
        }
        Ok(Interval {
            start: self.from,
            end: self.to,
        })
    }
}
pub async fn timeline_value(
    store: &Store,
    token: &Token,
    policy: &DisclosurePolicy,
    w: Interval,
    level: &str,
) -> ApiResult<Value> {
    if !["raw", "lanes", "episodes", "tasks"].contains(&level) {
        return Err(bad("unknown level"));
    }
    if ["raw", "lanes"].contains(&level) && !token.has("read_raw") {
        return Err(ApiError(StatusCode::FORBIDDEN, "read_raw required".into()));
    }
    let observations: Vec<_> = store
        .observations(w, level != "raw")
        .await?
        .into_iter()
        .filter(|o| token.allows(o.envelope.connector_instance.as_str()))
        .collect();
    let observed = observations
        .iter()
        .map(|o| o.envelope.time.observed_at)
        .max();
    let visible: Vec<_> = observations
        .iter()
        .filter(|o| {
            token.has("read_raw")
                || (o.envelope.sensitivity == hx_schema::Sensitivity::Normal
                    && hx_schema::kinds::is_core(o.envelope.kind.as_str()))
        })
        .cloned()
        .collect();
    let compiled = hx_compile::compile(&visible, w);
    // Prose cannot be selectively redacted: require every namespace and cited
    // record to be visible. The owner's read_raw scope deliberately bypasses disclosure.
    let rows = store
        .canonical_revisions(w.start, w.end, &token.prefixes)
        .await?;
    // Sensitive egress controls model transport, not the API reader's access.
    let broker = mcp::Broker::new(
        store.clone(),
        policy.clone(),
        processing::SensitiveEgress::Full,
        w,
    );
    let mut revisions = vec![];
    let mut interpreted = None;
    for row in rows {
        let interpretation: Value = row.interpretation;
        if !token.has("read_raw")
            && !broker
                .allowed_ids(&mcp::revision_evidence(&interpretation))
                .await?
        {
            continue;
        }
        interpreted = interpreted.max(Some(row.end_at));
        revisions.push(json!({"revision_id":row.id,"input_generation":row.input_generation,"interpretation":interpretation,"has_agent_run":row.has_agent_run}));
    }
    let mut out = json!({"observed_through":observed,"interpreted_through":interpreted,"coverage":compiled.coverage,"revisions":revisions});
    if level == "raw" {
        out["observations"] = serde_json::to_value(observations).map_err(|e| bad(e.to_string()))?;
    }
    if level == "lanes" {
        out["timeline"] = serde_json::to_value(compiled).map_err(|e| bad(e.to_string()))?;
    }
    Ok(out)
}
async fn timeline(
    State(store): State<Store>,
    Extension(policy): Extension<DisclosurePolicy>,
    headers: HeaderMap,
    Query(range): Query<Range>,
) -> ApiResult<Json<Value>> {
    let scope = if matches!(range.level.as_deref(), Some("raw" | "lanes")) {
        "read_raw"
    } else {
        "read_l3"
    };
    let token = auth(&store, &headers, scope).await?;
    Ok(Json(
        timeline_value(
            &store,
            &token,
            &policy,
            range.window()?,
            range.level.as_deref().unwrap_or("episodes"),
        )
        .await?,
    ))
}
#[derive(Deserialize)]
struct Search {
    q: String,
}
async fn search(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<Search>,
) -> ApiResult<Json<Value>> {
    let token = auth(&store, &headers, "read_l3").await?;
    Ok(Json(
        json!({"results":store.search(&q.q,token.has("read_raw"),&token.prefixes).await?}),
    ))
}
pub async fn authorized_evidence(store: &Store, token: &Token, id: i64) -> ApiResult<Value> {
    let v = store
        .evidence(id)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "evidence not found".into()))?;
    if !token.allows(v["envelope"]["connector_instance"].as_str().unwrap_or("")) {
        return Err(ApiError(StatusCode::NOT_FOUND, "evidence not found".into()));
    }
    if v["sensitivity"] != "normal" && !token.has("read_raw") {
        return Err(ApiError(StatusCode::FORBIDDEN, "read_raw required".into()));
    }
    Ok(v)
}
async fn evidence(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let token = auth(&store, &headers, "read_l3").await?;
    Ok(Json(authorized_evidence(&store, &token, id).await?))
}
async fn entities(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let token = auth(&store, &headers, "read_l3").await?;
    let values: Vec<Value> = store
        .visible_entities(token.has("read_raw"), &token.prefixes)
        .await?;
    Ok(Json(json!({"entities":values})))
}
async fn now(
    State(store): State<Store>,
    Extension(policy): Extension<DisclosurePolicy>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let token = auth(&store, &headers, "read_l3").await?;
    let now = Utc::now();
    Ok(Json(
        timeline_value(
            &store,
            &token,
            &policy,
            Interval {
                start: now - chrono::Duration::hours(4),
                end: now,
            },
            "episodes",
        )
        .await?,
    ))
}
