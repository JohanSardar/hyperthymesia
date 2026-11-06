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
    let Json(configured) = sources::list(State(store.clone()), headers).await?;
    let (name, description) = match query.template.as_deref() {
        Some("apple-mcp") => (
            "Apple Bridge MCP",
            "My Apple data through MCP at http://127.0.0.1:7780/mcp. Discover read tools such as calendar_agenda. Use a bounded recent time range; do not scan the full archive. Tool JSON may be in structuredContent or content text.",
        ),
        Some("apple-api") => (
            "Apple Bridge API",
            "My Apple data through the API at http://127.0.0.1:7780. Read calendar events with POST /v1/op/calendar.agenda (start, end). messages.thread takes chat_id, limit and opaque before/after cursors, not date filters. Use history-bound date filters. Discover schemas at /v1/catalog. records.query pages all history by ID; prefer time-bounded read operations.",
        ),
        Some("apple-cli") => (
            "Apple Bridge CLI",
            "My Apple data through `bridge`. Use bridge call --list and bridge call <read-operation> --args <JSON> --json. Read time-bounded calendar.agenda using history-bound filters. messages.thread uses chat_id and opaque before/after cursors, not dates. Do not start services, request permissions or read archives.",
        ),
        Some("gmail-gws") => (
            "Gmail via gws",
            "My Gmail through `gws`. Use gws gmail users messages list --params JSON with userId me, maxResults 50, q after:{{since_unix}}, pageToken {{cursor}}. List returns only id and threadId; use a detail request with gws gmail users messages get and id {{item}}, format full. Map internalDate as unix_ms and Subject from payload.headers. Use nextPageToken pagination; missing messages means empty. Auth exit code is 2. The server supplies the owner-configured executable and HOME; keep recipe env empty. Do not use --page-all (NDJSON) or export credentials.",
        ),
        Some("calendar-gws") => (
            "Google Calendar via gws",
            "My Google Calendar through `gws`. Use gws calendar events list --params JSON with calendarId primary, singleEvents true, timeMin {{since}}, timeMax {{until}}, maxResults 100 and pageToken {{cursor}}. Map id, summary, start.dateTime and end.dateTime. All-day dates are floating, not instants; never invent timestamps for them. Use nextPageToken pagination; missing items means empty. Auth exit code is 2. The server supplies the owner-configured executable and HOME; keep recipe env empty. Do not use --page-all (NDJSON) or export credentials.",
        ),
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
    let probe = match query.template.as_deref() {
        Some("gmail-gws") => {
            r#"{"kind":"cli","argv":["gws","gmail","users","messages","list","--params","{\"userId\":\"me\",\"maxResults\":1,\"q\":\"newer_than:1d\"}"],"env":{}}"#
        }
        Some("calendar-gws") => {
            r#"{"kind":"cli","argv":["gws","calendar","calendarList","list","--params","{\"maxResults\":1}"],"env":{}}"#
        }
        _ => "",
    };
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        shell("Connect sources", html! {
            main class="connect" {
                style {".connect{max-width:960px;margin:3rem auto}.eyebrow{color:#8fd3ff;text-transform:uppercase;letter-spacing:.14em;font-size:12px}.connect h1{font-size:2.6rem;margin:.4rem 0}.intro{color:#b9c6d8;max-width:620px;line-height:1.6}.catalog{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:.8rem;margin:1.5rem 0}.source-card,.connect-form,.disclosure{border:1px solid #354358;border-radius:12px;padding:1.3rem;background:linear-gradient(125deg,#1b2737,#151d2a)}.catalog a{display:block;text-decoration:none;font-weight:600}.catalog small{display:block;color:#b9c6d8;font-weight:400;margin-top:.5rem}.source-row{display:flex;justify-content:space-between;gap:1rem;align-items:center}.source-row h3{margin:.2rem 0}.source-card{margin:.8rem 0}.source-card p{overflow-wrap:anywhere;line-height:1.5}.connect-form{margin:1.5rem 0}.connect label{display:block;margin:1rem 0 .4rem;font-weight:600}.connect input,.connect textarea{box-sizing:border-box;width:100%;background:#0f1723;color:#e8edf5;border:1px solid #61758f;border-radius:6px;padding:.7rem;font:inherit}.connect textarea{min-height:110px;resize:vertical}.connect button{font:inherit;cursor:pointer;border:1px solid #8fd3ff;border-radius:6px;padding:.6rem 1rem;color:#10151e;background:#8fd3ff}.connect button[aria-checked=false]{color:#b9c6d8;background:#1b2737;border-color:#61758f}.connect .hint{font-size:14px;color:#b9c6d8;line-height:1.5}.disclosure{margin:2rem 0;border-color:#8c7951}.disclosure h2{font-size:18px;margin-top:0}.disclosure p{font-size:14px;line-height:1.6}.status{color:#a8e6c1}.notice{border-left:3px solid #a8e6c1;padding:.8rem;background:#1b302d}@media(max-width:600px){body{margin:1rem}.connect{margin:2rem 0}.connect h1{font-size:2rem}.source-row{align-items:flex-start}}"}
                h1 {"Connect your sources"}
                p class="intro" {"Connect an API, CLI or MCP server. The agent reads it during setup and later day-page runs. Saved answers can be cited."}
                @if query.connected.as_deref() == Some("1") {
                    p class="notice" role="status" {"Connection saved. Check its status below. Once login is available, one bounded setup run sets up automatic pulls."}
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
                                        @if source["enabled"] == true {(source["status"].as_str().unwrap_or("On"))} @else {"Off · broker requests and pulls stopped"}
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
                            @if let Some(error) = source["last_error"].as_str() {p role="alert" {(error)}}
                            @if source["status"] == "needs_reconnect" {
                                p {"Login is needed. Automatic pulls are paused; no repair run is purchased."}
                                @if source["oauth"] == true {
                                    a href=(format!("/connect/oauth?source={}",source["id"].as_str().unwrap_or(""))) {"Reconnect with OAuth"}
                                } @else {
                                    p {"Sign in to your CLI on the server using the configured HOME, or paste a replacement source key, then reconnect."}
                                    form method="post" action=(format!("/connect/{}/reconnect",source["id"].as_str().unwrap_or(""))) autocomplete="off" {
                                        label {"Replacement key (optional)" input type="password" name="key";}
                                        button type="submit" {"Reconnect"}
                                    }
                                }
                            }
                            p class="hint" {(source["description"].as_str().unwrap_or(""))}
                            p class="hint" {
                                @if source["has_key"] == true {"Key stored privately"} @else {"No key stored; uses the server's existing CLI login, if available"}
                                ". Turning a source off stays off. Only you can turn it back on."
                            }
                            p class="hint" {"Source removal is not available yet. Switching off stops this source's broker requests and automatic pulls, including running exchanges. It keeps the key, login material, settings, agent tables, saved evidence and citations. Full host bash can still access the source."}
                        }
                    }
                }
                section aria-label="Add a source" {
                    h2 {"Add a connection"}
                    div class="catalog" {
                        a class="source-card" href="/connect?template=apple-mcp#add" {"Apple Bridge MCP"}
                        a class="source-card" href="/connect?template=apple-api#add" {"Apple Bridge API"}
                        a class="source-card" href="/connect?template=apple-cli#add" {"Apple Bridge CLI"}
                        a class="source-card" href="/connect?template=gmail-gws#add" {"Gmail via gws"}
                        a class="source-card" href="/connect?template=calendar-gws#add" {"Calendar via gws"}
                        a class="source-card" href="/connect/oauth" {"Google Console OAuth" small {"Your own client; headless servers supported"}}
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
                        p class="hint" {"“Keep it local” in this description is an instruction to the agent, not a wall. " (host_bash_note()) " It can read files and credentials and can contact the network."}
                        label for="source-key" {"Key or credential"}
                        input id="source-key" name="key" type="password" autocomplete="new-password" spellcheck="false" autocapitalize="none" maxlength="16384" placeholder="Paste a key (optional for a signed-in CLI)";
                        p class="hint" {"Use a read-only, narrowly scoped key where possible. It is never shown back here."}
                        label for="source-history" {"History days (0–30)"}
                        input id="source-history" name="history_days" type="number" min="0" max="30" value="1";
                        label for="source-cli" {"CLI executable (optional)"}
                        input id="source-cli" name="cli_executable" placeholder="Command or absolute executable path" value=(match query.template.as_deref() { Some("gmail-gws" | "calendar-gws") => "gws", Some("apple-cli") => "bridge", _ => "" });
                        label for="source-home" {"CLI HOME (optional)"}
                        input id="source-home" name="cli_home" placeholder="Absolute HOME of the signed-in CLI on this server";
                        details {
                            summary {"Connection check"}
                            p class="hint" {"Optional read-only request checked without AI before setup. Change the executable here and in the description when using a custom CLI path. A failed check pauses setup until you reconnect."}
                            textarea name="probe_json" aria-label="Connection check JSON" {(probe)}
                        }
                        p class="hint" {"The configured cloud model can receive captured window titles, URLs, accessibility text, screenshot OCR, fuller saved evidence from broker lookups, and source responses during setup and live lookups. Connecting starts one paid setup run. Each day of history can also buy about 6 day-page runs, plus up to 6 more 24-hour passes, so keep history low."}
                        button type="submit" {"Connect and turn on"}
                    }
                }
                aside class="disclosure" aria-label="Before you connect" {
                    h2 {"Before you connect"}
                    p {(host_bash_note()) " It runs with its real HOME and extended PATH. There is no sandbox. Commands can read or change files, use existing CLI logins, install software and contact the network."}
                    p {"Code blocks common literal catastrophic commands, including disk erasure, shutdown, fork bombs and forced recursive deletion of root or home. It also blocks literal access to Hyperthymesia's state folder. This is an accident guard, not a security boundary: scripts, variables, symlinks, obfuscated commands and other destructive operations can bypass it."}
                    p {"Keys are kept in private, mode-0600 files on this server, outside SQLite and backups. Each call receives only its own source's key. Known keys and common encodings are scrubbed before prompts, tool logs and saved answers. Full host bash can still read keys and other host credentials or send them elsewhere; keys are not hidden from the agent, and scrubbing cannot stop every encoding or deliberate exfiltration."}
                    p {"Retrieved text is treated as untrusted data. Live answers are saved for citations, but old records and current lookups are context, not proof of activity at another time."}
                    p {"Connecting starts one bounded setup job with at most two attempts. Pulls then run every four hours without AI. Switching sources starts no model run. An off source cannot be called or re-enabled through the agent's broker tools."}
                }
            }
        }),
    ).into_response())
}

