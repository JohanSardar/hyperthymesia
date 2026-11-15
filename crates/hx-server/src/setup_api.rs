//! Owner-only JSON for the app's first-run setup on the Mac the server runs on:
//! live permission state, the drag-to-grant guide, proof that screen text is
//! arriving, and pairing this Mac with another computer instead.
//!
//! The Mac app and this server share the owner's account, so they talk through
//! small private files in the app's support folder, as `hx-server setup` does.
use crate::{
    ApiResult, bad,
    guided_setup::{support_dir, write_private},
    runs_api::{owner, owner_write},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, header},
    response::Response,
    routing::{get, post},
};
use chrono::Utc;
use hx_store::{Store, Ts};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn router() -> Router<Store> {
    Router::new()
        .route("/v1/setup", get(status))
        .route("/v1/setup/permissions", post(permissions))
        .route("/v1/setup/proof", get(proof))
        .route("/v1/setup/pair-elsewhere", post(pair_elsewhere))
        .layer(DefaultBodyLimit::max(4096))
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

fn read(path: &std::path::Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

async fn status(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let dir = support_dir();
    let agent = read(&dir.join("agent.json"));
    let server_url = agent
        .as_ref()
        .and_then(|a| a["server_url"].as_str())
        .unwrap_or_default()
        .to_owned();
    let paired = agent
        .as_ref()
        .and_then(|a| a["token"].as_str())
        .is_some_and(|t| !t.is_empty());
    let here = url::Url::parse(&server_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|h| matches!(h.as_str(), "127.0.0.1" | "localhost" | "[::1]"));
    // Written by the running Mac app when a grant changes and every 30 seconds.
    let permissions = read(&dir.join("permissions.json")).filter(|p| {
        p["at"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| Utc::now().signed_duration_since(at) < chrono::Duration::seconds(90))
    });
    let guide = read(&dir.join("setup/state.json"))
        .filter(|s| s["run"].as_str().is_some_and(|r| r.starts_with("app-")));
    let last: Option<Ts> = sqlx::query_scalar("SELECT max(received_at) FROM events WHERE connector_instance LIKE 'macos-agent:%' AND received_at>$1")
        .bind(Ts(Ts::now().0 - 86_400_000_000)).fetch_one(&store.pool).await?;
    Ok(Json(json!({
        "app": {
            "installed": crate::guided_setup::app_bundle().is_some(),
            "paired": paired,
            "paired_here": paired && here,
            "server": if paired && !here { url::Url::parse(&server_url).ok().and_then(|u| u.host_str().map(str::to_owned)) } else { None },
            "running": permissions.is_some(),
        },
        "permissions": permissions,
        "guide": guide,
        "pairing": read(&dir.join("pair-result.json")),
        "last_record_at": last.map(|t| t.0),
    })))
}

/// Opens the drag-to-grant guide beside System Settings for what is still off.
async fn permissions(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner_write(&store, &headers).await?;
    let dir = support_dir().join("setup");
    std::fs::create_dir_all(&dir).map_err(anyhow::Error::from)?;
    let run = format!("app-{}", ulid::Ulid::generate());
    // Input Monitoring stays in the list: on macOS 26 the guide marks it done
    // with Accessibility; on older systems it asks for it, as setup always did.
    write_private(
        &dir.join("request.json"),
        &json!({"run":run,"steps":["accessibility","input","screen"],"signin_title":null}),
    )?;
    Ok(Json(json!({"run":run})))
}

/// The newest screen text this Mac recorded in the last two minutes, so the
/// person sees capture working on real text. Owner only; nothing is invented.
async fn proof(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers).await?;
    let since = Ts(Ts::now().0 - 120_000_000);
    let rows: Vec<(Ts, Value)> = sqlx::query_as("SELECT source_start,envelope FROM events INDEXED BY events_time WHERE source_start>=$1 AND kind='text.snapshot' AND connector_instance LIKE 'macos-media:%' ORDER BY source_start DESC LIMIT 12")
        .bind(since).fetch_all(&store.pool).await?;
    let mut items = vec![];
    for (at, envelope) in rows {
        let payload = &envelope["payload"];
        let lines: Vec<&str> = payload["ocr"]["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["text"].as_str())
            .map(str::trim)
            .filter(|t| t.chars().filter(|c| c.is_alphanumeric()).count() >= 3)
            .take(8)
            .collect();
        if lines.is_empty() {
            continue;
        }
        items.push(json!({
            "at": at.0,
            "app": payload["app_name"],
            "title": payload["window_title"],
            "lines": lines,
        }));
        if items.len() == 3 {
            break;
        }
    }
    Ok(Json(json!({"items": items, "since": since.0})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    link: String,
}
/// Sends this Mac's recording to another computer: the running Mac app redeems
/// the one-time link and reports in `pair-result.json`.
async fn pair_elsewhere(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Pair>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner_write(&store, &headers).await?;
    let Json(Pair { link }) = request.map_err(|_| bad("invalid request body"))?;
    let link = link.trim().to_owned();
    let valid = url::Url::parse(&link).is_ok_and(|u| {
        u.scheme() == "hyperthymesia"
            && u.host_str() == Some("pair")
            && u.query_pairs().any(|(k, v)| {
                k == "code" && v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
            })
            && u.query_pairs().any(|(k, _)| k == "server")
    });
    if !valid {
        return Err(bad(
            "That isn't a pairing link. On the other computer, run hx-server pair and copy the link it prints.",
        ));
    }
    let dir = support_dir();
    let _ = std::fs::remove_file(dir.join("pair-result.json"));
    write_private(
        &dir.join("pair-request.json"),
        &json!({"link":link,"at":Utc::now().to_rfc3339()}),
    )?;
    Ok(Json(json!({"queued":true})))
}
