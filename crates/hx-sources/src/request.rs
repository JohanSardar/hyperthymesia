//! Bounded, source-scoped HTTP, CLI and MCP requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

pub const MAX_TIMEOUT: Duration = Duration::from_secs(180);
pub const MAX_HTTP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        body: Option<Value>,
    },
    Cli {
        argv: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Mcp {
        #[serde(flatten)]
        transport: McpTransport,
        tool: String,
        #[serde(default)]
        arguments: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpTransport {
    Http { url: String },
    Stdio { argv: Vec<String> },
}

#[derive(Clone, Copy, Default)]
pub struct Placeholders<'a> {
    pub key: Option<&'a str>,
    /// Opaque token, or an absolute next-link URL on the template's literal origin.
    pub cursor: Option<&'a str>,
}

/// Deliberately carries no URL, response body or underlying error: any could
/// contain a source credential. Callers can safely persist this message.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RequestError {
    #[error("source request cancelled by owner switch")]
    Cancelled,
    #[error("only HTTP execution is available")]
    Unsupported,
    #[error("invalid HTTP request")]
    Invalid,
    #[error("request requires a source key")]
    MissingKey,
    #[error("HTTP request failed or timed out")]
    Transport,
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("HTTP response exceeds size limit")]
    TooLarge,
    #[error("HTTP response is not JSON")]
    Json,
    #[error("MCP exchange failed or returned an invalid response")]
    McpUnavailable,
    #[error("destination is not named in the owner's source description")]
    Destination,
    #[error("invalid CLI request")]
    CliInvalid,
    #[error("host command blocked: {0}")]
    HostBlocked(&'static str),
    #[error("host command deadline exceeded")]
    HostDeadline,
    #[error("host command output exceeds 1 MB")]
    HostOutputLimit,
    #[error("host command could not start or read output")]
    HostIo,
    #[error("host command exited with code {0}")]
    Exit(i32),
    #[error("host command terminated by signal")]
    Signal,
    #[error("host command response is not JSON")]
    CliJson,
}

impl RequestError {
    pub fn needs_reconnect(&self) -> bool {
        matches!(self, Self::Status(401 | 403))
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Transport | Self::Status(408 | 429 | 500..=599))
    }
}

/// Rechecked immediately before each transport request or process start.
/// Callers also cancel the enclosing future while a response is in flight.
pub trait RequestGate: Sync {
    fn check(&self) -> impl std::future::Future<Output = Result<(), RequestError>> + Send;
}

struct Unrestricted;
impl RequestGate for Unrestricted {
    async fn check(&self) -> Result<(), RequestError> {
        Ok(())
    }
}