pub(crate) fn same_origin(headers: &HeaderMap) -> ApiResult<()> {
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

pub async fn agent_setup(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Response> {
    let Json(configured) = sources::list(State(store.clone()), headers.clone()).await?;
    let Json(recipes) = sources::recipes(
        State(store.clone()),
        headers.clone(),
        Query(sources::Filter { source: None }),
    )
    .await?;
    let Json(addons) = sources::addons(
        State(store.clone()),
        headers.clone(),
        Query(sources::Filter { source: None }),
    )
    .await?;
    let Json(tables) = sources::agent_tables(
        State(store.clone()),
        headers.clone(),
        Query(sources::Filter { source: None }),
    )
    .await?;
    let mut runs = Vec::new();
    for source in configured.as_array().into_iter().flatten() {
        if let (Some(id), Some(generation)) =
            (source["id"].as_str(), source["setup_generation"].as_i64())
            && let Some(run) = store.setup_run(id, generation).await?
        {
            runs.push(serde_json::json!({"source":id,"result":run.result,"error":run.last_error}));
        }
    }
    let runs = sources::scrubber(&store, None)
        .await?
        .scrub_json(&serde_json::json!(runs));
    Ok(([(header::CACHE_CONTROL,"no-store")],shell("What your agent set up",html! {
        main {
            h1 {"What your agent set up"}
            p {"Your agent discovers a source once. Plain code then pulls every four hours, resumes from a cursor, and catches up once after missed cycles. Pulls use no AI. A later day-page run can use and cite the captured data."}
            p {b {"Your switches stay yours."} " Switching a pull off stops requests, including queued pulls. Setup cannot turn it back on. Only you can re-enable it."}
            aside {p {b {"“Keep it local” is an instruction to the agent, not a wall."} " " (host_bash_note()) " Add-on prompts cannot enforce isolation or change core rules and citation checks. Allowed content may reach the configured cloud model."}}
            @if configured.as_array().is_none_or(Vec::is_empty) {p {"Nothing set up yet. " a href="/connect" {"Connect a source"}}}
            @for source in configured.as_array().into_iter().flatten() {
                section {
                    h2 {(source["name"].as_str().unwrap_or("Source"))}
                    p {"Status: " (source["status"].as_str().unwrap_or("unknown")) @if source["enabled"] != true {" · Source off"}}
                    @for run in runs.as_array().into_iter().flatten().filter(|r| r["source"]==source["id"]) {
                        @if let Some(summary) = run["result"]["summary"].as_str() {p {(summary)}}
                        @if let Some(reason) = run["result"]["no_pull"].as_str() {p {"No automatic pull: " (reason)}}
                        @if let Some(error) = run["error"].as_str() {p {(error)}}
                    }
                    @if let Some(error) = source["last_error"].as_str() {p {(error)}}
                    form method="post" action=(format!("/agent-setup/{}/retry",source["id"].as_str().unwrap_or(""))) {
                        button type="submit" {"Run setup again"}
                        small {" Starts one bounded paid setup job (at most two attempts). Your off switches stay off."}
                    }
                    h3 {"Automatic pulls"}
                    @for recipe in recipes.as_array().into_iter().flatten().filter(|r| r["source_id"]==source["id"]) {
                        article {
                            h4 {(recipe["name"].as_str().unwrap_or("Pull"))}
                            (configuration_form("recipe", recipe))
                            p {"Next pull: " (recipe["next_run_at"].as_str().unwrap_or("")) " · Last success: " (recipe["last_ok_at"].as_str().unwrap_or("none"))}
                            @if let Some(error) = recipe["last_error"].as_str() {p {(error)}}
                            details {summary {"Pull recipe"} pre {(serde_json::to_string_pretty(&recipe["recipe"]).unwrap_or_default())}}
                        }
                    }
                    h3 {"Add-on prompts"}
                    @for addon in addons.as_array().into_iter().flatten().filter(|a| a["source_id"]==source["id"]) {
                        h4 {(addon["name"].as_str().unwrap_or("Add-on"))} p {(addon["text"].as_str().unwrap_or(""))}
                        (configuration_form("addon", addon))
                    }
                    h3 {"Agent tables"}
                    @for table in tables.as_array().into_iter().flatten().filter(|t| t["name"].as_str().unwrap_or("").starts_with(&format!("s_{}_",source["id"].as_str().unwrap_or("")))) {
                        p {code {(table["name"].as_str().unwrap_or(""))} " · " (table["row_count"]) " rows"}
                    }
                }
            }
        }
    })).into_response())
}
fn configuration_form(kind: &str, value: &serde_json::Value) -> maud::Markup {
    let on = value["enabled"] == true && value["owner_off"] != true;
    html! {form method="post" action=(format!("/agent-setup/{kind}/{}",value["id"].as_str().unwrap_or(""))) {
        input type="hidden" name="enabled" value=(if on {"false"} else {"true"});
        button type="submit" role="switch" aria-checked=(if on {"true"} else {"false"}) aria-label=(format!("{} {}",if on {"Switch off"} else {"Switch on"}, value["name"].as_str().unwrap_or(kind))) {
            @if on {"Switch off"} @else {"Switch on"}
        }
    }}
}
pub async fn configuration_switch(
    State(store): State<Store>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, String)>,
    Form(request): Form<sources::Switch>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    match kind.as_str() {
        "recipe" => {
            let _ =
                sources::recipe_switch(State(store), headers, Path(id), Ok(Json(request))).await?;
        }
        "addon" => {
            let _ =
                sources::addon_switch(State(store), headers, Path(id), Ok(Json(request))).await?;
        }
        _ => return Err(crate::bad("unknown configuration kind")),
    }
    Ok(Redirect::to("/agent-setup").into_response())
}
pub async fn retry_setup(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    let _ = sources::setup(State(store), headers, Path(id)).await?;
    Ok(Redirect::to("/agent-setup").into_response())
}

#[derive(Deserialize)]
pub struct Reconnect {
    key: Option<String>,
}
pub async fn reconnect(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Result<Form<Reconnect>, axum::extract::rejection::FormRejection>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    sources::owner(&store, &headers).await?;
    let Form(request) = request.map_err(|_| crate::bad("Invalid reconnect form"))?;
    let key = request.key.filter(|s| !s.is_empty());
    if let Some(key) = &key {
        let filter = sources::scrubber(&store, Some(key)).await?;
        let source = store
            .source(&id)
            .await?
            .ok_or_else(|| crate::bad("Unknown source"))?;
        let config = serde_json::json!({"source":source,"recipes":store.recipes(Some(&id)).await?.iter().map(|r| &r.recipe.0).collect::<Vec<_>>()});
        if filter.contains_secret(&config.to_string()) {
            return Err(crate::bad(
                "Saved configuration must not contain credentials",
            ));
        }
    }
    sources::reconnect_source(&store, &id, key.as_deref()).await?;
    Ok(Redirect::to("/connect").into_response())
}

fn host_bash_note() -> &'static str {
    if crate::mcp::host_bash_enabled() {
        "The agent has full bash on this server."
    } else {
        "Host bash is off (HX_HOST_BASH=off), but commands a source names still run on this server under your account."
    }
}
