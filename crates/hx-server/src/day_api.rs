//! Owner-only JSON for the app's day page, timeline and search. Read-only.
//!
//! `/v1/day` is the compiled timeline cut down to what a person reads: every
//! focus and presence span, attention merged into runs, coverage per collector,
//! points other than input/health pings, per-minute input counts and the AI's
//! canonical account. `/v1/cite` opens one cited record with what the AI was
//! shown for it and the screen text recorded during it. `/v1/find` searches the
//! accounts' prose and the full stored text of every record.
use crate::{ApiError, ApiResult, auth, bad};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::get,
};
use chrono::{DateTime, Duration, DurationRound, Utc};
use hx_compile::{Lane, LaneInterval};
use hx_schema::Interval;
use hx_store::{Store, Token};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn router() -> Router<Store> {
    Router::new()
        .route("/v1/days", get(days))
        .route("/v1/day", get(day))
        .route("/v1/cite/{id}", get(cite))
        .route("/v1/find", get(find))
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

/// Raw reads are the owner's; namespaces still apply to every record returned.
async fn owner(store: &Store, headers: &HeaderMap) -> ApiResult<Token> {
    auth(store, headers, "read_raw").await
}

async fn days(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let token = owner(&store, &headers).await?;
    let counts = store.day_counts(&token.prefixes).await?;
    let windows = store
        .window_states(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
            Utc::now() + Duration::days(2),
        )
        .await?;
    Ok(Json(json!({"buckets":counts,"windows":windows})))
}

#[derive(Deserialize)]
struct DayQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

fn device_of(lane_key: &str) -> &str {
    lane_key.strip_prefix("device:").unwrap_or(lane_key)
}
fn ms(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn text_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max - 1).collect::<String>())
    }
}
/// Screen text without the header lines the recorder adds in front of it.
fn screen_body(text: &str) -> &str {
    text.split_once("Screen text (top to bottom):\n")
        .map_or(text, |(_, rest)| rest)
}

fn span(i: &LaneInterval, kind: &str) -> Value {
    let a = &i.attrs;
    let mut out = json!({
        "start": ms(i.interval.start),
        "end": ms(i.interval.end),
        "label": i.label,
        "evidence": i.evidence,
        "uncertain": i.uncertain_edges,
    });
    for key in [
        "app_name",
        "window_title",
        "app_bundle_id",
        "url",
        "state",
        "basis",
    ] {
        if let Some(value) = text_of(a, key) {
            out[key] = json!(value);
        }
    }
    if kind == "attention" {
        // Inferred runs cite many input buckets; the first few are enough to open.
        out["evidence_count"] = json!(i.evidence.len());
        out["evidence"] = json!(i.evidence.iter().take(3).collect::<Vec<_>>());
    }
    out
}

/// Touching runs with the same label become one span. Only attention is merged:
/// it is inferred second by second, and every other lane keeps each span.
fn merged(lane: &Lane) -> Vec<LaneInterval> {
    let mut out: Vec<LaneInterval> = Vec::new();
    for i in &lane.intervals {
        match out.last_mut() {
            Some(last)
                if last.label == i.label
                    && i.interval.start <= last.interval.end + Duration::seconds(1) =>
            {
                last.interval.end = last.interval.end.max(i.interval.end);
                last.uncertain_edges |= i.uncertain_edges;
                last.evidence.extend(&i.evidence);
            }
            _ => out.push(i.clone()),
        }
    }
    out
}

