use crate::{ApiResult, auth, authorized_evidence, disclosure::DisclosurePolicy};
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Html,
};
use chrono::{Duration, NaiveDate, Utc};
use hx_compile::Timeline;
use hx_schema::Interval;
use hx_store::Store;
use maud::{DOCTYPE, Markup, html};
use serde::Deserialize;

fn shell(title: &str, body: Markup) -> Html<String> {
    Html(html!{(DOCTYPE) html {head { meta charset="utf-8"; meta name="viewport" content="width=device-width, initial-scale=1";
        meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; form-action 'self'; base-uri 'none'";
        title {(title)}
        style {"body{font:16px system-ui;background:#10151e;color:#e8edf5;margin:2rem}a{color:#8fd3ff}.lane{margin:1rem 0}.entry{display:flex;gap:1rem;margin:.6rem 0}.when{font-variant-numeric:tabular-nums;color:#9fb0c8;min-width:7.5rem}.desc{margin:.2rem 0;color:#b9c6d8;font-size:14px}.summary{color:#cfd8e6}summary{cursor:pointer;margin:1rem 0}.track{height:2.5rem;background:#253143;position:relative;overflow:hidden}.bar{position:absolute;background:#255b87;height:100%;white-space:nowrap;overflow:hidden;font-size:12px;color:white}.axis{display:flex;justify-content:space-between}pre{white-space:pre-wrap;overflow-wrap:anywhere}.zoom:target .track{height:5rem}table{border-spacing:1rem .4rem}input{font:inherit}img{max-width:480px}"}
    } body {nav {a href="/" {"Today"} " · " a href="#episodes" {"What you did"} " · " a href="#tasks" {"Tasks"} " · " a href="#lanes" {"Lanes"}}
        (body)
    }}}.into_string())
}
/// One episode or task shown in the owner's local time.
struct Entry<'a> {
    start: chrono::DateTime<chrono::FixedOffset>,
    end: chrono::DateTime<chrono::FixedOffset>,
    title: &'a str,
    description: &'a str,
    evidence: Option<i64>,
}

fn entries<'a>(
    revisions: &'a [serde_json::Value],
    level: &str,
    day: Interval,
    zone: chrono::FixedOffset,
) -> Vec<Entry<'a>> {
    let mut out: Vec<Entry> = revisions
        .iter()
        .flat_map(|r| r["interpretation"][level].as_array().into_iter().flatten())
        .filter_map(|item| {
            let intervals: Vec<Interval> =
                serde_json::from_value(item["intervals"].clone()).ok()?;
            let start = intervals.iter().map(|i| i.start).min()?;
            let end = intervals.iter().map(|i| i.end).max()?;
            (start < day.end && end > day.start).then(|| Entry {
                start: start.with_timezone(&zone),
                end: end.with_timezone(&zone),
                title: item["title"].as_str().unwrap_or(""),
                description: item["description"].as_str().unwrap_or(""),
                evidence: item["evidence"][0].as_i64(),
            })
        })
        .collect();
    out.sort_by_key(|e| e.start);
    out
}

fn zone_label(zone: chrono::FixedOffset) -> String {
    let minutes = zone.local_minus_utc() / 60;
    let sign = if minutes < 0 { '−' } else { '+' };
    format!(
        "UTC{sign}{:02}:{:02}",
        minutes.abs() / 60,
        minutes.abs() % 60
    )
}

