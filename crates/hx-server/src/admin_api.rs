//! Owner-only JSON for Devices and the Storage, Devices-and-server and
//! Advanced settings: device names and merges, disk use and keep-for limits,
//! backups, deleting a time range, export, pairing links, logs, repair and
//! restart. Every delete takes a verified backup first and is logged.
use crate::{ApiError, ApiResult, auth, bad, preferences};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use chrono::{DateTime, Duration, Utc};
use hx_store::{Store, Ts, erase::Scope};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path as FsPath, PathBuf},
    sync::{LazyLock, Mutex, OnceLock},
};

/// Record types the owner can set a keep-for limit on, and the kinds in each.
/// Write-ups, health and clock records have no limit of their own.
pub const RETAINABLE: [&str; 5] = [
    "screen_text",
    "screen_pictures",
    "apps",
    "activity",
    "connections",
];
fn kinds_of(kind: &str) -> (Vec<String>, bool) {
    let list: &[&str] = match kind {
        "screen_text" => &["text.snapshot"],
        "screen_pictures" => &["media.still", "media.chunk"],
        "apps" => &["focus.span", "presence.span", "ui.click"],
        "activity" => &["input.bucket"],
        _ => &[],
    };
    (
        list.iter().map(|s| s.to_string()).collect(),
        kind == "connections",
    )
}

/// Set once by `serve`: where this server listens and when it started.
pub static SERVER: OnceLock<(std::net::SocketAddr, DateTime<Utc>)> = OnceLock::new();
/// Notified when the owner asks for a restart; `serve` shuts down gracefully.
pub static RESTART: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// One delete, backup or restore at a time.
static BUSY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static STORAGE: LazyLock<Mutex<Option<(std::time::Instant, Value)>>> =
    LazyLock::new(|| Mutex::new(None));

pub fn router() -> Router<Store> {
    Router::new()
        .route("/v1/devices", get(devices))
        .route("/v1/devices/merge", post(merge))
        .route("/v1/devices/pair", post(pair))
        .route("/v1/devices/{device}", put(rename))
        .route("/v1/devices/{device}/unmerge", post(unmerge))
        .route("/v1/storage", get(storage))
        .route("/v1/storage/clean-up", post(clean_up_now))
        .route("/v1/backups", get(backups).post(create_backup))
        .route("/v1/backups/{name}", delete(delete_backup))
        .route("/v1/backups/{name}/restore", post(restore_backup))
        .route("/v1/erase/plan", post(erase_plan))
        .route("/v1/erase", post(erase_range))
        .route("/v1/erase/log", get(erase_log))
        .route("/v1/export", get(export))
        .route("/v1/server", get(server))
        .route("/v1/server/logs", get(logs))
        .route("/v1/server/repair", post(repair))
        .route("/v1/server/restart", post(restart))
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

async fn owner(store: &Store, headers: &HeaderMap, write: bool) -> ApiResult<()> {
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
    if write && !headers.contains_key(header::AUTHORIZATION) {
        crate::connections::same_origin(headers)?;
    }
    Ok(())
}
fn body<T>(request: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    request
        .map(|Json(v)| v)
        .map_err(|_| bad("invalid request body"))
}
fn failed(error: anyhow::Error) -> ApiError {
    tracing::error!("{error:#}");
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, plain(&error))
}
/// A short, content-free reason for the owner.
fn plain(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    if text.contains("No space left") {
        "The disk is full".into()
    } else {
        text.chars().take(200).collect()
    }
}
fn micros(v: Option<i64>) -> Option<DateTime<Utc>> {
    v.and_then(DateTime::<Utc>::from_timestamp_micros)
}

// ---------------------------------------------------------------- devices

/// Device name as recorded → the device it shows as, after merges.
pub async fn device_aliases(store: &Store) -> anyhow::Result<BTreeMap<String, String>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT device, merged_into FROM device_names WHERE merged_into IS NOT NULL",
    )
    .fetch_all(&store.pool)
    .await?;
    let map: BTreeMap<String, String> = rows
        .into_iter()
        .filter_map(|(d, m)| m.map(|m| (d, m)))
        .collect();
    // Merges point at a device that is not itself merged, but follow chains anyway.
    Ok(map
        .keys()
        .map(|d| {
            let mut target = &map[d];
            for _ in 0..8 {
                match map.get(target) {
                    Some(next) => target = next,
                    None => break,
                }
            }
            (d.clone(), target.clone())
        })
        .collect())
}
/// Owner names for devices, by recorded name.
pub async fn device_labels(store: &Store) -> anyhow::Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT device, name FROM device_names WHERE name IS NOT NULL")
            .fetch_all(&store.pool)
            .await?;
    Ok(rows.into_iter().collect())
}

