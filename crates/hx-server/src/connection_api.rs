//! JSON client contract for Connections and Agent. Reuses the source job lifecycle.
use crate::{ApiResult, bad, sources};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use hx_sources::{
    connection::Credentials,
    request::{McpTransport, Request},
};
use hx_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn body<T>(request: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    request
        .map(|Json(value)| value)
        .map_err(|_| bad("Invalid connection JSON"))
}
fn catalog_entries() -> Value {
    serde_json::from_str(include_str!("connection_catalog.json"))
        .expect("compiled connection catalog")
}
pub async fn catalog(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    sources::owner(&store, &headers).await?;
    Ok(Json(
        json!({"entries":catalog_entries(),"categories":["Mail","Calendar","Chat","Notes","Reading","Tasks","Files","Code","Music","Health","Activity","Apple"],"custom_kinds":["mcp","api","cli"],"cadence_seconds":14400}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    catalog_id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    key: Option<String>,
    connection: Option<Connection>,
    /// A single server object or an mcpServers object. Multiple servers require import_name.
    import_config: Option<Value>,
    import_name: Option<String>,
    #[serde(default)]
    variables: BTreeMap<String, String>,
    #[serde(default = "history_default")]
    history_days: u8,
    cli_home: Option<String>,
}
fn history_default() -> u8 {
    1
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Connection {
    Api {
        base_url: String,
        #[serde(default)]
        probe_path: String,
        #[serde(default = "get")]
        method: String,
        body: Option<Value>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    Mcp {
        transport: String,
        url: Option<String>,
        argv: Option<Vec<String>>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Cli {
        argv: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}
fn get() -> String {
    "GET".into()
}
fn imported(config: Value, selected: Option<&str>) -> ApiResult<(Connection, Option<String>)> {
    let (server, name) = if let Some(servers) = config.get("mcpServers") {
        let servers = servers
            .as_object()
            .ok_or_else(|| bad("mcpServers must be an object"))?;
        let name = selected
            .or_else(|| (servers.len() == 1).then(|| servers.keys().next().unwrap().as_str()))
            .ok_or_else(|| bad("Choose import_name when importing multiple MCP servers"))?;
        (
            servers
                .get(name)
                .ok_or_else(|| bad("Unknown import_name"))?
                .clone(),
            Some(name.to_owned()),
        )
    } else {
        (config, None)
    };
    let server = server
        .as_object()
        .ok_or_else(|| bad("MCP server configuration must be an object"))?;
    if server.keys().any(|k| {
        ![
            "url",
            "command",
            "args",
            "env",
            "headers",
            "type",
            "transport",
        ]
        .contains(&k.as_str())
    }) {
        return Err(bad("Unsupported MCP configuration field"));
    }
    if server.contains_key("url") == server.contains_key("command") {
        return Err(bad("MCP configuration needs exactly one of url or command"));
    }
    let map = |name| -> ApiResult<BTreeMap<String, String>> {
        serde_json::from_value(server.get(name).cloned().unwrap_or_else(|| json!({})))
            .map_err(|_| bad("MCP headers and env must contain string values"))
    };
    let transport = if let Some(url) = server.get("url") {
        McpTransport::Http {
            url: url.as_str().ok_or_else(|| bad("Invalid MCP URL"))?.into(),
        }
    } else {
        let mut argv = vec![
            server["command"]
                .as_str()
                .ok_or_else(|| bad("Invalid MCP command"))?
                .into(),
        ];
        let args: Vec<String> =
            serde_json::from_value(server.get("args").cloned().unwrap_or_else(|| json!([])))
                .map_err(|_| bad("MCP args must be an array of strings"))?;
        argv.extend(args);
        McpTransport::Stdio { argv }
    };
    Ok((
        Connection::Mcp {
            transport: match &transport {
                McpTransport::Http { .. } => "http",
                McpTransport::Stdio { .. } => "stdio",
            }
            .into(),
            url: match &transport {
                McpTransport::Http { url } => Some(url.clone()),
                _ => None,
            },
            argv: match transport {
                McpTransport::Stdio { argv } => Some(argv),
                _ => None,
            },
            headers: map("headers")?,
            env: map("env")?,
        },
        name,
    ))
}
fn endpoint(text: &str) -> ApiResult<url::Url> {
    let url = url::Url::parse(text).map_err(|_| bad("Invalid connection URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || text.contains("{{")
        || text.contains("${")
        || text.chars().any(char::is_control)
    {
        return Err(bad(
            "Use an HTTP(S) URL without credentials, fragments or unresolved variables",
        ));
    }
    Ok(url)
}
fn expand(value: &mut Value, variables: &BTreeMap<String, String>) -> ApiResult<()> {
    match value {
        Value::String(text) => {
            for (name, value) in variables {
                if name.is_empty()
                    || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    || value.contains("{{")
                    || value.chars().any(char::is_control)
                    || name == "key"
                {
                    return Err(bad("Invalid connection variable"));
                }
                *text = text.replace(&format!("{{{{{name}}}}}"), value);
                *text = text.replace(&format!("${{{name}}}"), value);
            }
        }
        Value::Array(values) => {
            for value in values {
                expand(value, variables)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                expand(value, variables)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub async fn connect(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<Connect>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    sources::owner(&store, &headers).await?;
    let request = body(request)?;
    if usize::from(request.catalog_id.is_some())
        + usize::from(request.connection.is_some())
        + usize::from(request.import_config.is_some())
        != 1
    {
        return Err(bad(
            "Supply exactly one of catalog_id, connection or import_config",
        ));
    }
    let mut name = request.name;
    let mut description = request.description.unwrap_or_default();
    let connection = if let Some(id) = &request.catalog_id {
        let entries = catalog_entries();
        let entry = entries
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"].as_str() == Some(id))
            .ok_or_else(|| bad("Unknown catalog_id"))?;
        if entry["auth"] == "key" && request.key.as_deref().is_none_or(str::is_empty) {
            return Err(bad(
                "This catalog connection requires a read-only API key or access token",
            ));
        }
        name = name.or_else(|| entry["name"].as_str().map(str::to_owned));
        description = format!(
            "{}\n{}",
            entry["description"].as_str().unwrap(),
            description
        );
        let mut config = entry["connection"].clone();
        expand(&mut config, &request.variables)?;
        serde_json::from_value(config).map_err(|_| bad("Invalid catalog connection"))?
    } else if let Some(config) = request.import_config {
        let mut config = config;
        expand(&mut config, &request.variables)?;
        let (connection, imported_name) = imported(config, request.import_name.as_deref())?;
        name = name.or(imported_name);
        connection
    } else {
        request.connection.unwrap()
    };
    let mut config = serde_json::to_value(connection).map_err(anyhow::Error::from)?;
    let private_templates = json!([config["headers"], config["env"]]).to_string();
    let redactions: Vec<String> = request
        .variables
        .iter()
        .filter(|(name, secret)| {
            secret.len() >= 4
                && (private_templates.contains(&format!("${{{name}}}"))
                    || private_templates.contains(&format!("{{{{{name}}}}}")))
        })
        .map(|(_, value)| value.clone())
        .collect();
    expand(&mut config, &request.variables)?;
    let connection: Connection =
        serde_json::from_value(config).map_err(|_| bad("Invalid connection configuration"))?;
    let mut credentials = Credentials {
        hx_connection_v1: true,
        key: request.key.filter(|v| !v.is_empty()),
        redactions,
        ..Default::default()
    };
    let (probe, cli, destination) = match connection {
        Connection::Api {
            base_url,
            probe_path,
            method,
            body,
            headers: mut values,
        } => {
            if !matches!(method.as_str(), "GET" | "POST") {
                return Err(bad("API method must be GET or POST"));
            }
            let mut url = json!(base_url);
            expand(&mut url, &request.variables)?;
            let base = endpoint(url.as_str().unwrap())?;
            let url = base
                .join(&probe_path)
                .map_err(|_| bad("Invalid probe path"))?;
            endpoint(url.as_str())?;
            if url.origin() != base.origin() {
                return Err(bad("Probe must use the API base origin"));
            }
            credentials.origin = Some(base.origin().ascii_serialization());
            if values.is_empty() && credentials.key.is_some() {
                values.insert("Authorization".into(), "Bearer {{key}}".into());
            }
            credentials.headers = values;
            (
                Request::Http {
                    method,
                    url: url.to_string(),
                    headers: BTreeMap::new(),
                    body,
                },
                None,
                format!("API at {base}"),
            )
        }
        Connection::Mcp {
            transport,
            url,
            argv,
            headers,
            env,
        } => {
            let transport = match (transport.as_str(), url, argv) {
                ("http", Some(url), None) => McpTransport::Http { url },
                ("stdio", None, Some(argv)) => McpTransport::Stdio { argv },
                _ => return Err(bad("MCP needs transport http with url, or stdio with argv")),
            };
            credentials.headers = headers;
            credentials.env = env;
            let (cli, destination) = match &transport {
                McpTransport::Http { url } => {
                    let base = endpoint(url)?;
                    if !credentials.env.is_empty() {
                        return Err(bad("HTTP MCP uses headers, not environment variables"));
                    }
                    credentials.origin = Some(base.origin().ascii_serialization());
                    if credentials.headers.is_empty() && credentials.key.is_some() {
                        credentials
                            .headers
                            .insert("Authorization".into(), "Bearer {{key}}".into());
                    }
                    (None, format!("MCP HTTP at {base}"))
                }
                McpTransport::Stdio { argv } => {
                    if !credentials.headers.is_empty() {
                        return Err(bad(
                            "STDIO MCP uses environment variables, not HTTP headers",
                        ));
                    }
                    (
                        argv.first().cloned(),
                        format!("MCP STDIO via {}", command_description(argv)?),
                    )
                }
            };
            (
                Request::Mcp {
                    transport,
                    tool: "tools/list".into(),
                    arguments: json!({}),
                },
                cli,
                destination,
            )
        }
        Connection::Cli { argv, env } => {
            credentials.env = env;
            let destination = format!(
                "CLI via {}. Executable {}",
                command_description(&argv)?,
                command_description(&argv[..1])?
            );
            let cli = argv.first().cloned();
            (
                Request::Cli {
                    argv,
                    env: BTreeMap::new(),
                    text: None,
                },
                cli,
                destination,
            )
        }
    };
    for values in [&mut credentials.headers, &mut credentials.env] {
        for value in values.values_mut() {
            for (name, secret) in &request.variables {
                if secret.len() >= 4
                    && (value.contains(secret)
                        || value.contains(&format!("${{{name}}}"))
                        || value.contains(&format!("{{{{{name}}}}}")))
                {
                    credentials.redactions.push(secret.clone());
                }
            }
            let mut json = json!(value);
            expand(&mut json, &request.variables)?;
            *value = json.as_str().unwrap().to_owned();
            if value.contains("{{key}}") {
                let key = credentials
                    .key
                    .as_deref()
                    .ok_or_else(|| bad("Connection requires a key"))?;
                *value = value.replace("{{key}}", key);
            }
            if value.contains("{{") || value.contains("${") {
                return Err(bad("Supply all configuration variables"));
            }
        }
    }
    credentials
        .validate()
        .map_err(|_| bad("Invalid connection credentials, headers or environment variables"))?;
    let key = serde_json::to_string(&credentials).map_err(anyhow::Error::from)?;
    let description = format!(
        "{description}\n{destination}. Owner-configured headers and environment are injected privately by the server on every request. Reuse the connection_check transport for MCP. Do not request or inspect credential values."
    );
    let legacy: sources::Connect = serde_json::from_value(json!({
        "name":name.unwrap_or_else(|| "Custom connection".into()), "description":description,
        "key":key,"history_days":request.history_days,"probe":probe,"cli_executable":cli,"cli_home":request.cli_home,
    })).map_err(|_| bad("Invalid connection"))?;
    sources::connect(State(store), headers, Ok(Json(legacy))).await
}
fn command_description(argv: &[String]) -> ApiResult<String> {
    if argv.is_empty()
        || argv.iter().any(|v| {
            v.contains('`')
                || v.contains("{{")
                || v.contains("${")
                || v.chars().any(char::is_control)
        })
    {
        return Err(bad(
            "Supply literal command arguments; put credentials in env",
        ));
    }
    Ok(format!(
        "`{}`",
        argv.iter()
            .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

pub async fn agent(State(store): State<Store>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let Json(sources) = sources::list(State(store.clone()), headers.clone()).await?;
    let Json(recipes) = sources::recipes(
        State(store.clone()),
        headers.clone(),
        Query(sources::Filter { source: None }),
    )
    .await?;
    let Json(addons) = sources::addons(
        State(store.clone()),
        headers,
        Query(sources::Filter { source: None }),
    )
    .await?;
    let mut connections = vec![];
    for source in sources.as_array().into_iter().flatten() {
        let id = source["id"].as_str().unwrap();
        let run = store
            .setup_run(id, source["setup_generation"].as_i64().unwrap())
            .await?;
        let setup = run.map(|r| json!({"generation":r.generation,"status":r.status,"result":r.result,"last_error":r.last_error,"created_at":r.created_at,"finished_at":r.finished_at}));
        connections.push(json!({"source":source,"setup":setup,"cadence_seconds":14400,
            "pulls":recipes.as_array().into_iter().flatten().filter(|r| r["source_id"] == source["id"]).collect::<Vec<_>>(),
            "addons":addons.as_array().into_iter().flatten().filter(|r| r["source_id"] == source["id"]).collect::<Vec<_>>() }));
    }
    Ok(Json(
        sources::scrubber(&store, None)
            .await?
            .scrub_json(&json!({"connections":connections})),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconnect {
    key: Option<String>,
}
pub async fn reconnect(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Result<Json<Reconnect>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    sources::owner(&store, &headers).await?;
    let request = body(request)?;
    if let Some(key) = &request.key {
        let filter = sources::scrubber(&store, Some(key)).await?;
        let source = store
            .source(&id)
            .await?
            .ok_or_else(|| crate::ApiError(StatusCode::NOT_FOUND, "Source not found".into()))?;
        let saved = json!({"source":source,"check":store.source_check(&id).await?,"recipes":store.recipes(Some(&id)).await?.iter().map(|r| &r.recipe.0).collect::<Vec<_>>(),"addons":store.addons(Some(&id)).await?});
        if filter.contains_secret(&saved.to_string()) {
            return Err(bad("Saved configuration must not contain credentials"));
        }
    }
    sources::reconnect_source(&store, &id, request.key.as_deref()).await?;
    Ok(Json(json!({"id":id,"status":"setting_up"})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entries {
    before: Option<i64>,
    #[serde(default = "entry_limit")]
    limit: u16,
}
fn entry_limit() -> u16 {
    50
}
pub async fn entries(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<Entries>,
) -> ApiResult<Json<Value>> {
    sources::owner(&store, &headers).await?;
    if query.limit == 0 || query.limit > 200 {
        return Err(bad("limit must be 1..=200"));
    }
    let source = store
        .source(&id)
        .await?
        .ok_or_else(|| crate::ApiError(StatusCode::NOT_FOUND, "Source not found".into()))?;
    let entries = store
        .source_entries(&source.connector_instance, query.before, query.limit)
        .await?;
    let next = entries.last().and_then(|e| e["id"].as_i64());
    Ok(Json(sources::scrubber(&store, None).await?.scrub_json(
        &json!({"entries":entries,"next_before":next}),
    )))
}
pub async fn detail(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let Json(agent) = agent(State(store.clone()), headers).await?;
    let mut value = agent["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["source"]["id"].as_str() == Some(&id))
        .cloned()
        .ok_or_else(|| crate::ApiError(StatusCode::NOT_FOUND, "Source not found".into()))?;
    value["connection_check"] =
        serde_json::to_value(store.source_check(&id).await?).map_err(anyhow::Error::from)?;
    Ok(Json(
        sources::scrubber(&store, None).await?.scrub_json(&value),
    ))
}
pub async fn retry_pull(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    sources::owner(&store, &headers).await?;
    if store.recipe(&id).await?.is_none() {
        return Err(crate::ApiError(
            StatusCode::NOT_FOUND,
            "Pull not found".into(),
        ));
    }
    if !store.retry_recipe(&id).await? {
        return Err(crate::ApiError(
            StatusCode::CONFLICT,
            "Pull is running, switched off, or source is not ready".into(),
        ));
    }
    store.schedule_pulls(chrono::Utc::now()).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id":id,"status":"queued"})),
    ))
}
pub(crate) fn router() -> axum::Router<Store> {
    use axum::{
        Router,
        extract::DefaultBodyLimit,
        routing::{get, post},
    };
    Router::new()
        .route("/v1/connections/catalog", get(catalog))
        .route("/v1/connections", get(sources::list).post(connect))
        .route(
            "/v1/connections/{id}",
            get(detail).patch(sources::patch_source),
        )
        .route("/v1/connections/{id}/setup", post(sources::setup))
        .route("/v1/connections/{id}/reconnect", post(reconnect))
        .route("/v1/connections/{id}/entries", get(entries))
        .route("/v1/agent", get(agent))
        .route("/v1/recipes/{id}/retry", post(retry_pull))
        .route("/v1/connections/oauth", post(crate::oauth::api_start))
        .route(
            "/v1/connections/oauth/finish",
            post(crate::oauth::api_finish),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}
