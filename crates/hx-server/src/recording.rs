//! Pause and never-record: one durable record read by the Mac app and the web
//! app, and changed by both. The Mac keeps its own copy so a pause holds while
//! the server is unreachable; this record is what both sides agree on.
use crate::{ApiError, ApiResult, auth, bad, preferences};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    routing::{get, put},
};
use chrono::{DateTime, Duration, Utc};
use hx_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct App {
    pub bundle_id: String,
    pub name: String,
}
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct NeverRecord {
    pub apps: Vec<App>,
    /// Hosts such as `bank.example`; each also covers its subdomains.
    pub sites: Vec<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Controls {
    paused_until: Option<DateTime<Utc>>,
    never_record: NeverRecord,
    changed_at: Option<DateTime<Utc>>,
}

pub fn router() -> Router<Store> {
    Router::new()
        .route("/v1/recording", get(read))
        .route("/v1/recording/pause", put(pause))
        .route("/v1/recording/never-record", put(never_record))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

/// The paired Mac (an ingest token) or the owner signed in to the web app.
async fn allowed(store: &Store, headers: &HeaderMap, write: bool) -> ApiResult<()> {
    if headers.contains_key(header::AUTHORIZATION) && auth(store, headers, "ingest").await.is_ok() {
        return Ok(());
    }
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

async fn load(store: &Store) -> anyhow::Result<Controls> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM recording_controls WHERE id=1")
            .fetch_optional(&store.pool)
            .await?;
    Ok(value
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default())
}
async fn save(store: &Store, controls: &Controls) -> anyhow::Result<()> {
    let mut tx = store.begin_write().await?;
    sqlx::query("INSERT INTO recording_controls(id,value) VALUES(1,$1) ON CONFLICT(id) DO UPDATE SET value=excluded.value")
        .bind(serde_json::to_string(controls)?).execute(&mut *tx).await?;
    tx.commit().await
}

async fn view(store: &Store, controls: &Controls) -> ApiResult<Value> {
    let now = Utc::now();
    // Content-free markers the Mac turns into desktop notifications.
    let ready: Option<(i64, i64)> = sqlx::query_as(
        "SELECT start_at,end_at FROM windows WHERE canonical_revision IS NOT NULL ORDER BY start_at DESC LIMIT 1",
    )
    .fetch_optional(&store.pool)
    .await?;
    let failed_pass: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind='interpret' AND state='failed' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&store.pool)
    .await?;
    let failed_pull: Option<(i64, String)> = sqlx::query_as(
        "SELECT id,recipe_id FROM pull_history WHERE status='failed' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&store.pool)
    .await?;
    let micros = |v: i64| DateTime::<Utc>::from_timestamp_micros(v);
    Ok(json!({
        "enabled": preferences::load(store).await?.mac_recording,
        "paused_until": controls.paused_until.filter(|until| *until > now),
        "never_record": controls.never_record,
        "changed_at": controls.changed_at,
        "notices": {
            "account_ready": ready.map(|(start, end)| json!({"start": micros(start), "end": micros(end)})),
            "failed_pass": failed_pass,
            "failed_pull": failed_pull.map(|(id, recipe)| json!({"id": id, "recipe_id": recipe})),
        },
    }))
}

async fn read(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    allowed(&store, &headers, false).await?;
    let controls = load(&store).await?;
    Ok(Json(view(&store, &controls).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pause {
    /// `null` resumes recording.
    until: Option<DateTime<Utc>>,
}
async fn pause(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Pause>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    allowed(&store, &headers, true).await?;
    let Json(Pause { until }) = request.map_err(|_| bad("send {\"until\": time or null}"))?;
    let now = Utc::now();
    if until.is_some_and(|until| until > now + Duration::days(8)) {
        return Err(bad("a pause can last at most a week"));
    }
    let mut controls = load(&store).await?;
    controls.paused_until = until.filter(|until| *until > now);
    controls.changed_at = Some(now);
    save(&store, &controls).await?;
    Ok(Json(view(&store, &controls).await?))
}

async fn never_record(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<NeverRecord>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    allowed(&store, &headers, true).await?;
    let Json(rules) = request.map_err(|_| bad("send {\"apps\": [...], \"sites\": [...]}"))?;
    let rules = normalize(rules).map_err(bad)?;
    let mut controls = load(&store).await?;
    controls.never_record = rules;
    controls.changed_at = Some(Utc::now());
    save(&store, &controls).await?;
    Ok(Json(view(&store, &controls).await?))
}

/// Accepts what a person pastes (`https://www.Bank.example/login`) and keeps
/// the host (`bank.example`). Duplicates are dropped, order is kept.
fn normalize(rules: NeverRecord) -> Result<NeverRecord, String> {
    if rules.apps.len() > 200 || rules.sites.len() > 500 {
        return Err("too many never-record entries".into());
    }
    let mut out = NeverRecord::default();
    for app in rules.apps {
        let id = app.bundle_id.trim().to_owned();
        if id.is_empty()
            || id.len() > 255
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        {
            return Err(format!("not an app bundle id: {:?}", app.bundle_id));
        }
        let name = app
            .name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect::<String>();
        if !out.apps.iter().any(|a| a.bundle_id == id) {
            out.apps.push(App {
                name: if name.is_empty() { id.clone() } else { name },
                bundle_id: id,
            });
        }
    }
    for site in rules.sites {
        let host = site_host(&site).ok_or_else(|| format!("not a site: {site:?}"))?;
        if !out.sites.contains(&host) {
            out.sites.push(host);
        }
    }
    Ok(out)
}
fn site_host(input: &str) -> Option<String> {
    let input = input.trim();
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let host = url::Url::parse(&with_scheme)
        .ok()?
        .host_str()?
        .to_ascii_lowercase();
    let host = host
        .trim_start_matches("www.")
        .trim_end_matches('.')
        .to_owned();
    (host.contains('.') || host == "localhost")
        .then_some(host)
        .filter(|h| h.len() <= 253)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_sites_become_hosts_and_bad_entries_are_refused() {
        let rules = normalize(NeverRecord {
            apps: vec![App {
                bundle_id: "com.apple.Safari".into(),
                name: "".into(),
            }],
            sites: vec![
                "https://www.Bank.example/login?x=1".into(),
                "bank.example".into(),
                "mail.example.org/".into(),
            ],
        })
        .unwrap();
        assert_eq!(rules.sites, ["bank.example", "mail.example.org"]);
        assert_eq!(rules.apps[0].name, "com.apple.Safari");
        assert!(
            normalize(NeverRecord {
                apps: vec![],
                sites: vec!["not a site".into()]
            })
            .is_err()
        );
        assert!(
            normalize(NeverRecord {
                apps: vec![App {
                    bundle_id: "com.x; rm".into(),
                    name: "x".into()
                }],
                sites: vec![]
            })
            .is_err()
        );
    }
}