async fn devices(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let rows: Vec<(String, i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT device, count(*), min(source_start), max(source_start) FROM events INDEXED BY events_device WHERE device IS NOT NULL GROUP BY device",
    )
    .fetch_all(&store.pool)
    .await?;
    let names: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT device, name, merged_into FROM device_names")
            .fetch_all(&store.pool)
            .await?;
    let names: BTreeMap<_, _> = names.into_iter().map(|(d, n, m)| (d, (n, m))).collect();
    let aliases = device_aliases(&store).await?;
    let paused = crate::recording::paused_until(&store).await?.is_some();
    let on = preferences::load(&store).await?.mac_recording;
    let now = Utc::now();
    // One entry per device as shown; merged names listed under it.
    let mut shown: BTreeMap<String, Value> = BTreeMap::new();
    let mut all: Vec<&(String, i64, Option<i64>, Option<i64>)> = rows.iter().collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.3));
    for (device, count, first, last) in all {
        let target = aliases
            .get(device)
            .cloned()
            .unwrap_or_else(|| device.clone());
        let entry = shown.entry(target.clone()).or_insert_with(|| {
            json!({"device": target, "name": names.get(&target).and_then(|n| n.0.clone()), "records": 0, "first": null, "last": null, "names": []})
        });
        entry["records"] = json!(entry["records"].as_i64().unwrap_or(0) + count);
        let first = micros(*first);
        let last = micros(*last);
        if entry["first"].is_null()
            || first.is_some_and(|f| Some(f) < serde_json::from_value(entry["first"].clone()).ok())
        {
            entry["first"] = json!(first);
        }
        if entry["last"].is_null()
            || last.is_some_and(|l| Some(l) > serde_json::from_value(entry["last"].clone()).ok())
        {
            entry["last"] = json!(last);
        }
        entry["names"]
            .as_array_mut()
            .expect("array")
            .push(json!({"device": device, "records": count, "first": first, "last": last}));
    }
    let list: Vec<Value> = shown
        .into_values()
        .map(|mut d| {
            let last: Option<DateTime<Utc>> = serde_json::from_value(d["last"].clone()).ok();
            let recent = last.is_some_and(|l| now - l < Duration::minutes(3));
            d["recording"] = json!(if !recent {
                "quiet"
            } else if !on {
                "off"
            } else if paused {
                "paused"
            } else {
                "recording"
            });
            d
        })
        .collect();
    let mut list = list;
    list.sort_by(|a, b| b["last"].as_str().cmp(&a["last"].as_str()));
    let pairings = store.list_pair_codes().await?;
    Ok(Json(json!({"devices": list, "pairings": pairings})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rename {
    name: Option<String>,
}
async fn rename(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(device): Path<String>,
    request: Result<Json<Rename>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    let Rename { name } = body(request)?;
    let name = name
        .map(|n| {
            n.trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect::<String>()
        })
        .filter(|n| !n.is_empty());
    known_device(&store, &device).await?;
    let mut tx = store.begin_write().await?;
    sqlx::query("INSERT INTO device_names(device,name) VALUES($1,$2) ON CONFLICT(device) DO UPDATE SET name=excluded.name")
        .bind(&device)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"device": device, "name": name})))
}
async fn known_device(store: &Store, device: &str) -> ApiResult<()> {
    let known: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_device WHERE device=$1) OR EXISTS(SELECT 1 FROM device_names WHERE device=$1)")
        .bind(device)
        .fetch_one(&store.pool)
        .await?;
    if known {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            "no device by that name".into(),
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Merge {
    devices: Vec<String>,
    into: String,
}
async fn merge(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Merge>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    let Merge { devices, into } = body(request)?;
    if devices.is_empty() || devices.len() > 50 || devices.contains(&into) {
        return Err(bad("pick the devices to merge and one to keep"));
    }
    known_device(&store, &into).await?;
    for d in &devices {
        known_device(&store, d).await?;
    }
    let mut tx = store.begin_write().await?;
    sqlx::query("INSERT INTO device_names(device) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(&into)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE device_names SET merged_into=NULL WHERE device=$1")
        .bind(&into)
        .execute(&mut *tx)
        .await?;
    for d in &devices {
        // Whatever was merged into this device now goes to the kept one too.
        sqlx::query("UPDATE device_names SET merged_into=$2 WHERE merged_into=$1")
            .bind(d)
            .bind(&into)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO device_names(device,merged_into) VALUES($1,$2) ON CONFLICT(device) DO UPDATE SET merged_into=excluded.merged_into")
            .bind(d)
            .bind(&into)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"into": into, "merged": devices})))
}
async fn unmerge(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(device): Path<String>,
) -> ApiResult<StatusCode> {
    owner(&store, &headers, true).await?;
    let mut tx = store.begin_write().await?;
    sqlx::query("UPDATE device_names SET merged_into=NULL WHERE device=$1")
        .bind(&device)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    name: String,
    /// `local` for a Mac on this computer, `remote` for the owner's remote address.
    #[serde(default)]
    reach: Option<String>,
}
async fn pair(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Pair>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    let Pair { name, reach } = body(request)?;
    let name = name.trim().to_owned();
    if name.is_empty() || name.len() > 120 {
        return Err(bad("give the device a name"));
    }
    let server = match reach.as_deref() {
        Some("remote") => preferences::load(&store)
            .await?
            .remote_url
            .ok_or_else(|| bad("set a remote address first"))?,
        _ => local_url(),
    };
    let code = store
        .create_pair_code(&name)
        .await
        .map_err(|e| bad(plain(&e)))?;
    let mut link = url::Url::parse("hyperthymesia://pair").expect("static URL");
    link.query_pairs_mut()
        .append_pair("server", server.trim_end_matches('/'))
        .append_pair("code", &code);
    Ok(Json(
        json!({"link": link.to_string(), "server": server, "expires_at": Utc::now() + Duration::minutes(15)}),
    ))
}
fn local_url() -> String {
    SERVER.get().map_or_else(
        || "http://127.0.0.1:7377".into(),
        |(addr, _)| format!("http://{addr}"),
    )
}