pub fn render_day(
    timeline: &Timeline,
    revisions: &[serde_json::Value],
    results: &[serde_json::Value],
    zone: chrono::FixedOffset,
) -> Html<String> {
    let width = timeline.window.len().num_milliseconds().max(1) as f64;
    let local_start = timeline.window.start.with_timezone(&zone);
    let episodes = entries(revisions, "episodes", timeline.window, zone);
    let tasks = entries(revisions, "tasks", timeline.window, zone);
    shell(
        "Hyperthymesia day",
        html! {
            h1 {(local_start.format("%A, %B %-d, %Y")) " " small {"(" (zone_label(zone)) ")"}}
            form method="get" {input name="q" placeholder="Search captured text"; button type="submit" {"Search"}}
            @for result in results {p {a href=(format!("/evidence/{}",result["id"])) {(result["text"].as_str().unwrap_or(""))}}}
            @for revision in revisions {
                @if let Some(summary) = revision["interpretation"]["summary"].as_str().filter(|s| !s.is_empty()) {
                    @if let (Ok(start), Ok(end)) = (
                        serde_json::from_value::<chrono::DateTime<Utc>>(revision["interpretation"]["window"]["start"].clone()),
                        serde_json::from_value::<chrono::DateTime<Utc>>(revision["interpretation"]["window"]["end"].clone()),
                    ) {
                        p class="summary" {b {(start.with_timezone(&zone).format("%H:%M")) "–" (end.with_timezone(&zone).format("%H:%M")) " "} (summary)}
                    }
                }
            }
            section id="episodes" {h2 {"What you did"}
                @if episodes.is_empty() {p {"No replica for this day yet."}}
                @for e in &episodes {
                    div class="entry" {
                        span class="when" {(e.start.format("%H:%M")) "–" (e.end.format("%H:%M"))}
                        div {
                            @match e.evidence {
                                Some(id) => a href=(format!("/evidence/{id}")) {(e.title)},
                                None => b {(e.title)},
                            }
                            @if !e.description.is_empty() && e.description != e.title {p class="desc" {(e.description)}}
                        }
                    }
                }
            }
            section id="tasks" {h2 {"Tasks"}
                @for t in &tasks {
                    div class="entry" {span class="when" {(t.start.format("%H:%M")) "–" (t.end.format("%H:%M"))} div {b {(t.title)}}}
                }
            }
            details id="lanes" {summary {"Device and source lanes (raw capture; gaps are unknown, not idle)"}
                div class="axis" {@for hour in (0..=24).step_by(2) {span {(format!("{hour:02}:00"))}}}
                @for lane in &timeline.lanes {
                    div class="lane" {h3 {(&lane.id.key) " / " (&lane.id.kind)}
                        div class="track" {
                            @for span in &coalesced(&lane.intervals) {
                                @let left=((span.interval.start-timeline.window.start).num_milliseconds() as f64/width*100.0).clamp(0.0,100.0);
                                @let size=(span.interval.len().num_milliseconds() as f64/width*100.0).max(0.05).min(100.0-left);
                                a class="bar" style=(format!("left:{left}%;width:{size}%")) title=(&span.label)
                                    href=(span.evidence.first().map(|id|format!("/evidence/{id}")).unwrap_or_else(||"#lanes".into())) {(&span.label)}
                            }
                        }
                    }
                }
                h3 {"Points"}
                // Input buckets and health pings are already drawn as lanes.
                @for point in timeline.points.iter().filter(|p| !matches!(p.kind.as_str(), "input.bucket" | "collector.health")) {p {(point.at.with_timezone(&zone).format("%H:%M:%S")) " " a href=(format!("/evidence/{}",point.evidence)) {(&point.label)}}}
                h3 {"Coverage"}
                @for span in &timeline.coverage {p {(&span.lane_key) " " (format!("{:?}",span.state)) " " (span.reason.as_deref().unwrap_or(""))}}
                h3 {"Time per observed label"}
                table {@for (label,duration) in hx_compile::exposure_by_label(timeline,"focus") {tr {td {(label)} td {(duration.num_seconds()) " s"}}}}
            }
            details {summary {"Replica revisions (raw)"}
                @for revision in revisions {p {"Revision " (revision["revision_id"])}
                    @if revision["has_agent_run"] == true {
                        p {a href=(format!("/revision/{}/run",revision["revision_id"])) {"Agent tool-call log"}}
                    }
                    p {"Evidence: "
                        @for id in crate::mcp::revision_evidence(&revision["interpretation"]) {
                            a href=(if revision["has_agent_run"] == true {format!("/revision/{}/evidence/{id}",revision["revision_id"])} else {format!("/evidence/{id}")}) {(id)} " "
                        }
                    }
                    pre {(serde_json::to_string_pretty(&revision["interpretation"]).unwrap_or_default())}
                }
            }
        },
    )
}

/// Touching spans with the same label drawn as one bar, linked to the first
/// span's evidence. Only the picture is merged; lanes keep every span.
fn coalesced(spans: &[hx_compile::LaneInterval]) -> Vec<hx_compile::LaneInterval> {
    let mut out: Vec<hx_compile::LaneInterval> = Vec::new();
    for span in spans {
        match out.last_mut() {
            Some(last)
                if last.label == span.label
                    && span.interval.start >= last.interval.start
                    && span.interval.start - last.interval.end <= Duration::seconds(1) =>
            {
                last.interval.end = last.interval.end.max(span.interval.end);
            }
            _ => out.push(span.clone()),
        }
    }
    out
}

