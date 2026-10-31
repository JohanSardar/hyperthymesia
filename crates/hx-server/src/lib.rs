//! Authenticated ingest, evidence API, and a local, escaped day view.
pub mod dashboard;
pub mod disclosure;
pub mod processing;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use hx_schema::{Envelope, Interval};
use hx_store::{Store, Token, UploadBatch};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

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
pub fn router(store: Store) -> Router {
    Router::new()
        .route("/login", get(dashboard::login).post(dashboard::sign_in))
        .route("/v1/events", post(events))
        .route("/v1/blobs/{sha256}", put(upload_blob).get(download_blob))
        .route("/v1/timeline", get(timeline))
        .route("/v1/search", get(search))
        .route("/v1/evidence/{id}", get(evidence))
        .route("/v1/entities", get(entities))
        .route("/v1/now", get(now))
        .route("/", get(dashboard::today))
        .route("/day/{day}", get(dashboard::day))
        .route("/evidence/{id}", get(dashboard::evidence_page))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(store)
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
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM event_blobs eb JOIN events e ON e.id=eb.event_id WHERE eb.sha256=$1 AND EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE starts_with(e.connector_instance,p)))").bind(&sha).bind(&token.prefixes).fetch_one(&store.pool).await?;
    if !allowed {
        return Err(ApiError(StatusCode::NOT_FOUND, "blob not found".into()));
    }
    let published: bool = sqlx::query_scalar("SELECT published FROM blobs WHERE sha256=$1")
        .bind(&sha)
        .fetch_one(&store.pool)
        .await?;
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
    // A canonical interpretation may combine namespaces. Serve it only when every
    // input namespace is authorized, rather than attempting to redact derived prose.
    let rows=sqlx::query("SELECT r.id,r.input_generation,r.interpretation,w.end_at FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE w.start_at<$2 AND w.end_at>$1 AND NOT EXISTS(SELECT 1 FROM unnest(r.input_namespaces) n WHERE NOT EXISTS(SELECT 1 FROM unnest($3::text[]) p WHERE starts_with(n,p))) ORDER BY w.start_at")
        .bind(w.start).bind(w.end).bind(&token.prefixes).fetch_all(&store.pool).await?;
    let revisions:Vec<Value>=rows.iter().map(|r|json!({"revision_id":r.get::<i64,_>("id"),"input_generation":r.get::<i64,_>("input_generation"),"interpretation":r.get::<Value,_>("interpretation")})).collect();
    let interpreted = rows
        .iter()
        .map(|r| r.get::<DateTime<Utc>, _>("end_at"))
        .max();
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
    let values:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'entity',a.entity,'assertion',a.assertion,'evidence',a.evidence) FROM entity_assertions a WHERE ($1 OR sensitivity='normal') AND NOT EXISTS(SELECT 1 FROM events e WHERE e.id=ANY(a.evidence) AND NOT EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE starts_with(e.connector_instance,p)))").bind(token.has("read_raw")).bind(&token.prefixes).fetch_all(&store.pool).await?;
    Ok(Json(json!({"entities":values})))
}
async fn now(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let token = auth(&store, &headers, "read_l3").await?;
    let now = Utc::now();
    Ok(Json(
        timeline_value(
            &store,
            &token,
            Interval {
                start: now - chrono::Duration::hours(4),
                end: now,
            },
            "episodes",
        )
        .await?,
    ))
}