async fn day(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<DayQuery>,
) -> ApiResult<Json<Value>> {
    let token = owner(&store, &headers).await?;
    if q.to <= q.from || q.to - q.from > Duration::hours(50) {
        return Err(bad("range must be positive and at most 50 hours"));
    }
    let w = Interval {
        start: q.from,
        end: q.to,
    };
    let observations: Vec<_> = store
        .observations(w, true)
        .await?
        .into_iter()
        .filter(|o| token.allows(o.envelope.connector_instance.as_str()))
        .collect();
    let observed_through = observations
        .iter()
        .map(|o| o.envelope.time.observed_at)
        .max();
    let timeline = hx_compile::compile(&observations, w);
    // Merged devices show as the one they were merged into, by the owner's name.
    let aliases = crate::admin_api::device_aliases(&store).await?;
    let labels = crate::admin_api::device_labels(&store).await?;
    let shown = |lane_key: &str| -> String {
        let device = device_of(lane_key);
        aliases
            .get(device)
            .cloned()
            .unwrap_or_else(|| device.to_owned())
    };
    let label = |device: &str| -> String {
        labels
            .get(device)
            .cloned()
            .unwrap_or_else(|| device.to_owned())
    };
    let by_id: BTreeMap<i64, &hx_compile::Observation> =
        observations.iter().map(|o| (o.id, o)).collect();

    let coverage: Vec<Value> = timeline
        .coverage
        .iter()
        .map(|c| {
            // Reasons read `<connector>[<collector>]:<what>`.
            let reason = c.reason.as_deref().unwrap_or("");
            let (connector, rest) = reason.split_once('[').unwrap_or(("", reason));
            let collector = rest.split_once(']').map_or("", |(name, _)| name);
            json!({
                "device": shown(&c.lane_key),
                "device_name": label(&shown(&c.lane_key)),
                "connector": connector,
                "collector": collector,
                "state": c.state,
                "start": ms(c.interval.start),
                "end": ms(c.interval.end),
            })
        })
        .collect();

    let lanes: Vec<Value> = timeline
        .lanes
        .iter()
        .map(|lane| {
            let kind = lane.id.kind.as_str();
            let spans: Vec<Value> = if kind == "attention" {
                merged(lane).iter().map(|i| span(i, kind)).collect()
            } else {
                lane.intervals.iter().map(|i| span(i, kind)).collect()
            };
            (shown(&lane.id.key), kind.to_owned(), spans)
        })
        .fold(
            Vec::<(String, String, Vec<Value>)>::new(),
            |mut out, (device, kind, spans)| {
                match out.iter_mut().find(|(d, k, _)| *d == device && *k == kind) {
                    Some(existing) => existing.2.extend(spans),
                    None => out.push((device, kind, spans)),
                }
                out
            },
        )
        .into_iter()
        .map(|(device, kind, mut spans)| {
            spans.sort_by(|a, b| a["start"].as_i64().cmp(&b["start"].as_i64()));
            json!({"device_name": label(&device), "device": device, "kind": kind, "spans": spans})
        })
        .collect();

    let mut points = vec![];
    let mut activity: BTreeMap<DateTime<Utc>, [u64; 3]> = BTreeMap::new();
    for point in &timeline.points {
        let observation = by_id.get(&point.evidence);
        if point.kind == "input.bucket" {
            // Counts only: the recorder never stores which keys were pressed.
            if let Some(o) = observation {
                let p = &o.envelope.payload;
                let minute = point
                    .at
                    .duration_trunc(Duration::minutes(1))
                    .unwrap_or(point.at);
                let slot = activity.entry(minute).or_default();
                slot[0] += p["keys"].as_u64().unwrap_or(0);
                slot[1] += p["clicks"].as_u64().unwrap_or(0);
                slot[2] += p["scroll_events"].as_u64().unwrap_or(0);
            }
            continue;
        }
        if matches!(
            point.kind.as_str(),
            "collector.health" | "clock.calibration"
        ) {
            continue;
        }
        let mut out = json!({
            "at": ms(point.at),
            "kind": point.kind,
            "label": point.label,
            "evidence": point.evidence,
            "device": shown(&point.lane_key),
        });
        if let Some(o) = observation {
            let p = &o.envelope.payload;
            for key in [
                "app_name",
                "window_title",
                "app_bundle_id",
                "surface",
                "source",
                "label",
                "role",
            ] {
                if let Some(value) = text_of(p, key) {
                    out[key] = json!(clip(&value, 200));
                }
            }
            if let Some(text) = p["text"].as_str() {
                out["chars"] = json!(text.chars().count());
                out["preview"] = json!(clip(screen_body(text), 160));
            }
        }
        points.push(out);
    }
    let activity: Vec<Value> = activity
        .into_iter()
        .map(|(at, [keys, clicks, scrolls])| json!([ms(at), keys, clicks, scrolls]))
        .collect();

    let revisions: Vec<Value> = store
        .canonical_revisions(w.start, w.end, &token.prefixes)
        .await?
        .into_iter()
        .filter(|row| row.input_namespaces.iter().all(|n| token.allows(n)))
        .map(|row| {
            let mut interpretation = row.interpretation;
            let model = interpretation["input_manifest"]["model"].clone();
            if let Some(object) = interpretation.as_object_mut() {
                object.remove("input_manifest");
            }
            json!({"revision_id":row.id,"has_agent_run":row.has_agent_run,"model":model,"interpretation":interpretation})
        })
        .collect();
    let windows = store.window_states(w.start, w.end).await?;
    Ok(Json(json!({
        "window": {"start": ms(w.start), "end": ms(w.end)},
        "observed_through": observed_through,
        "coverage": coverage,
        "lanes": lanes,
        "points": points,
        "activity": activity,
        "aggregates": timeline.aggregates,
        "revisions": revisions,
        "windows": windows,
    })))
}