impl Request {
    /// Only owner-authored configuration can bind a credential destination.
    /// Response URLs and model-chosen source IDs are not credential authority.
    pub fn authorize(&self, description: &str) -> Result<(), RequestError> {
        let permitted = match self {
            Self::Http { url, .. }
            | Self::Mcp {
                transport: McpTransport::Http { url },
                ..
            } => {
                let target = substitute_url(
                    url,
                    Placeholders {
                        key: Some(""),
                        cursor: None,
                    },
                )?;
                let urls = regex::Regex::new(r#"https?://[^\s<>"'`]+"#)
                    .map_err(|_| RequestError::Invalid)?;
                urls.find_iter(description).any(|matched| {
                    reqwest::Url::parse(
                        matched
                            .as_str()
                            .trim_end_matches(['.', ',', ';', ')', ']', '}']),
                    )
                    .is_ok_and(|owner| owner.origin() == target.origin())
                })
            }
            Self::Cli { argv, .. }
            | Self::Mcp {
                transport: McpTransport::Stdio { argv },
                ..
            } => {
                let executable = argv.first().ok_or(RequestError::CliInvalid)?;
                let launcher = executable.rsplit('/').next().unwrap_or(executable);
                let indirect = [
                    "sh", "bash", "zsh", "env", "python", "python3", "node", "ruby", "perl", "npx",
                    "npm", "uv", "uvx", "bun", "deno",
                ]
                .contains(&launcher);
                let literal = description.split('`').skip(1).step_by(2).any(|command| {
                    shell_words::split(command).is_ok_and(|prefix| {
                        !prefix.is_empty()
                            && (!indirect || prefix.len() >= 2)
                            && argv.starts_with(&prefix)
                    })
                });
                literal
                    || (!indirect
                        && description.split_whitespace().any(|word| {
                            word.trim_matches(['\'', '"', ',', '.', ';', '(', ')']) == executable
                        }))
            }
        };
        if permitted {
            Ok(())
        } else {
            Err(RequestError::Destination)
        }
    }

    /// No retries and no model. CLI argv remain literal arguments, including
    /// response-controlled cursors; no substituted text is evaluated as shell.
    #[cfg(unix)]
    pub async fn execute(
        &self,
        values: Placeholders<'_>,
        timeout: Duration,
        host: &crate::host::HostRunner,
    ) -> Result<Value, RequestError> {
        self.execute_guarded(values, timeout, host, &Unrestricted)
            .await
    }

    #[cfg(unix)]
    pub async fn execute_guarded(
        &self,
        values: Placeholders<'_>,
        timeout: Duration,
        host: &crate::host::HostRunner,
        gate: &impl RequestGate,
    ) -> Result<Value, RequestError> {
        match self {
            Self::Http { .. } => self.execute_http_guarded(values, timeout, gate).await,
            Self::Mcp {
                transport,
                tool,
                arguments,
            } => crate::mcp::execute(transport, tool, arguments, values, timeout, host, gate).await,
            Self::Cli { argv, env } => {
                if timeout.is_zero() || argv.first().is_none_or(String::is_empty) {
                    return Err(RequestError::CliInvalid);
                }
                let argv = argv
                    .iter()
                    .map(|arg| substitute(arg, values))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut env = env
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), substitute(value, values)?)))
                    .collect::<Result<BTreeMap<_, _>, RequestError>>()?;
                if let Some(key) = values.key {
                    env.insert("SOURCE_KEY".into(), key.into());
                }
                let command = argv
                    .iter()
                    .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join(" ");
                gate.check().await?;
                let output =
                    host.run(&command, &env, timeout)
                        .await
                        .map_err(|error| match error {
                            crate::host::HostError::Blocked(reason) => {
                                RequestError::HostBlocked(reason)
                            }
                            crate::host::HostError::Deadline => RequestError::HostDeadline,
                            crate::host::HostError::OutputLimit => RequestError::HostOutputLimit,
                            crate::host::HostError::Io(_) => RequestError::HostIo,
                        })?;
                if !output.status.success() {
                    return Err(match output.status.code() {
                        Some(code) => RequestError::Exit(code),
                        None => RequestError::Signal,
                    });
                }
                serde_json::from_slice(&output.stdout).map_err(|_| RequestError::CliJson)
            }
        }
    }

    /// One HTTP exchange, no retries. Timeout covers headers and the entire body.
    /// Scrub the returned JSON before mapping, saving, or returning it to a model.
    pub async fn execute_http(
        &self,
        values: Placeholders<'_>,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        self.execute_http_guarded(values, timeout, &Unrestricted)
            .await
    }

    async fn execute_http_guarded(
        &self,
        values: Placeholders<'_>,
        timeout: Duration,
        gate: &impl RequestGate,
    ) -> Result<Value, RequestError> {
        let Self::Http {
            method,
            url,
            headers,
            body,
        } = self
        else {
            return Err(RequestError::Unsupported);
        };
        if timeout.is_zero() {
            return Err(RequestError::Invalid);
        }
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| RequestError::Invalid)?;
        if !matches!(method, reqwest::Method::GET | reqwest::Method::POST) {
            return Err(RequestError::Invalid);
        }
        let url = substitute_url(url, values)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(timeout.min(MAX_TIMEOUT))
            .build()
            .map_err(|_| RequestError::Invalid)?;
        let mut request = client.request(method, url);
        for (name, value) in headers {
            let value = substitute(value, values)?;
            if name.chars().any(char::is_control) || value.chars().any(char::is_control) {
                return Err(RequestError::Invalid);
            }
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.json(&substitute_json(body, values)?);
        }
        let request = request.build().map_err(|_| RequestError::Invalid)?;
        gate.check().await?;
        let mut response = client
            .execute(request)
            .await
            .map_err(|_| RequestError::Transport)?;
        if !response.status().is_success() {
            return Err(RequestError::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_HTTP_RESPONSE_BYTES as u64)
        {
            return Err(RequestError::TooLarge);
        }
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
        }
        serde_json::from_slice(&bytes).map_err(|_| RequestError::Json)
    }
}

