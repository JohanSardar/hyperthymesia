//! One bounded MCP session per call; no remote instructions or sampling execute here.
use crate::{
    host::{HostRunner, OUTPUT_LIMIT, ProcessGroup},
    request::{
        MAX_HTTP_RESPONSE_BYTES, MAX_TIMEOUT, McpTransport, Placeholders, RequestError, substitute,
        substitute_json, substitute_url,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const VERSION: &str = "2025-11-25";

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":VERSION,"capabilities":{},
        "clientInfo":{"name":"hyperthymesia","version":"1"}}})
}
fn initialized() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}
fn lookup(tool: &str, arguments: Value) -> Value {
    if tool == "tools/list" {
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    } else {
        let arguments = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":arguments}})
    }
}
fn result(value: Value, id: u64) -> Result<Value, RequestError> {
    if value["jsonrpc"] != "2.0"
        || value["id"] != id
        || value.get("error").is_some()
        || value.get("result").is_none()
        || value["result"]["isError"] == true
    {
        return Err(RequestError::McpUnavailable);
    }
    Ok(value["result"].clone())
}

pub(crate) async fn execute(
    transport: &McpTransport,
    tool: &str,
    arguments: &Value,
    values: Placeholders<'_>,
    timeout: Duration,
    host: &HostRunner,
) -> Result<Value, RequestError> {
    if timeout.is_zero() || tool.is_empty() || tool.len() > 256 {
        return Err(RequestError::McpUnavailable);
    }
    let call = lookup(tool, substitute_json(arguments, values)?);
    tokio::time::timeout(timeout.min(MAX_TIMEOUT), async {
        match transport {
            McpTransport::Http { url } => http(url, &call, values).await,
            McpTransport::Stdio { argv } => stdio(argv, &call, values, host).await,
        }
    })
    .await
    .map_err(|_| RequestError::Transport)?
}

async fn http(url: &str, call: &Value, values: Placeholders<'_>) -> Result<Value, RequestError> {
    let url = substitute_url(url, values)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| RequestError::Invalid)?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "accept",
        "application/json, text/event-stream".parse().unwrap(),
    );
    if let Some(key) = values.key {
        headers.insert(
            "authorization",
            format!("Bearer {key}")
                .parse()
                .map_err(|_| RequestError::Invalid)?,
        );
    }
    let response = client
        .post(url.clone())
        .headers(headers.clone())
        .json(&initialize())
        .send()
        .await
        .map_err(|_| RequestError::Transport)?;
    if let Some(session) = response.headers().get("mcp-session-id") {
        if session.len() > 4096 {
            return Err(RequestError::McpUnavailable);
        }
        headers.insert("mcp-session-id", session.clone());
    }
    let init = http_result(response, 1).await?;
    let version = init["protocolVersion"]
        .as_str()
        .filter(|version| version.len() <= 32)
        .ok_or(RequestError::McpUnavailable)?;
    headers.insert(
        "mcp-protocol-version",
        version.parse().map_err(|_| RequestError::McpUnavailable)?,
    );
    let notified = client
        .post(url.clone())
        .headers(headers.clone())
        .json(&initialized())
        .send()
        .await
        .map_err(|_| RequestError::Transport)?;
    if !notified.status().is_success() {
        return Err(RequestError::Status(notified.status().as_u16()));
    }
    let response = client
        .post(url)
        .headers(headers)
        .json(call)
        .send()
        .await
        .map_err(|_| RequestError::Transport)?;
    http_result(response, 2).await
}

async fn http_result(mut response: reqwest::Response, id: u64) -> Result<Value, RequestError> {
    if !response.status().is_success() {
        return Err(RequestError::Status(response.status().as_u16()));
    }
    let sse = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RequestError::Transport)?
    {
        if chunk.len() > MAX_HTTP_RESPONSE_BYTES - bytes.len() {
            return Err(RequestError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
        if sse {
            // Streaming servers need not close the response after the reply.
            let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
            for event in text.split("\n\n").take(text.matches("\n\n").count()) {
                let data = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Ok(value) = serde_json::from_str::<Value>(&data)
                    && value["id"] == id
                {
                    return result(value, id);
                }
            }
        }
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| RequestError::McpUnavailable)?;
    result(value, id)
}

async fn stdio(
    argv: &[String],
    call: &Value,
    values: Placeholders<'_>,
    host: &HostRunner,
) -> Result<Value, RequestError> {
    if argv.first().is_none_or(String::is_empty) {
        return Err(RequestError::CliInvalid);
    }
    let command = argv
        .iter()
        .map(|arg| substitute(arg, values).map(|arg| format!("'{}'", arg.replace('\'', "'\\''"))))
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let env = values
        .key
        .map(|key| BTreeMap::from([("SOURCE_KEY".into(), key.into())]))
        .unwrap_or_default();
    let mut child = host
        .command(&command, &env)
        .map_err(|error| match error {
            crate::host::HostError::Blocked(reason) => RequestError::HostBlocked(reason),
            _ => RequestError::HostIo,
        })?
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|_| RequestError::HostIo)?;
    let _group = ProcessGroup::for_child(child.id().ok_or(RequestError::HostIo)?);
    let mut input = child.stdin.take().ok_or(RequestError::HostIo)?;
    let mut output = BufReader::new(child.stdout.take().ok_or(RequestError::HostIo)?);
    let mut stderr = child.stderr.take().ok_or(RequestError::HostIo)?;
    let budget = AtomicUsize::new(0);
    let exchange = async {
        for (message, id) in [
            (initialize(), Some(1)),
            (initialized(), None),
            (call.clone(), Some(2)),
        ] {
            let mut bytes =
                serde_json::to_vec(&message).map_err(|_| RequestError::McpUnavailable)?;
            bytes.push(b'\n');
            input
                .write_all(&bytes)
                .await
                .map_err(|_| RequestError::HostIo)?;
            if let Some(id) = id {
                loop {
                    let mut line = Vec::new();
                    let n = (&mut output)
                        .take((OUTPUT_LIMIT + 1) as u64)
                        .read_until(b'\n', &mut line)
                        .await
                        .map_err(|_| RequestError::HostIo)?;
                    if budget.fetch_add(n, Ordering::Relaxed) + n > OUTPUT_LIMIT {
                        return Err(RequestError::HostOutputLimit);
                    }
                    let value: Value =
                        serde_json::from_slice(&line).map_err(|_| RequestError::McpUnavailable)?;
                    if value["id"] == id {
                        let value = result(value, id)?;
                        if id == 2 {
                            return Ok(value);
                        }
                        break;
                    }
                    if value.get("id").is_some() {
                        return Err(RequestError::McpUnavailable);
                    }
                }
            }
        }
        Err(RequestError::McpUnavailable)
    };
    tokio::pin!(exchange);
    let mut buffer = [0; 4096];
    let mut stderr_open = true;
    loop {
        tokio::select! {
            value = &mut exchange => return value,
            read = stderr.read(&mut buffer), if stderr_open => {
                let n = read.map_err(|_| RequestError::HostIo)?;
                stderr_open = n != 0;
                if budget.fetch_add(n, Ordering::Relaxed) + n > OUTPUT_LIMIT {
                    return Err(RequestError::HostOutputLimit);
                }
            }
        }
    }
}