// ---------------------------------------------------------------- storage

async fn storage(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let cached = STORAGE.lock().expect("storage cache").clone();
    if !q.contains_key("fresh")
        && let Some((at, value)) = cached
        && at.elapsed() < std::time::Duration::from_secs(600)
    {
        return Ok(Json(with_disk(&store, value).await?));
    }
    let rows: Vec<(String, bool, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT kind, connector_instance LIKE 'source:%', count(*), sum(length(envelope)), min(source_start) FROM events GROUP BY 1,2",
    )
    .fetch_all(&store.pool)
    .await?;
    let mut types: BTreeMap<&str, (i64, i64, Option<i64>)> = BTreeMap::new();
    for (kind, connection, count, bytes, first) in rows {
        let group = if connection {
            "connections"
        } else {
            match kind.as_str() {
                "text.snapshot" => "screen_text",
                "media.still" | "media.chunk" => "screen_pictures",
                "focus.span" | "presence.span" | "ui.click" => "apps",
                "input.bucket" => "activity",
                _ => "system",
            }
        };
        let e = types.entry(group).or_default();
        e.0 += count;
        e.1 += bytes;
        e.2 = match (e.2, first) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    let (files, file_bytes): (i64, i64) =
        sqlx::query_as("SELECT count(*), COALESCE(sum(bytes),0) FROM blobs")
            .fetch_one(&store.pool)
            .await?;
    let (ocr, ocr_bytes): (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(length(result)),0) FROM ocr_jobs WHERE result IS NOT NULL",
    )
    .fetch_one(&store.pool)
    .await?;
    let (revisions, revision_bytes, first_revision): (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(length(interpretation)),0), min(start_at) FROM revisions",
    )
    .fetch_one(&store.pool)
    .await?;
    let entry = |key: &str| types.get(key).copied().unwrap_or_default();
    let mut out = vec![];
    for key in [
        "screen_text",
        "screen_pictures",
        "apps",
        "activity",
        "connections",
        "system",
    ] {
        let (records, mut bytes, first) = entry(key);
        let mut extra = json!({});
        if key == "screen_pictures" {
            bytes += file_bytes + ocr_bytes;
            extra = json!({"files": files, "file_bytes": file_bytes, "text_read": ocr});
        }
        out.push(json!({"type": key, "records": records, "bytes": bytes, "first": micros(first), "extra": extra}));
    }
    out.push(json!({"type": "write_ups", "records": revisions, "bytes": revision_bytes, "first": micros(first_revision), "extra": {}}));
    let value = json!({"types": out, "measured_at": Utc::now()});
    *STORAGE.lock().expect("storage cache") = Some((std::time::Instant::now(), value.clone()));
    Ok(Json(with_disk(&store, value).await?))
}
async fn with_disk(store: &Store, mut value: Value) -> ApiResult<Value> {
    let size = |name: &str| std::fs::metadata(store.data_dir.join(name)).map_or(0, |m| m.len());
    value["databases"] =
        json!(size("core.sqlite") + size("core.sqlite-wal") + size("agent.sqlite"));
    value["disk_free"] = json!(free_bytes(&store.data_dir));
    value["state_dir"] = json!(store.data_dir);
    value["backups_dir"] = json!(backups_dir(store));
    value["last_clean_up"] = json!(read_json(&backups_dir(store).join("clean-up.json")));
    Ok(value)
}

