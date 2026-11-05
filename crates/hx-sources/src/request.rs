//! Transport-neutral request shapes. Only HTTP is executed in this piece.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

pub const MAX_TIMEOUT: Duration = Duration::from_secs(180);
pub const MAX_HTTP_RESPONSE_BYTES: usize = 64 * 1024 * 1_000;

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
    pub cursor: Option<&'a str>,
}

/// Deliberately carries no URL, response body or underlying error: any could
/// contain a source credential. Callers can safely persist this message.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RequestError {
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
}

impl RequestError {
    pub fn needs_reconnect(&self) -> bool {
        matches!(self, Self::Status(401 | 403))
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Transport | Self::Status(408 | 429 | 500..=599))
    }
}

impl Request {
    /// One HTTP exchange, no retries. Timeout covers headers and the entire body.
    /// Scrub the returned JSON before mapping, saving, or returning it to a model.
    pub async fn execute_http(
        &self,
        values: Placeholders<'_>,
        timeout: Duration,
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
        let url =
            reqwest::Url::parse(&substitute(url, values)?).map_err(|_| RequestError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(RequestError::Invalid);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(timeout.min(MAX_TIMEOUT))
            .build()
            .map_err(|_| RequestError::Invalid)?;
        let mut request = client.request(method, url);
        for (name, value) in headers {
            request = request.header(name, substitute(value, values)?);
        }
        if let Some(body) = body {
            request = request.json(&substitute_json(body, values)?);
        }
        let request = request.build().map_err(|_| RequestError::Invalid)?;
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

/// A single pass keeps credential bytes such as `{{cursor}}` literal. Substitute
/// JSON string values before serialization so quotes cannot change the JSON shape.
fn substitute(input: &str, values: Placeholders<'_>) -> Result<String, RequestError> {
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

fn substitute_json(value: &Value, values: Placeholders<'_>) -> Result<Value, RequestError> {
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