/// Pin the unexpanded authority before any response or credential is substituted.
/// An absolute cursor is a next-link URL; ordinary cursor/key values are URL data.
pub(crate) fn substitute_url(
    template: &str,
    values: Placeholders<'_>,
) -> Result<reqwest::Url, RequestError> {
    let (_, rest) = template.split_once("://").ok_or(RequestError::Invalid)?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .ok_or(RequestError::Invalid)?;
    if authority.is_empty()
        || authority.contains(['{', '}', '@', '\\'])
        || template
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(RequestError::Invalid);
    }
    let pinned = reqwest::Url::parse(template).map_err(|_| RequestError::Invalid)?;
    if !matches!(pinned.scheme(), "http" | "https") || pinned.host_str().is_none() {
        return Err(RequestError::Invalid);
    }
    let url = if let Some(cursor) = values.cursor.filter(|_| template.contains("{{cursor}}"))
        && (cursor.contains("://") || cursor.starts_with("//"))
    {
        // Reject scheme-relative links too: next links must state their scheme.
        reqwest::Url::parse(cursor).map_err(|_| RequestError::Invalid)?
    } else {
        let key = values.key.map(encode_url_value);
        let cursor = values.cursor.map(encode_url_value);
        let expanded = substitute(
            template,
            Placeholders {
                key: key.as_deref(),
                cursor: cursor.as_deref(),
            },
        )?;
        reqwest::Url::parse(&expanded).map_err(|_| RequestError::Invalid)?
    };
    if url.scheme() != pinned.scheme()
        || url.host_str() != pinned.host_str()
        || url.port_or_known_default() != pinned.port_or_known_default()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RequestError::Invalid);
    }
    Ok(url)
}

/// Encode every byte except RFC 3986 unreserved bytes, regardless of URL position.
fn encode_url_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    out
}