#[derive(Deserialize)]
struct CiteQuery {
    revision: Option<i64>,
}

/// Ids a digest line lists in its leading `[...]`. Elided runs (`…`) list only
/// their ends, so an id hidden inside one is not claimed.
fn digest_ids(line: &str) -> Vec<i64> {
    let Some(rest) = line.strip_prefix('[') else {
        return vec![];
    };
    let Some((ids, _)) = rest.split_once(']') else {
        return vec![];
    };
    ids.split(',')
        .filter_map(|part| part.split_whitespace().next()?.parse().ok())
        .collect()
}

async fn cite(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<CiteQuery>,
) -> ApiResult<Json<Value>> {
    let token = owner(&store, &headers).await?;
    let record = crate::authorized_evidence(&store, &token, id).await?;
    let envelope = &record["envelope"];
    let kind = envelope["kind"].as_str().unwrap_or("");

    // What the AI was given for this record, exactly as stored with its run.
    let mut seen = Value::Null;
    if let Some(revision) = q.revision {
        match crate::dashboard::authorized_run(&store, &headers, revision).await {
            Ok(run) => {
                let digest = run["digest"].as_str().unwrap_or("");
                let lines: Vec<&str> = digest
                    .lines()
                    .filter(|line| digest_ids(line).contains(&id))
                    .take(400)
                    .collect();
                let snapshot = run["evidence"][id.to_string()]["text"].clone();
                seen = json!({"revision": revision, "digest_lines": lines, "snapshot": snapshot, "has_digest": !digest.is_empty()});
            }
            Err(ApiError(StatusCode::NOT_FOUND, _)) => {
                seen = json!({"revision": revision, "unavailable": true});
            }
            Err(error) => return Err(error),
        }
    }

    // Spans say what was in front; the text records made during them say what it read.
    let mut screen_text = vec![];
    let mut screen_text_total = 0;
    if kind != "text.snapshot" {
        let start = envelope["time"]["source_start"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        let end = envelope["time"]["source_end"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        if let (Some(start), Some(end), Some(device)) = (start, end, envelope["device"].as_str())
            && end > start
            && end - start <= Duration::days(1)
        {
            for row in store.screen_text_between(start, end, device, 60).await? {
                let connector = row.envelope["connector_instance"].as_str().unwrap_or("");
                if !connector.is_empty() && !token.allows(connector) {
                    continue;
                }
                screen_text_total += 1;
                screen_text.push(json!({"id": row.id, "envelope": row.envelope}));
            }
        }
    }
    Ok(Json(json!({
        "record": record,
        "seen": seen,
        "screen_text": screen_text,
        "screen_text_total": screen_text_total,
    })))
}

#[derive(Deserialize)]
struct FindQuery {
    q: String,
}

fn entry_hit(
    revision: i64,
    window: &Value,
    kind: &str,
    item: &Value,
    terms: &[String],
) -> Option<Value> {
    let mut haystack = String::new();
    for key in ["title", "description", "goal"] {
        if let Some(text) = item[key].as_str() {
            haystack.push_str(text);
            haystack.push('\n');
        }
    }
    for claim in item["claims"].as_array().into_iter().flatten() {
        for key in ["action", "object"] {
            haystack.push_str(claim[key].as_str().unwrap_or(""));
            haystack.push(' ');
        }
    }
    let lower = haystack.to_lowercase();
    if !terms.iter().all(|t| lower.contains(t.as_str())) {
        return None;
    }
    let intervals = item["intervals"].as_array();
    let start = intervals
        .and_then(|v| v.iter().filter_map(|i| i["start"].as_str()).min())
        .or_else(|| window["start"].as_str());
    let end = intervals
        .and_then(|v| v.iter().filter_map(|i| i["end"].as_str()).max())
        .or_else(|| window["end"].as_str());
    Some(json!({
        "kind": kind,
        "revision_id": revision,
        "id": item["id"],
        "title": item["title"],
        "description": item["description"],
        "goal": item["goal"],
        "start": start,
        "end": end,
        "evidence": item["evidence"].as_array().map(|v| v.iter().take(5).cloned().collect::<Vec<_>>()),
    }))
}

/// The stretch of `text` around the first term, so a hit shows why it matched.
fn snippet(text: &str, terms: &[String]) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t.as_str()))
        .min()
        .unwrap_or(0);
    // `lower` can differ in byte length; map back by characters.
    let chars_before = lower[..at].chars().count();
    let chars: Vec<char> = flat.chars().collect();
    let from = chars_before.saturating_sub(80);
    let to = (chars_before + 200).min(chars.len());
    let mut out: String = chars[from..to].iter().collect();
    if from > 0 {
        out.insert(0, '…');
    }
    if to < chars.len() {
        out.push('…');
    }
    out
}

