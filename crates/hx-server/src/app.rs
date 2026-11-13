//! Web app delivery and the owner-facing Today API.
use crate::{ApiError, ApiResult, auth, bad};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response, Sse, sse::Event},
};
use chrono::{FixedOffset, Local, TimeZone, Utc};
use hx_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, path::Component, time::Duration};

const ASSETS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/app_assets.rs"));

pub async fn index(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Response> {
    asset(State(store), Path(String::new()), headers).await
}

pub async fn asset(
    State(store): State<Store>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Err(error) = auth(&store, &headers, "read_raw").await {
        return if error.0 == StatusCode::UNAUTHORIZED {
            Ok(Redirect::to("/login").into_response())
        } else {
            Err(error)
        };
    }
    if std::path::Path::new(&path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || path.contains('\\')
    {
        return Err(bad("invalid asset path"));
    }
    let requested = if path.is_empty() { "index.html" } else { &path };
    // Client routes use the SPA index; missing assets must remain 404s.
    let spa = !requested.rsplit('/').next().unwrap_or("").contains('.');
    let root = std::env::var_os("HX_APP_DIST")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            cfg!(debug_assertions)
                .then(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/dist"))
                .filter(|p| p.join("index.html").is_file())
        });
    let (name, bytes) = if let Some(root) = root {
        let root = tokio::fs::canonicalize(root).await.map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "web build unavailable".into(),
            )
        })?;
        let mut name = requested;
        let mut target = tokio::fs::canonicalize(root.join(name)).await;
        if target
            .as_ref()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            && spa
        {
            name = "index.html";
            target = tokio::fs::canonicalize(root.join(name)).await;
        }
        let target =
            target.map_err(|_| ApiError(StatusCode::NOT_FOUND, "asset not found".into()))?;
        if !target.starts_with(&root) {
            return Err(ApiError(StatusCode::NOT_FOUND, "asset not found".into()));
        }
        (
            name,
            tokio::fs::read(target)
                .await
                .map_err(|_| ApiError(StatusCode::NOT_FOUND, "asset not found".into()))?,
        )
    } else {
        let (name, bytes) = ASSETS
            .iter()
            .find(|(name, _)| *name == requested)
            .or_else(|| {
                spa.then(|| ASSETS.iter().find(|(name, _)| *name == "index.html"))
                    .flatten()
            })
            .ok_or(ApiError(StatusCode::NOT_FOUND, "asset not found".into()))?;
        (*name, bytes.to_vec())
    };
    let mime = match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct TodayQuery {
    utc_offset_min: Option<i32>,
}
pub async fn today(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<TodayQuery>,
) -> ApiResult<Response> {
    let token = auth(&store, &headers, "read_raw").await?;
    let offset = match query.utc_offset_min {
        Some(minutes) if (-1439..=1439).contains(&minutes) => {
            FixedOffset::east_opt(minutes * 60).unwrap()
        }
        Some(_) => return Err(bad("invalid UTC offset")),
        None => *Local::now().offset(),
    };
    let date = Utc::now().with_timezone(&offset).date_naive();
    let start = offset
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let end = start + chrono::Duration::days(1);
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(store.today_figures(start, end, &token.prefixes).await?),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct StreamQuery {
    after: Option<i64>,
}
pub async fn stream(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> ApiResult<Response> {
    auth(&store, &headers, "read_raw").await?;
    let after = headers
        .get("last-event-id")
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(|| bad("invalid Last-Event-ID"))
        })
        .transpose()?
        .or(query.after);
    if after.is_some_and(|id| id < 0) {
        return Err(bad("invalid entry cursor"));
    }
    let output = futures_util::stream::unfold(
        (store, headers, after, true),
        |(store, headers, mut after, first)| async move {
            if !first {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            // Re-check revocation and current namespace grants for every batch.
            let token = auth(&store, &headers, "read_raw").await.ok()?;
            let rows = match store.entry_log(after, &token.prefixes).await {
                Ok(rows) => rows,
                Err(_) => {
                    return Some((
                        Ok::<_, Infallible>(
                            Event::default().event("error").data("database unavailable"),
                        ),
                        (store, headers, after, false),
                    ));
                }
            };
            let event = if let Some(last) = rows.last() {
                after = Some(last.id);
                let entries: Vec<Value> = rows.iter().map(|row| json!({"id":row.id,"received_at":row.received_at,"envelope":row.envelope})).collect();
                Event::default()
                    .event("entries")
                    .id(last.id.to_string())
                    .json_data(json!({"entries":entries,"cursor":last.id}))
                    .expect("JSON values serialize")
            } else {
                // An empty initial snapshot still completes the initial read.
                if after.is_none() {
                    after = Some(0);
                }
                Event::default().comment("keepalive")
            };
            Some((Ok::<_, Infallible>(event), (store, headers, after, false)))
        },
    );
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Sse::new(output),
    )
        .into_response())
}
