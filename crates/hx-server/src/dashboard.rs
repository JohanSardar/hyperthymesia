use crate::{ApiResult, auth, authorized_evidence};
use axum::{
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
        style {"body{font:16px system-ui;background:#10151e;color:#e8edf5;margin:2rem}a{color:#8fd3ff}.lane{margin:1rem 0}.track{height:2.5rem;background:#253143;position:relative;overflow:hidden}.bar{position:absolute;background:#255b87;height:100%;white-space:nowrap;overflow:hidden;font-size:12px;color:white}.axis{display:flex;justify-content:space-between}pre{white-space:pre-wrap;overflow-wrap:anywhere}.zoom:target .track{height:5rem}table{border-spacing:1rem .4rem}input{font:inherit}img{max-width:480px}"}
    } body {nav {a href="/" {"Today"} " · " a href="#lanes" {"Lanes"} " · " a href="#episodes" {"Episodes"} " · " a href="#tasks" {"Tasks"}}
        (body)
    }}}.into_string())
}
pub fn render_day(
    timeline: &Timeline,
    revisions: &[serde_json::Value],
    results: &[serde_json::Value],
) -> Html<String> {
    let width = timeline.window.len().num_milliseconds().max(1) as f64;
    shell(
        "Hyperthymesia day",
        html! {
            h1 {"Day " (timeline.window.start.date_naive()) " (UTC)"}
            p {"Observed lanes retain short switches. Gaps are unknown, not idle."}
            form method="get" {input name="q" placeholder="Search captured text"; button type="submit" {"Search"}}
            @for result in results {p {a href=(format!("/evidence/{}",result["id"])) {(result["text"].as_str().unwrap_or(""))}}}
            section id="lanes" class="zoom" {h2 {"Device and source lanes"}
                div class="axis" {@for hour in (0..=24).step_by(2) {span {(format!("{hour:02}:00"))}}}
                @for lane in &timeline.lanes {
                    div class="lane" {h3 {(&lane.id.key) " / " (&lane.id.kind)}
                        div class="track" {
                            @for span in &lane.intervals {
                                @let left=((span.interval.start-timeline.window.start).num_milliseconds() as f64/width*100.0).clamp(0.0,100.0);
                                @let size=(span.interval.len().num_milliseconds() as f64/width*100.0).clamp(0.05,100.0-left);
                                a class="bar" style=(format!("left:{left}%;width:{size}%")) title=(&span.label)
                                    href=(span.evidence.first().map(|id|format!("/evidence/{id}")).unwrap_or_else(||"#lanes".into())) {(&span.label)}
                            }
                        }
                    }
                }
                @for point in &timeline.points {p {(point.at.format("%H:%M:%S")) " " a href=(format!("/evidence/{}",point.evidence)) {(&point.label)}}}
                h3 {"Coverage"}
                @for span in &timeline.coverage {p {(&span.lane_key) " " (format!("{:?}",span.state)) " " (span.reason.as_deref().unwrap_or(""))}}
            }
            h2 {"Time per observed label"}
            table {@for (label,duration) in hx_compile::exposure_by_label(timeline,"focus") {tr {td {(label)} td {(duration.num_seconds()) " s"}}}}
            @for level in ["episodes","tasks"] {
                section id=(level) class="zoom" {h2 {(level)}
                    @for revision in revisions {p {"Revision " (revision["revision_id"])}
                        pre {(serde_json::to_string_pretty(&revision["interpretation"][level]).unwrap_or_default())}
                    }
                }
            }
        },
    )
}
#[derive(Default, Deserialize)]
pub struct DayQuery {
    q: Option<String>,
}
pub async fn today(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(q): Query<DayQuery>,
) -> ApiResult<Html<String>> {
    day(
        State(store),
        headers,
        Path(Utc::now().date_naive().to_string()),
        Query(q),
    )
    .await
}
pub async fn day(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(q): Query<DayQuery>,
) -> ApiResult<Html<String>> {
    let token = auth(&store, &headers, "read_raw").await?;
    let date = NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(crate::bad)?;
    let start = date
        .and_hms_opt(0, 0, 0)
        .ok_or(crate::bad("invalid day"))?
        .and_utc();
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
    let l3 = crate::timeline_value(&store, &token, w, "episodes").await?;
    let results = if let Some(q) = q.q {
        store.search(&q, true, &token.prefixes).await?
    } else {
        vec![]
    };
    Ok(render_day(
        &timeline,
        l3["revisions"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &results,
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
    Ok(shell(
        "Evidence",
        html! {h1 {"Evidence " (id)} pre {(serde_json::to_string_pretty(&value).unwrap_or_default())} @for src in thumbnails {img src=(src) alt="Captured still";}},
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