async fn find(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<FindQuery>,
) -> ApiResult<Json<Value>> {
    let token = owner(&store, &headers).await?;
    let query = q.q.trim();
    if query.is_empty() || query.len() > 500 {
        return Err(bad("q must be 1 to 500 bytes"));
    }
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|t| t.trim_matches('"').to_owned())
        .filter(|t| !t.is_empty())
        .collect();

    let mut entries = vec![];
    let rows = store
        .canonical_revisions(
            DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
            Utc::now() + Duration::days(2),
            &token.prefixes,
        )
        .await?;
    for row in rows.iter().rev() {
        if !row.input_namespaces.iter().all(|n| token.allows(n)) {
            continue;
        }
        let i = &row.interpretation;
        for (kind, key) in [("task", "tasks"), ("episode", "episodes")] {
            for item in i[key].as_array().into_iter().flatten() {
                if let Some(hit) = entry_hit(row.id, &i["window"], kind, item, &terms) {
                    entries.push(hit);
                }
            }
        }
        if let Some(summary) = i["summary"].as_str() {
            let lower = summary.to_lowercase();
            if terms.iter().all(|t| lower.contains(t.as_str())) {
                entries.push(json!({"kind":"summary","revision_id":row.id,"title":summary,"start":i["window"]["start"],"end":i["window"]["end"]}));
            }
        }
        if entries.len() >= 100 {
            break;
        }
    }
    entries.sort_by(|a, b| b["start"].as_str().cmp(&a["start"].as_str()));
    entries.truncate(100);

    let hits = store.search(query, true, &token.prefixes).await?;
    let ids: Vec<i64> = hits.iter().filter_map(|h| h["id"].as_i64()).collect();
    let briefs: BTreeMap<i64, hx_store::RecordBrief> = store
        .record_briefs(&ids)
        .await?
        .into_iter()
        .map(|b| (b.id, b))
        .collect();
    let records: Vec<Value> = hits
        .iter()
        .filter_map(|h| {
            let id = h["id"].as_i64()?;
            // The store's prefix match doesn't stop at a namespace boundary.
            let brief = briefs
                .get(&id)
                .filter(|b| token.allows(&b.connector_instance))?;
            Some(json!({
                "id": id,
                "at": h["at"],
                "kind": brief.kind,
                "connector_instance": brief.connector_instance,
                "app": brief.app,
                "title": brief.title,
                "snippet": snippet(h["text"].as_str().unwrap_or(""), &terms),
            }))
        })
        .collect();
    Ok(Json(json!({"entries": entries, "records": records})))
}