/// A single pass keeps credential bytes such as `{{cursor}}` literal. Substitute
/// JSON string values before serialization so quotes cannot change the JSON shape.
pub(crate) fn substitute(input: &str, values: Placeholders<'_>) -> Result<String, RequestError> {
    let mut out = String::new();
    let mut remaining = input;
    while let Some(index) = remaining.find("{{") {
        out.push_str(&remaining[..index]);
        remaining = &remaining[index..];
        if let Some(rest) = remaining.strip_prefix("{{key}}") {
            out.push_str(values.key.ok_or(RequestError::MissingKey)?);
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix("{{cursor}}") {
            out.push_str(values.cursor.unwrap_or(""));
            remaining = rest;
        } else {
            out.push_str("{{");
            remaining = &remaining[2..];
        }
    }
    out.push_str(remaining);
    Ok(out)
}

pub(crate) fn substitute_json(
    value: &Value,
    values: Placeholders<'_>,
) -> Result<Value, RequestError> {
    match value {
        Value::String(s) => Ok(Value::String(substitute(s, values)?)),
        Value::Array(a) => a
            .iter()
            .map(|v| substitute_json(v, values))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| Ok((k.clone(), substitute_json(v, values)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::OriginalUri,
        http::{HeaderMap, StatusCode},
        routing::any,
    };
    use serde_json::json;

    struct Fixture {
        url: String,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn fixture(router: Router) -> Fixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Fixture { url, task }
    }
    async fn unfinished_response(headers: String) -> Fixture {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0, "client closed before sending headers");
                request.extend_from_slice(&buffer[..n]);
            }
            socket.write_all(headers.as_bytes()).await.unwrap();
            std::future::pending::<()>().await;
        });
        Fixture { url, task }
    }
    fn request(url: String) -> Request {
        Request::Http {
            method: "GET".into(),
            url,
            headers: BTreeMap::new(),
            body: None,
        }
    }

    #[tokio::test]
    async fn cursor_cannot_send_credentials_to_a_second_origin() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let other = fixture(Router::new().fallback(any(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Json(json!({}))
            }
        })))
        .await;
        let next_link = format!("{}/next", other.url);
        let home = fixture(Router::new().route(
            "/",
            any(move || {
                let next = next_link.clone();
                async move { Json(json!({"next": next})) }
            }),
        ))
        .await;
        let response = request(format!("{}/", home.url))
            .execute_http(Placeholders::default(), Duration::from_secs(2))
            .await
            .unwrap();
        let cursor = response["next"].as_str().unwrap();
        let injected_authority = format!("@{}/next", other.url.strip_prefix("http://").unwrap());
        let mut results = Vec::new();
        for (template, cursor) in [
            ("{{cursor}}".to_owned(), cursor),
            (
                format!("{}{{{{cursor}}}}", home.url),
                injected_authority.as_str(),
            ),
            (format!("{}/?cursor={{{{cursor}}}}", home.url), cursor),
            (format!("{}/{{{{cursor}}}}", home.url), cursor),
        ] {
            let mut req = request(template);
            if let Request::Http { headers, .. } = &mut req {
                headers.insert("authorization".into(), "Bearer {{key}}".into());
            }
            results.push(
                req.execute_http(
                    Placeholders {
                        key: Some("synthetic-key"),
                        cursor: Some(cursor),
                    },
                    Duration::from_secs(2),
                )
                .await,
            );
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "credentials must never reach the second server"
        );
        assert!(results.iter().all(|r| *r == Err(RequestError::Invalid)));

        for next in [
            home.url.replacen("http://", "https://", 1),
            home.url.replacen("127.0.0.1", "localhost", 1),
        ] {
            assert_eq!(
                substitute_url(
                    &format!("{}/{{{{cursor}}}}", home.url),
                    Placeholders {
                        key: None,
                        cursor: Some(&next)
                    }
                ),
                Err(RequestError::Invalid)
            );
        }
        let echo = fixture(Router::new().route(
            "/",
            any(|uri: OriginalUri| async move { Json(json!({"query":uri.query()})) }),
        ))
        .await;
        let result = request(format!(
            "{}/?cursor={{{{cursor}}}}&key={{{{key}}}}",
            echo.url
        ))
        .execute_http(
            Placeholders {
                key: Some("ab#c&d e"),
                cursor: Some("x&admin=1#frag"),
            },
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(
            result["query"],
            "cursor=x%26admin%3D1%23frag&key=ab%23c%26d%20e"
        );
        let result = request(format!("{}/{{{{cursor}}}}", echo.url))
            .execute_http(
                Placeholders {
                    key: None,
                    cursor: Some(&format!("{}/?safe=yes", echo.url)),
                },
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(
            result["query"], "safe=yes",
            "same-origin next links remain usable"
        );
    }

    #[tokio::test]
    async fn response_budget_is_eight_mebibytes_for_fixed_and_streaming_bodies() {
        let fixed = unfinished_response(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            8 * 1024 * 1024 + 1
        ))
        .await;
        assert_eq!(
            request(fixed.url.clone())
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await,
            Err(RequestError::TooLarge)
        );
        let streaming = fixture(Router::new().route(
            "/",
            any(|| async {
                let chunks = futures_util::stream::iter((0..9).map(|_| {
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b' '; 1024 * 1024]))
                }));
                axum::body::Body::from_stream(chunks)
            }),
        ))
        .await;
        assert_eq!(
            request(format!("{}/", streaming.url))
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await,
            Err(RequestError::TooLarge)
        );
    }

    #[tokio::test]
    async fn only_get_post_and_control_free_headers_are_allowed() {
        let server = fixture(Router::new().fallback(any(|| async { Json(json!({})) }))).await;
        for method in [
            "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
        ] {
            let mut req = request(format!("{}/", server.url));
            if let Request::Http { method: m, .. } = &mut req {
                *m = method.into();
            }
            assert_eq!(
                req.execute_http(Placeholders::default(), Duration::from_secs(2))
                    .await,
                Err(RequestError::Invalid),
                "accepted {method}"
            );
        }
        for control in ['\t', '\r', '\n', '\0', '\u{007f}'] {
            let mut req = request(format!("{}/", server.url));
            if let Request::Http { headers, .. } = &mut req {
                headers.insert("authorization".into(), "Bearer {{key}}".into());
            }
            assert_eq!(
                req.execute_http(
                    Placeholders {
                        key: Some(&format!("made-up{control}key")),
                        cursor: None
                    },
                    Duration::from_secs(2)
                )
                .await,
                Err(RequestError::Invalid)
            );
        }
    }

    #[tokio::test]
    async fn http_executes_method_headers_json_body_and_placeholders() {
        let fixture = fixture(Router::new().route("/items", any(
            |method: axum::http::Method, uri: OriginalUri, headers: HeaderMap, Json(body): Json<Value>| async move {
                Json(json!({"method":method.as_str(), "query":uri.query(), "authorization":headers["authorization"].to_str().unwrap(), "body":body}))
            }
        ))).await;
        let request = Request::Http {
            method: "POST".into(),
            url: format!("{}/items?cursor={{{{cursor}}}}", fixture.url),
            headers: BTreeMap::from([("authorization".into(), "Bearer {{key}}".into())]),
            body: Some(json!({"nested":["{{key}}", "{{cursor}}"]})),
        };
        let result = request
            .execute_http(
                Placeholders {
                    key: Some("synthetic-\"key\\"),
                    cursor: Some("page-2"),
                },
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(
            result,
            json!({"method":"POST", "query":"cursor=page-2", "authorization":"Bearer synthetic-\"key\\", "body":{"nested":["synthetic-\"key\\", "page-2"]}})
        );
    }

    #[tokio::test]
    async fn first_request_has_empty_cursor_and_placeholder_values_are_not_reexpanded() {
        let fixture = fixture(Router::new().route(
            "/",
            any(|Json(body): Json<Value>| async move { Json(body) }),
        ))
        .await;
        let request = Request::Http {
            method: "POST".into(),
            url: format!("{}/", fixture.url),
            headers: BTreeMap::new(),
            body: Some(json!(["{{key}}", "{{cursor}}", "literal"])),
        };
        let result = request
            .execute_http(
                Placeholders {
                    key: Some("planted-{{cursor}}"),
                    cursor: None,
                },
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(result, json!(["planted-{{cursor}}", "", "literal"]));
    }

    #[tokio::test]
    async fn statuses_classify_reconnect_and_retry_without_exposing_response_secrets() {
        let fixture = fixture(Router::new().route(
            "/{status}",
            any(
                |axum::extract::Path(status): axum::extract::Path<u16>| async move {
                    (StatusCode::from_u16(status).unwrap(), "planted-secret")
                },
            ),
        ))
        .await;
        for (status, reconnect, retryable) in [
            (401, true, false),
            (403, true, false),
            (429, false, true),
            (500, false, true),
            (400, false, false),
        ] {
            let error = request(format!("{}/{status}?key=planted-secret", fixture.url))
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await
                .unwrap_err();
            assert_eq!(error, RequestError::Status(status));
            assert_eq!(error.needs_reconnect(), reconnect);
            assert_eq!(error.retryable(), retryable);
            assert!(!format!("{error:?} {error}").contains("planted-secret"));
        }
    }

    #[tokio::test]
    async fn body_read_has_a_deadline_and_invalid_json_is_reported_without_body() {
        let slow =
            unfinished_response("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}".into()).await;
        let start = std::time::Instant::now();
        assert_eq!(
            request(slow.url.clone())
                .execute_http(Placeholders::default(), Duration::from_millis(40))
                .await
                .unwrap_err(),
            RequestError::Transport
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let fixture =
            fixture(Router::new().route("/bad", any(|| async { "planted-secret" }))).await;
        assert_eq!(
            request(format!("{}/bad", fixture.url))
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await
                .unwrap_err(),
            RequestError::Json
        );
    }

    #[tokio::test]
    async fn oversized_response_is_refused_before_waiting_for_the_body() {
        let fixture = unfinished_response(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_HTTP_RESPONSE_BYTES + 1
        ))
        .await;
        assert_eq!(
            request(fixture.url.clone())
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await
                .unwrap_err(),
            RequestError::TooLarge
        );
    }

    #[tokio::test]
    async fn no_redirect_or_automatic_retry_occurs() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let fixture = fixture(Router::new().route(
            "/",
            any(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::FOUND, [("location", "/redirected")])
                }
            }),
        ))
        .await;
        assert_eq!(
            request(format!("{}/", fixture.url))
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await
                .unwrap_err(),
            RequestError::Status(302)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_key_and_non_http_execution_are_explicit() {
        let missing = request("http://127.0.0.1/?key={{key}}".into());
        assert_eq!(
            missing
                .execute_http(Placeholders::default(), Duration::from_secs(1))
                .await
                .unwrap_err(),
            RequestError::MissingKey
        );
        let cli = Request::Cli {
            argv: vec!["must-not-execute".into()],
            env: BTreeMap::new(),
        };
        assert_eq!(
            cli.execute_http(Placeholders::default(), Duration::from_secs(1))
                .await
                .unwrap_err(),
            RequestError::Unsupported
        );
    }

    #[tokio::test]
    async fn all_request_shapes_round_trip_but_only_http_executes() {
        let fixture =
            fixture(Router::new().route("/", any(|| async { Json(json!({"ok":true})) }))).await;
        for value in [
            json!({"kind":"http", "method":"GET", "url":format!("{}/", fixture.url), "headers":{}, "body":null}),
            json!({"kind":"cli", "argv":["fixture", "{{cursor}}"], "env":{"KEY":"{{key}}"}}),
            json!({"kind":"mcp", "transport":"http", "url":"http://127.0.0.1", "tool":"list", "arguments":{}}),
            json!({"kind":"mcp", "transport":"stdio", "argv":["fixture"], "tool":"list", "arguments":{}}),
        ] {
            let request: Request = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(&request).unwrap(), value);
            let outcome = request
                .execute_http(Placeholders::default(), Duration::from_secs(2))
                .await;
            if value["kind"] == "http" {
                assert_eq!(outcome.unwrap(), json!({"ok":true}));
            } else {
                assert_eq!(outcome.unwrap_err(), RequestError::Unsupported);
            }
        }
    }
}