/// Free bytes on the disk holding `path`, from `df`; no unsafe code here.
fn free_bytes(path: &FsPath) -> Option<u64> {
    let out = std::process::Command::new("/bin/df")
        .arg("-k")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let line = text.lines().nth(1)?;
    line.split_whitespace()
        .nth(3)?
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
}

// ---------------------------------------------------------------- backups

/// Backups live beside the state folder, never inside it.
pub fn backups_dir(store: &Store) -> PathBuf {
    store
        .data_dir
        .parent()
        .unwrap_or(&store.data_dir)
        .join("backups")
}
fn log_path(store: &Store) -> PathBuf {
    backups_dir(store).join("deletions.jsonl")
}
fn read_json(path: &FsPath) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
fn append_log(store: &Store, entry: Value) -> anyhow::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(backups_dir(store))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(store))?;
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    file.sync_all()?;
    Ok(())
}
fn valid_backup_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !name.starts_with('-')
}

/// Takes a complete, verified backup. Refuses when the disk could fill up.
pub async fn take_backup(store: &Store, reason: &str) -> anyhow::Result<(String, PathBuf)> {
    let dir = backups_dir(store);
    std::fs::create_dir_all(&dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let size = |name: &str| std::fs::metadata(store.data_dir.join(name)).map_or(0, |m| m.len());
    // Pictures are cloned on APFS; the databases are written out in full.
    let need =
        2 * (size("core.sqlite") + size("core.sqlite-wal") + size("agent.sqlite")) + 1_000_000_000;
    if let Some(free) = free_bytes(&dir) {
        anyhow::ensure!(
            free > need,
            "Not enough free disk for a backup: {} free, about {} needed. Nothing was changed.",
            human(free),
            human(need)
        );
    }
    let name = format!("{}-{reason}", Utc::now().format("%Y%m%d-%H%M%S"));
    let path = dir.join(&name);
    hx_store::backup::backup(&store.data_dir, &path).await?;
    anyhow::ensure!(
        path.join("manifest.json").is_file(),
        "the backup did not finish"
    );
    Ok((name, path))
}
fn human(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}
fn dir_bytes(path: &FsPath) -> u64 {
    std::fs::read_dir(path).map_or(0, |entries| {
        entries
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => dir_bytes(&e.path()),
                Ok(_) => e.metadata().map_or(0, |m| m.len()),
                Err(_) => 0,
            })
            .sum()
    })
}

async fn backups(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let dir = backups_dir(&store);
    let mut list = vec![];
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !entry.path().is_dir() || !valid_backup_name(&name) {
                continue;
            }
            let complete = entry.path().join("manifest.json").is_file();
            let reason = name.splitn(3, '-').nth(2).unwrap_or("").to_owned();
            let at = DateTime::parse_from_str(
                &format!("{} +0000", name.get(..15).unwrap_or("")),
                "%Y%m%d-%H%M%S %z",
            )
            .ok()
            .map(|t| t.with_timezone(&Utc));
            list.push(json!({"name": name, "reason": reason, "at": at, "complete": complete, "bytes": dir_bytes(&entry.path())}));
        }
    }
    list.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str()));
    // States set aside by a restore, kept until the owner deletes them.
    let mut kept = vec![];
    if let (Some(parent), Some(state_name)) = (
        store.data_dir.parent(),
        store.data_dir.file_name().and_then(|n| n.to_str()),
    ) && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(suffix) = name.strip_prefix(&format!("{state_name}.before-restore-")) {
                kept.push(json!({"name": name, "at": suffix, "path": entry.path()}));
            }
        }
    }
    Ok(Json(
        json!({"dir": dir, "backups": list, "set_aside": kept, "restore": read_json(&restore_result_path(&store))}),
    ))
}
async fn create_backup(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    let _busy = BUSY.try_lock().map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "Another backup or delete is running".into(),
        )
    })?;
    let (name, path) = take_backup(&store, "manual").await.map_err(failed)?;
    Ok(Json(json!({"name": name, "path": path})))
}
async fn delete_backup(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    owner(&store, &headers, true).await?;
    if !valid_backup_name(&name) {
        return Err(bad("not a backup"));
    }
    let path = backups_dir(&store).join(&name);
    if !path.is_dir() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "no backup by that name".into(),
        ));
    }
    let _busy = BUSY
        .try_lock()
        .map_err(|_| ApiError(StatusCode::CONFLICT, "A backup or delete is running".into()))?;
    tokio::fs::remove_dir_all(&path)
        .await
        .map_err(|e| failed(e.into()))?;
    append_log(
        &store,
        json!({"at": Utc::now(), "action": "backup_deleted", "backup": name}),
    )
    .map_err(failed)?;
    Ok(StatusCode::NO_CONTENT)
}