/// The owner's current UTC offset, from the newest capture that recorded one.
async fn owner_zone(store: &Store) -> ApiResult<chrono::FixedOffset> {
    let minutes: Option<i32> = store.owner_utc_offset().await?;
    let utc = chrono::FixedOffset::east_opt(0).expect("zero offset");
    Ok(chrono::FixedOffset::east_opt(minutes.unwrap_or(0) * 60).unwrap_or(utc))
}
#[derive(Default, Deserialize)]
pub struct DayQuery {
    q: Option<String>,
}
pub async fn today(
    State(store): State<Store>,
    Extension(policy): Extension<DisclosurePolicy>,
    headers: HeaderMap,
    Query(q): Query<DayQuery>,
) -> ApiResult<Html<String>> {
    let zone = owner_zone(&store).await?;
    day(
        State(store),
        Extension(policy),
        headers,
        Path(Utc::now().with_timezone(&zone).date_naive().to_string()),
        Query(q),
    )
    .await
}
pub async fn day(
    State(store): State<Store>,
    Extension(policy): Extension<DisclosurePolicy>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(q): Query<DayQuery>,
) -> ApiResult<Html<String>> {
    let token = auth(&store, &headers, "read_raw").await?;
    let date = NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(crate::bad)?;
    let zone = owner_zone(&store).await?;
    let start = date
        .and_hms_opt(0, 0, 0)
        .ok_or(crate::bad("invalid day"))?
        .and_local_timezone(zone)
        .single()
        .ok_or(crate::bad("invalid day"))?
        .with_timezone(&Utc);
    let w = Interval {
        start,
        end: start + Duration::days(1),
    };
    let observations: Vec<_> = store
        .observations(w, true)
        .await?
        .into_iter()
        .filter(|o| token.allows(o.envelope.connector_instance.as_str()))
        .collect();
    let timeline = hx_compile::compile(&observations, w);
    let l3 = crate::timeline_value(&store, &token, &policy, w, "episodes").await?;
    let results = if let Some(q) = q.q {
        store.search(&q, true, &token.prefixes).await?
    } else {
        vec![]
    };
    Ok(render_day(
        &timeline,
        l3["revisions"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &results,
        zone,
    ))
}
pub async fn evidence_page(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Html<String>> {
    let token = auth(&store, &headers, "read_raw").await?;
    let value = authorized_evidence(&store, &token, id).await?;
    // Embedded images must be verified raster formats; never render captured SVG/HTML.
    let mut thumbnails = vec![];
    if value["envelope"]["kind"] == "media.still"
        && let Some(blobs) = value["blobs"].as_array()
    {
        use base64::Engine;
        for blob in blobs {
            if blob["evidence_expired"] == true {
                continue;
            }
            if let Some(sha) = blob["sha256"].as_str()
                && let Ok(bytes) = tokio::fs::read(store.blob_path(sha)?).await
            {
                let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    "image/png"
                } else if bytes.starts_with(b"\xff\xd8\xff") {
                    "image/jpeg"
                } else {
                    continue;
                };
                thumbnails.push(format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                ));
            }
        }
    }
    let mut ocr_text = Vec::new();
    if let Some(derived) = value["derived_evidence"].as_array() {
        for item in derived {
            if let Some(lines) = item["envelope"]["payload"]["ocr"]["lines"].as_array() {
                ocr_text.push((
                    item["id"].as_i64().unwrap_or_default(),
                    lines
                        .iter()
                        .filter_map(|l| l["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ));
            }
        }
    }
    Ok(shell(
        "Evidence",
        html! {h1 {"Evidence " (id)} pre {(serde_json::to_string_pretty(&value).unwrap_or_default())} @for src in thumbnails {img src=(src) alt="Captured still";} @for (ocr_id, text) in ocr_text { section {h2 {a href=(format!("/evidence/{ocr_id}")) {"OCR text"}} pre {(text)}} }},
    ))
}

pub async fn login() -> Html<String> {
    shell(
        "Sign in",
        html! {h1 {"Hyperthymesia"} form method="post" action="/login" {
            label for="token" {"Device token"} input type="password" name="token" id="token" required;
            button type="submit" {"Sign in"}
        }},
    )
}
#[derive(Deserialize)]
pub struct Credentials {
    token: String,
}
pub async fn sign_in(
    State(store): State<Store>,
    axum::Form(credentials): axum::Form<Credentials>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    if !hx_store::valid_hash(&credentials.token)
        || store.authenticate(&credentials.token).await?.is_none()
    {
        return Err(crate::ApiError(
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid token".into(),
        ));
    }
    let cookie = format!(
        "hx_token={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200",
        credentials.token
    );
    Ok((
        [(axum::http::header::SET_COOKIE, cookie)],
        axum::response::Redirect::to("/"),
    )
        .into_response())
}

/// Audit prose is indivisible: hide it when any input namespace or policy changed.
async fn authorized_run(
    store: &Store,
    headers: &HeaderMap,
    revision: i64,
) -> ApiResult<serde_json::Value> {
    let token = auth(store, headers, "read_raw").await?;
    let row = store.revision_run(revision).await?.ok_or(crate::ApiError(
        axum::http::StatusCode::NOT_FOUND,
        "run not found".into(),
    ))?;
    let namespaces: Vec<String> = row.input_namespaces;
    let interpretation: serde_json::Value = row.interpretation;
    let broker = crate::mcp::Broker::new(
        store.clone(),
        crate::disclosure::DisclosurePolicy::from_env()?,
        crate::processing::SensitiveEgress::from_env(),
        hx_store::window_at(Utc::now()),
    );
    if namespaces.iter().any(|ns| !token.allows(ns))
        || interpretation["input_manifest"]["disclosure_policy"]["digest"] != broker.policy.digest()
        || interpretation["input_manifest"]["sensitive_egress"]
            != serde_json::json!(broker.sensitive)
    {
        return Err(crate::ApiError(
            axum::http::StatusCode::NOT_FOUND,
            "run unavailable under current policy".into(),
        ));
    }
    let value: serde_json::Value = row.agent_run;
    for id in value["evidence"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.keys())
        .filter_map(|s| s.parse::<i64>().ok())
    {
        if !broker.allowed_id(id).await? {
            return Err(crate::ApiError(
                axum::http::StatusCode::NOT_FOUND,
                "run unavailable under current policy".into(),
            ));
        }
    }
    Ok(value)
}
pub async fn revision_evidence(
    State(store): State<Store>,
    headers: HeaderMap,
    Path((revision, id)): Path<(i64, i64)>,
) -> ApiResult<Html<String>> {
    let run = authorized_run(&store, &headers, revision).await?;
    let snapshot = run["evidence"][id.to_string()].clone();
    if snapshot.is_null() {
        return Err(crate::ApiError(
            axum::http::StatusCode::NOT_FOUND,
            "evidence not seen in this run".into(),
        ));
    }
    let projection: serde_json::Value =
        serde_json::from_str(snapshot["text"].as_str().unwrap_or("")).unwrap_or_default();
    let mut links: Vec<String> = projection["links"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|link| link["target"].as_str().map(str::to_owned))
        .collect();
    for key in ["url", "commit_url", "source_url"] {
        if let Some(url) = projection["payload"][key].as_str() {
            links.push(url.to_owned());
        }
    }
    links.retain(|url| {
        ["https://", "http://", "t3://", "codex://", "claude://"]
            .iter()
            .any(|scheme| url.starts_with(scheme))
    });
    links.sort();
    links.dedup();
    Ok(shell(
        "Agent evidence",
        html! { h1 {"Evidence " (id) " seen in revision " (revision)}
            p {a href=(format!("/revision/{revision}/run")) {"Agent tool-call log"} " · " a href=(format!("/evidence/{id}")) {"Original stored record and source links"}}
            @for link in links {p {a href=(link.as_str()) rel="noreferrer noopener" {(link)}}}
            pre {(serde_json::to_string_pretty(&snapshot).unwrap_or_default())}
        },
    ))
}
pub async fn run_page(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(revision): Path<i64>,
) -> ApiResult<Html<String>> {
    let run = authorized_run(&store, &headers, revision).await?;
    Ok(shell(
        "Agent run",
        html! {h1 {"Agent run for revision " (revision)}
            p {"fx " (run["fx_version"]) " / " (run["model"]) " / effort " (run["effort"]) " / fast " (run["fast"])}
            @for (id,_) in run["evidence"].as_object().into_iter().flatten() {
                a href=(format!("/revision/{revision}/evidence/{id}")) {"Evidence " (id)} " "
            }
            h2 {"Tool calls and exact results"} pre {(serde_json::to_string_pretty(&run["calls"]).unwrap_or_default())}
            h2 {"fx completion log"} pre {(serde_json::to_string_pretty(&run["responses"]).unwrap_or_default())}
        },
    ))
}
