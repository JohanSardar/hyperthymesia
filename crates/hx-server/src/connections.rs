//! Server-rendered owner controls: no scripts, credential echo, or model calls.
use crate::{ApiResult, dashboard::shell, sources};
use axum::{
    Form, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use hx_store::Store;
use maud::html;
use serde::Deserialize;

#[derive(Default, Deserialize)]
pub struct ConnectQuery {
    template: Option<String>,
    connected: Option<String>,
}

pub async fn page(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(query): Query<ConnectQuery>,
) -> ApiResult<Response> {
    let Json(configured) = sources::list(State(store), headers).await?;
    let (name, description) = match query.template.as_deref() {
        Some("github") => (
            "GitHub",
            "My GitHub work. Use https://api.github.com to read my recent activity, issues and pull requests.",
        ),
        Some("linear") => (
            "Linear",
            "My Linear work. Use https://api.linear.app/graphql to read my issues and recent updates. Send the key in the Authorization header as {{key}}.",
        ),
        _ => ("", ""),
    };
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        shell("Connect sources", html! {
            main class="connect" {
                style {".connect{max-width:960px;margin:3rem auto}.eyebrow{color:#8fd3ff;text-transform:uppercase;letter-spacing:.14em;font-size:12px}.connect h1{font-size:2.6rem;margin:.4rem 0}.intro{color:#b9c6d8;max-width:620px;line-height:1.6}.catalog{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:.8rem;margin:1.5rem 0}.source-card,.connect-form,.disclosure{border:1px solid #354358;border-radius:12px;padding:1.3rem;background:linear-gradient(125deg,#1b2737,#151d2a)}.catalog a{display:block;text-decoration:none;font-weight:600}.catalog small{display:block;color:#b9c6d8;font-weight:400;margin-top:.5rem}.source-row{display:flex;justify-content:space-between;gap:1rem;align-items:center}.source-row h3{margin:.2rem 0}.source-card{margin:.8rem 0}.source-card p{overflow-wrap:anywhere;line-height:1.5}.connect-form{margin:1.5rem 0}.connect label{display:block;margin:1rem 0 .4rem;font-weight:600}.connect input,.connect textarea{box-sizing:border-box;width:100%;background:#0f1723;color:#e8edf5;border:1px solid #61758f;border-radius:6px;padding:.7rem;font:inherit}.connect textarea{min-height:110px;resize:vertical}.connect button{font:inherit;cursor:pointer;border:1px solid #8fd3ff;border-radius:6px;padding:.6rem 1rem;color:#10151e;background:#8fd3ff}.connect button[aria-checked=false]{color:#b9c6d8;background:#1b2737;border-color:#61758f}.connect .hint{font-size:14px;color:#b9c6d8;line-height:1.5}.disclosure{margin:2rem 0;border-color:#8c7951}.disclosure h2{font-size:18px;margin-top:0}.disclosure p{font-size:14px;line-height:1.6}.status{color:#a8e6c1}.notice{border-left:3px solid #a8e6c1;padding:.8rem;background:#1b302d}@media(max-width:600px){body{margin:1rem}.connect{margin:2rem 0}.connect h1{font-size:2rem}.source-row{align-items:flex-start}}"}
                p class="eyebrow" {"Your memory, connected"}
                h1 {"Connect your sources"}
                p class="intro" {"Bring the rest of your work into your day. Paste a key and tell Hyperthymesia what it connects to. Your agent can use it live on later runs, with citations back to the answer it received."}
                @if query.connected.as_deref() == Some("1") {
                    p class="notice" role="status" {"Connected and on. No other setup is needed for live use. Your next scheduled run can use this source; connecting did not start a model run."}
                }
                section aria-label="Connected sources" {
                    h2 {"Your sources"}
                    @if configured.as_array().is_none_or(Vec::is_empty) {
                        p class="hint" {"No sources yet. Add your first connection below."}
                    }
                    @for source in configured.as_array().into_iter().flatten() {
                        article class="source-card" {
                            div class="source-row" {
                                div {
                                    h3 {(source["name"].as_str().unwrap_or("Source"))}
                                    span class="status" {
                                        @if source["enabled"] == true {"On · available live"} @else {"Off · unavailable to the agent"}
                                    }
                                }
                                form method="post" action=(format!("/connect/{}",source["id"].as_str().unwrap_or(""))) {
                                    input type="hidden" name="enabled" value=(if source["enabled"] == true {"false"} else {"true"});
                                    button type="submit" role="switch" aria-checked=(if source["enabled"] == true {"true"} else {"false"})
                                        aria-label=(format!("{} {}",if source["enabled"] == true {"Switch off"} else {"Switch on"},source["name"].as_str().unwrap_or("source"))) {
                                        @if source["enabled"] == true {"Switch off"} @else {"Switch on"}
                                    }
                                }
                            }
                            p class="hint" {(source["description"].as_str().unwrap_or(""))}
                            p class="hint" {
                                @if source["has_key"] == true {"Key stored privately"} @else {"No key stored; uses the server's existing CLI login, if available"}
                                ". Turning a source off stays off. Only you can turn it back on."
                            }
                        }
                    }
                }
                section aria-label="Add a source" {
                    h2 {"Add a connection"}
                    div class="catalog" {
                        a class="source-card" href="/connect?template=github#add" {"GitHub" small {"Issues, pull requests, activity"}}
                        a class="source-card" href="/connect?template=linear#add" {"Linear" small {"Issues and project updates"}}
                        a class="source-card" href="/connect#add" {"Add your own" small {"Any API, CLI or MCP server"}}
                    }
                    form class="connect-form" id="add" method="post" action="/connect" autocomplete="off" {
                        h2 {"Add your own"}
                        label for="source-name" {"Name"}
                        input id="source-name" name="name" required maxlength="200" placeholder="e.g. My work inbox" value=(name);
                        label for="source-description" {"What is it?"}
                        textarea id="source-description" name="description" required maxlength="16384" placeholder="My messages at https://mail.example/api. Read my recent conversations." {(description)}
                        p class="hint" {"Include the API or MCP server's address. For a CLI, include its command in backticks, like `my-cli`. The agent works out how to call it. Only owner-described destinations receive the key in direct requests."}
                        label for="source-key" {"Key or credential"}
                        input id="source-key" name="key" type="password" autocomplete="new-password" spellcheck="false" autocapitalize="none" maxlength="16384" placeholder="Paste a key (optional for a signed-in CLI)";
                        p class="hint" {"Use a read-only, narrowly scoped key where possible. It is never shown back here."}
                        button type="submit" {"Connect and turn on"}
                    }
                }
                aside class="disclosure" aria-label="Before you connect" {
                    h2 {"Before you connect"}
                    p {"The agent has full bash on this server, with its real HOME and extended PATH. There is no sandbox. Commands can read or change files, use existing CLI logins, install software and contact the network."}
                    p {"Code blocks common literal catastrophic commands, including disk erasure, shutdown, fork bombs and forced recursive deletion of root or home. It also blocks literal access to Hyperthymesia's state folder. This is an accident guard, not a security boundary: scripts, variables, symlinks, obfuscated commands and other destructive operations can bypass it."}
                    p {"Keys are kept in private, mode-0600 files on this server, outside SQLite and backups. Each call receives only its own source's key. Known keys and common encodings are scrubbed before prompts, tool logs and saved answers. Full host bash can still read keys and other host credentials or send them elsewhere; keys are not hidden from the agent, and scrubbing cannot stop every encoding or deliberate exfiltration."}
                    p {"Allowed source content and your summaries go to the configured cloud model. Retrieved text is treated as untrusted data. Live answers are saved for citations, but old records and current lookups are context, not proof of activity at another time."}
                    p {"Connecting and switching sources do not start a model run. An off source cannot be called or re-enabled through the agent's broker tools."}
                }
            }
        }),
    ).into_response())
}

fn same_origin(headers: &HeaderMap) -> ApiResult<()> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| url::Url::parse(v).ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if let (Some(origin), Some(host)) = (origin, host)
        && matches!(origin.scheme(), "http" | "https")
        && url::Url::parse(&format!("{}://{host}", origin.scheme()))
            .is_ok_and(|expected| expected.origin() == origin.origin())
    {
        return Ok(());
    }
    Err(crate::ApiError(
        StatusCode::FORBIDDEN,
        "same-origin form required".into(),
    ))
}

pub async fn connect(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Form<sources::Connect>, axum::extract::rejection::FormRejection>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    let Form(request) = request.map_err(|_| crate::bad("invalid connection form"))?;
    let _ = sources::connect(State(store), headers, Ok(Json(request))).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Redirect::to("/connect?connected=1"),
    )
        .into_response())
}

pub async fn switch(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Result<Form<sources::SourcePatch>, axum::extract::rejection::FormRejection>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    let Form(request) = request.map_err(|_| crate::bad("invalid source switch"))?;
    let _ = sources::patch_source(State(store), headers, Path(id), Ok(Json(request))).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Redirect::to("/connect"),
    )
        .into_response())
}