fn restore_request_path(store: &Store) -> PathBuf {
    backups_dir(store).join("restore-request.json")
}
fn restore_result_path(store: &Store) -> PathBuf {
    backups_dir(store).join("restore-result.json")
}
async fn restore_backup(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    if !valid_backup_name(&name) {
        return Err(bad("not a backup"));
    }
    let from = backups_dir(&store).join(&name);
    if !from.join("manifest.json").is_file() {
        return Err(bad("that backup is incomplete and can't be restored"));
    }
    if !supervised() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This server wasn't started by the Mac's service manager, so it can't restart by itself. Stop it and run hx-server restore instead.".into(),
        ));
    }
    let request = json!({"from": from, "state": store.data_dir, "asked_at": Utc::now()});
    std::fs::write(
        restore_request_path(&store),
        serde_json::to_vec(&request).map_err(|e| failed(e.into()))?,
    )
    .map_err(|e| failed(e.into()))?;
    let _ = std::fs::remove_file(restore_result_path(&store));
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        RESTART.notify_one();
    });
    Ok(Json(json!({"restarting": true})))
}

/// Run by `serve` before the store opens: applies a restore the owner asked
/// for. The current state is set aside beside it, never deleted.
pub async fn apply_pending_restore(state_dir: &FsPath) -> anyhow::Result<()> {
    let backups = state_dir.parent().unwrap_or(state_dir).join("backups");
    let request_path = backups.join("restore-request.json");
    let Some(request) = read_json(&request_path) else {
        return Ok(());
    };
    std::fs::remove_file(&request_path)?;
    let from = PathBuf::from(request["from"].as_str().unwrap_or_default());
    let asked_state = PathBuf::from(request["state"].as_str().unwrap_or_default());
    let result_path = backups.join("restore-result.json");
    let result = async {
        anyhow::ensure!(
            asked_state == state_dir,
            "the restore was asked for another state folder"
        );
        anyhow::ensure!(
            from.starts_with(&backups) && from.join("manifest.json").is_file(),
            "the backup is missing or incomplete"
        );
        let name = state_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("state");
        let aside = state_dir.with_file_name(format!(
            "{name}.before-restore-{}",
            Utc::now().format("%Y%m%d-%H%M%S")
        ));
        std::fs::rename(state_dir, &aside)?;
        if let Err(error) = hx_store::backup::restore(&from, state_dir).await {
            let _ = std::fs::remove_dir_all(state_dir);
            std::fs::rename(&aside, state_dir)?;
            return Err(error);
        }
        // Keys and server settings are never in backups; carry them over.
        for item in ["secrets", "server.env", "desktop-session.json"] {
            let source = aside.join(item);
            if source.exists() && !state_dir.join(item).exists() {
                copy_tree(&source, &state_dir.join(item))?;
            }
        }
        carry_tokens(&aside.join("core.sqlite"), &state_dir.join("core.sqlite")).await?;
        Ok(aside)
    }
    .await;
    let entry = match &result {
        Ok(aside) => {
            json!({"at": Utc::now(), "action": "restored", "backup": from.file_name().map(|n| n.to_string_lossy()), "previous_state": aside, "ok": true})
        }
        Err(error) => {
            json!({"at": Utc::now(), "action": "restore_failed", "backup": from.file_name().map(|n| n.to_string_lossy()), "error": plain(error), "ok": false})
        }
    };
    std::fs::write(&result_path, serde_json::to_vec(&entry)?)?;
    use std::io::Write;
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(backups.join("deletions.jsonl"))?;
    writeln!(log, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}
/// API keys and paired devices are credentials, not history: a restore keeps
/// them as they are now, so a Mac paired after the backup keeps uploading and a
/// key revoked since stays revoked.
async fn carry_tokens(prior: &FsPath, restored: &FsPath) -> anyhow::Result<()> {
    use sqlx::{ConnectOptions, Connection};
    if !prior.is_file() {
        return Ok(());
    }
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(restored)
        .connect()
        .await?;
    let path = prior
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("the old state folder's path isn't UTF-8"))?;
    sqlx::query("ATTACH DATABASE $1 AS prior")
        .bind(path)
        .execute(&mut conn)
        .await?;
    let mut tx = conn.begin().await?;
    sqlx::query(
        "INSERT INTO device_tokens(hash,name,prefixes,scopes,revoked) SELECT hash,name,prefixes,scopes,revoked FROM prior.device_tokens WHERE true \
         ON CONFLICT(hash) DO UPDATE SET name=excluded.name, prefixes=excluded.prefixes, scopes=excluded.scopes, revoked=excluded.revoked",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    sqlx::query("DETACH DATABASE prior")
        .execute(&mut conn)
        .await?;
    conn.close().await?;
    Ok(())
}
fn copy_tree(from: &FsPath, to: &FsPath) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if from.is_dir() {
        std::fs::create_dir(to)?;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o700))?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- delete

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Range {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    /// Must be `delete` to delete; the plan route ignores it.
    #[serde(default)]
    confirm: Option<String>,
}
fn range_scope(r: &Range) -> ApiResult<Scope> {
    if r.end <= r.start {
        return Err(bad("the range must end after it starts"));
    }
    if r.end - r.start > Duration::days(3660) {
        return Err(bad("pick a range of at most ten years"));
    }
    Ok(Scope::Range {
        start: r.start,
        end: r.end,
    })
}
async fn agent_tables(store: &Store) -> anyhow::Result<Vec<(String, i64)>> {
    let path = store.data_dir.join("agent.sqlite");
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut names = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names: Vec<String> = names
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut out = vec![];
    for name in names {
        let count: i64 = conn.query_row(
            &format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")),
            [],
            |r| r.get(0),
        )?;
        out.push((name, count));
    }
    Ok(out)
}
async fn erase_plan(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Range>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let range = body(request)?;
    let plan = store
        .erase_plan(&range_scope(&range)?)
        .await
        .map_err(failed)?;
    let tables: Vec<Value> = agent_tables(&store)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(n, c)| json!({"name": n, "rows": c}))
        .collect();
    Ok(Json(
        json!({"plan": plan, "agent_tables": tables, "disk_free": free_bytes(&store.data_dir)}),
    ))
}
async fn erase_range(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Range>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    let range = body(request)?;
    if range.confirm.as_deref() != Some("delete") {
        return Err(bad("confirm the delete"));
    }
    let scope = range_scope(&range)?;
    let _busy = BUSY.try_lock().map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "Another backup or delete is running".into(),
        )
    })?;
    // No backup, no delete.
    let (backup, _) = take_backup(&store, "before-delete").await.map_err(failed)?;
    // No log, no delete: the entry is written before anything goes.
    append_log(&store, json!({"at": Utc::now(), "action": "deleting", "start": range.start, "end": range.end, "backup": backup}))
        .map_err(failed)?;
    let summary = store.erase(&scope).await.map_err(failed)?;
    *STORAGE.lock().expect("storage cache") = None;
    // The agent's own tables have no times, so they may hold copies of what
    // went; they're always emptied, and the agent fills them at its next setup.
    let cleared = clear_agent_tables(&store).await.map_err(|error| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "The time was deleted, but the agent's own tables couldn't be emptied: {}",
                plain(&error)
            ),
        )
    })?;
    let entry = json!({"at": Utc::now(), "action": "deleted_range", "start": range.start, "end": range.end, "backup": backup, "removed": summary, "agent_tables_cleared": cleared});
    if let Err(error) = append_log(&store, entry.clone()) {
        tracing::error!(
            "deleted {} to {} but couldn't log the counts: {error}",
            range.start,
            range.end
        );
    }
    Ok(Json(entry))
}
async fn clear_agent_tables(store: &Store) -> anyhow::Result<Vec<String>> {
    let tables = agent_tables(store).await?;
    let path = store.data_dir.join("agent.sqlite");
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute_batch("PRAGMA secure_delete=ON")?;
    let mut out = vec![];
    for (name, rows) in tables {
        if rows > 0 {
            conn.execute(
                &format!("DELETE FROM \"{}\"", name.replace('"', "\"\"")),
                [],
            )?;
            out.push(name);
        }
    }
    Ok(out)
}
async fn erase_log(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let text = std::fs::read_to_string(log_path(&store)).unwrap_or_default();
    let mut entries: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    // A "deleting" line is only news when nothing logged its outcome.
    let finished: std::collections::HashSet<String> = entries
        .iter()
        .filter(|e| e["action"] == "deleted_range" || e["action"] == "cleaned_up")
        .filter_map(|e| e["backup"].as_str().map(String::from))
        .collect();
    entries.retain(|e| {
        e["action"] != "deleting" || !e["backup"].as_str().is_some_and(|b| finished.contains(b))
    });
    entries.reverse();
    entries.truncate(200);
    Ok(Json(json!({"entries": entries, "path": log_path(&store)})))
}

/// Keep-for limits: once a week, after a backup, records older than their
/// type's limit are deleted and the delete is logged.
pub async fn clean_up(store: &Store, force: bool) -> anyhow::Result<Option<Value>> {
    let preferences = preferences::load(store).await?;
    if preferences.retention.is_empty() {
        return Ok(None);
    }
    let state_path = backups_dir(store).join("clean-up.json");
    let last: Option<DateTime<Utc>> =
        read_json(&state_path).and_then(|v| serde_json::from_value(v["at"].clone()).ok());
    if !force && last.is_some_and(|l| Utc::now() - l < Duration::days(7)) {
        return Ok(None);
    }
    let now = Utc::now();
    let scopes: Vec<(String, u32, Scope)> = preferences
        .retention
        .iter()
        .map(|(kind, days)| {
            let (kinds, connections) = kinds_of(kind);
            (
                kind.clone(),
                *days,
                Scope::Older {
                    before: now - Duration::days(i64::from(*days)),
                    kinds,
                    connections,
                },
            )
        })
        .collect();
    let mut planned = 0;
    for (_, _, scope) in &scopes {
        planned += store.erase_plan(scope).await?.records;
    }
    if planned == 0 {
        let entry = json!({"at": now, "removed": 0});
        std::fs::create_dir_all(backups_dir(store))?;
        std::fs::write(&state_path, serde_json::to_vec(&entry)?)?;
        return Ok(Some(entry));
    }
    let _busy = BUSY.lock().await;
    let (backup, _) = match take_backup(store, "before-clean-up").await {
        Ok(done) => done,
        Err(error) => {
            let entry = json!({"at": now, "action": "clean_up_skipped", "error": plain(&error)});
            append_log(store, entry.clone())?;
            return Ok(Some(entry));
        }
    };
    append_log(
        store,
        json!({"at": now, "action": "deleting", "clean_up": true, "backup": backup}),
    )?;
    let mut removed = serde_json::Map::new();
    for (kind, days, scope) in &scopes {
        let summary = store.erase(scope).await?;
        removed.insert(
            kind.clone(),
            json!({"older_than_days": days, "removed": summary}),
        );
    }
    *STORAGE.lock().expect("storage cache") = None;
    let entry = json!({"at": now, "action": "cleaned_up", "backup": backup, "types": removed});
    append_log(store, entry.clone())?;
    std::fs::write(
        &state_path,
        serde_json::to_vec(&json!({"at": now, "backup": backup}))?,
    )?;
    Ok(Some(entry))
}
async fn clean_up_now(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    Ok(Json(
        clean_up(&store, true)
            .await
            .map_err(failed)?
            .unwrap_or(json!({"removed": 0, "nothing_set": true})),
    ))
}

// ---------------------------------------------------------------- export

#[derive(Deserialize)]
struct ExportQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}
/// Every stored record and write-up in the range as JSON lines, streamed so
/// a long range never sits in memory.
async fn export(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    owner(&store, &headers, false).await?;
    if q.to <= q.from {
        return Err(bad("the range must end after it starts"));
    }
    let (from, to) = (Ts::from(q.from), Ts::from(q.to));
    let stream = async_stream(store, from, to);
    let name = format!(
        "hyperthymesia-{}-to-{}.jsonl",
        q.from.format("%Y%m%d-%H%M"),
        q.to.format("%Y%m%d-%H%M")
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-ndjson".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}
fn async_stream(
    store: Store,
    from: Ts,
    to: Ts,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    // State: the last record ID sent, or None once records are done.
    futures_util::stream::unfold(Some(0i64), move |after| {
        let store = store.clone();
        async move {
            let after = after?;
            let rows: Vec<(i64, String)> = match sqlx::query_as("SELECT id, envelope FROM events INDEXED BY events_time WHERE source_start >= $1 AND source_start < $2 AND id > $3 ORDER BY id LIMIT 1000")
                .bind(from)
                .bind(to)
                .bind(after)
                .fetch_all(&store.pool)
                .await
            {
                Ok(rows) => rows,
                Err(error) => return Some((Err(std::io::Error::other(error)), None)),
            };
            if let Some(last) = rows.last().map(|r| r.0) {
                let mut chunk = Vec::new();
                for (id, envelope) in rows {
                    chunk.extend_from_slice(
                        format!("{{\"type\":\"record\",\"id\":{id},\"envelope\":{envelope}}}\n")
                            .as_bytes(),
                    );
                }
                return Some((Ok(chunk), Some(last)));
            }
            let revisions: Vec<(i64, i64, String)> = sqlx::query_as("SELECT id, start_at, interpretation FROM revisions WHERE start_at >= $1 AND start_at < $2 AND id IN (SELECT canonical_revision FROM windows) ORDER BY start_at")
                .bind(from)
                .bind(to)
                .fetch_all(&store.pool)
                .await
                .unwrap_or_default();
            let mut chunk = Vec::new();
            for (id, start, interpretation) in revisions {
                chunk.extend_from_slice(
                    format!("{{\"type\":\"write_up\",\"id\":{id},\"window_start\":{},\"interpretation\":{interpretation}}}\n", json!(micros(Some(start)))).as_bytes(),
                );
            }
            Some((Ok(chunk), None))
        }
    })
}

// ---------------------------------------------------------------- server

/// Whether a service manager brings this server back after it exits.
pub fn supervised() -> bool {
    std::env::var("HX_SUPERVISED").is_ok_and(|v| v == "1")
        || std::env::var("XPC_SERVICE_NAME").is_ok_and(|v| v.starts_with("com.hyperthymesia."))
}
fn log_file() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HX_LOG_FILE") {
        return Some(PathBuf::from(path));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let path = home.join("Library/Logs/Hyperthymesia/server.log");
    path.is_file().then_some(path)
}
async fn server(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let (address, started) = SERVER
        .get()
        .map_or((None, None), |(a, s)| (Some(a.to_string()), Some(*s)));
    let jobs: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, state, count(*) FROM jobs WHERE state NOT IN ('done') GROUP BY 1,2",
    )
    .fetch_all(&store.pool)
    .await?;
    let running_passes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='interpret' AND state='running'")
            .fetch_one(&store.pool)
            .await?;
    let last_record: Option<i64> = sqlx::query_scalar(
        "SELECT max(received_at) FROM events WHERE id > (SELECT max(id)-1000 FROM events)",
    )
    .fetch_one(&store.pool)
    .await?;
    let tailscale = [
        "/opt/homebrew/bin/tailscale",
        "/usr/local/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ]
    .iter()
    .find(|p| FsPath::new(p).exists())
    .map(|p| p.to_string());
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "address": address,
        "local_url": local_url(),
        "started_at": started,
        "supervised": supervised(),
        "state_dir": store.data_dir,
        "log_file": log_file(),
        "jobs": jobs.into_iter().map(|(k, s, n)| json!({"kind": k, "state": s, "count": n})).collect::<Vec<_>>(),
        "running_passes": running_passes,
        "last_record_at": micros(last_record),
        "remote": {"url": preferences::load(&store).await?.remote_url, "tailscale": tailscale},
    })))
}
async fn logs(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, false).await?;
    let Some(path) = log_file() else {
        return Ok(Json(json!({"path": null, "lines": []})));
    };
    let text = tokio::fs::read(&path).await.unwrap_or_default();
    let tail = &text[text.len().saturating_sub(256 * 1024)..];
    let text = String::from_utf8_lossy(tail);
    let lines: Vec<&str> = text.lines().collect();
    let lines = &lines[lines.len().saturating_sub(300)..];
    // Never send keys or tokens that might have reached a log line.
    let scrub = crate::sources::scrubber(&store, None).await?;
    let lines: Vec<String> = lines.iter().map(|l| scrub.scrub(l)).collect();
    Ok(Json(json!({"path": path, "lines": lines})))
}
async fn repair(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    let mut steps = vec![];
    let check: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(&store.pool)
        .await?;
    let healthy = check == ["ok"];
    steps.push(json!({"step": "Database check", "ok": healthy, "detail": if healthy { "No damage found".to_owned() } else { check.join("; ").chars().take(300).collect() }}));
    let fts =
        sqlx::query("INSERT INTO normalized_fts(normalized_fts,rank) VALUES('integrity-check',1)")
            .execute(&store.pool)
            .await;
    if fts.is_err() {
        let mut tx = store.begin_write().await?;
        let rebuilt = sqlx::query("INSERT INTO normalized_fts(normalized_fts) VALUES('rebuild')")
            .execute(&mut *tx)
            .await
            .is_ok();
        tx.commit().await?;
        steps.push(json!({"step": "Search index", "ok": rebuilt, "detail": if rebuilt { "It was damaged and has been rebuilt" } else { "It is damaged and could not be rebuilt" }}));
    } else {
        steps
            .push(json!({"step": "Search index", "ok": true, "detail": "Matches the stored text"}));
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&store.data_dir)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0);
    if mode != 0o700 {
        std::fs::set_permissions(&store.data_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| failed(e.into()))?;
        steps.push(json!({"step": "Folder privacy", "ok": true, "detail": format!("The data folder was readable by others ({mode:o}); now only you can open it")}));
    } else {
        steps.push(json!({"step": "Folder privacy", "ok": true, "detail": "Only you can open the data folder"}));
    }
    // Interrupted leases come back by themselves; failed jobs are retried once more.
    let mut tx = store.begin_write().await?;
    let revived = sqlx::query("UPDATE jobs SET state='ready', attempts=0, available_at=$1, lease_until=NULL, lease_owner=NULL WHERE state='failed' AND kind IN ('normalize','compile')")
        .bind(Ts::now())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    steps.push(json!({"step": "Stuck processing", "ok": true, "detail": if revived == 0 { "Nothing was stuck".to_owned() } else { format!("{revived} failed processing steps will run again (no AI runs)") }}));
    Ok(Json(json!({"steps": steps})))
}
async fn restart(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    owner(&store, &headers, true).await?;
    if !supervised() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This server wasn't started by the Mac's service manager, so it wouldn't come back after stopping".into(),
        ));
    }
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        RESTART.notify_one();
    });
    Ok(Json(json!({"restarting": true})))
}
