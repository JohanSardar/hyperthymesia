//! OAuth credentials are opaque key-store entries, never recipe configuration.
//! Only owner-bound resource origins receive the short-lived access token.
use crate::{
    keys::KeyStore,
    request::{MAX_HTTP_RESPONSE_BYTES, McpTransport, Request, RequestError},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub authorization_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: String,
    pub origins: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub hx_oauth_v1: bool,
    pub config: Config,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

fn safe_url(text: &str) -> Result<reqwest::Url, RequestError> {
    let url = reqwest::Url::parse(text).map_err(|_| RequestError::Invalid)?;
    if (url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(RequestError::Invalid);
    }
    Ok(url)
}
impl Config {
    pub fn validate(&self) -> Result<(), RequestError> {
        safe_url(&self.authorization_url)?;
        safe_url(&self.token_url)?;
        safe_url(&self.redirect_uri)?;
        if self.client_id.is_empty() || self.scopes.is_empty() || self.origins.is_empty() {
            return Err(RequestError::Invalid);
        }
        for origin in &self.origins {
            safe_url(origin)?;
        }
        Ok(())
    }
}
impl Credential {
    pub fn parse(value: &str) -> Result<Option<Self>, RequestError> {
        let Ok(json) = serde_json::from_str::<Value>(value) else {
            return Ok(None);
        };
        if json.get("hx_oauth_v1").is_none() {
            return Ok(None);
        }
        let credential: Self = serde_json::from_value(json).map_err(|_| RequestError::Invalid)?;
        credential.config.validate()?;
        Ok(Some(credential))
    }
    pub fn authorize(&self, request: &Request) -> Result<(), RequestError> {
        let target = match request {
            Request::Http { url, .. }
            | Request::Mcp {
                transport: McpTransport::Http { url },
                ..
            } => reqwest::Url::parse(url).map_err(|_| RequestError::Invalid)?,
            _ => return Err(RequestError::Destination),
        };
        if self.config.origins.iter().any(|origin| {
            reqwest::Url::parse(origin).is_ok_and(|url| url.origin() == target.origin())
        }) {
            Ok(())
        } else {
            Err(RequestError::Destination)
        }
    }
}

/// No redirects, ambient proxies, provider error bodies, or unbounded retries.
pub async fn token(config: &Config, fields: &[(&str, &str)]) -> Result<Value, RequestError> {
    config.validate()?;
    let mut form =
        reqwest::Url::parse("https://form.invalid").map_err(|_| RequestError::Invalid)?;
    form.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("client_secret", &config.client_secret)
        .extend_pairs(fields.iter().copied());
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| RequestError::Invalid)?;
    let mut response = client
        .post(&config.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form.query().unwrap_or("").to_owned())
        .send()
        .await
        .map_err(|_| RequestError::Transport)?;
    if !response.status().is_success() {
        return Err(if matches!(response.status().as_u16(), 400 | 401 | 403) {
            RequestError::Login
        } else {
            crate::request::http_failure(response).await
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RequestError::Transport)?
    {
        if bytes.len() + chunk.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(RequestError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| RequestError::Json)?;
    if value["token_type"]
        .as_str()
        .is_none_or(|s| !s.eq_ignore_ascii_case("bearer"))
        || value["access_token"]
            .as_str()
            .is_none_or(|s| s.len() < 4 || s.chars().any(char::is_control))
    {
        return Err(RequestError::Login);
    }
    Ok(value)
}
pub fn expiry(value: &Value) -> Result<DateTime<Utc>, RequestError> {
    let seconds = value["expires_in"]
        .as_i64()
        .filter(|n| (1..=31_536_000).contains(n))
        .ok_or(RequestError::Login)?;
    Ok(Utc::now() + Duration::seconds(seconds))
}

// Serialize refresh/rotation across setup, live tools and the pull worker.
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub async fn access_key(
    keys: &KeyStore,
    id: &str,
    request: &Request,
    force: bool,
) -> Result<Option<String>, RequestError> {
    let _lock = REFRESH.lock().await;
    let Some(saved) = keys.read(id).map_err(|_| RequestError::MissingKey)? else {
        return Ok(None);
    };
    let Some(mut credential) = Credential::parse(&saved)? else {
        return Ok(Some(saved));
    };
    credential.authorize(request)?;
    if force || credential.expires_at <= Utc::now() + Duration::seconds(30) {
        if credential.refresh_token.len() < 4 {
            return Err(RequestError::Login);
        }
        let value = token(
            &credential.config,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &credential.refresh_token),
            ],
        )
        .await?;
        credential.access_token = value["access_token"]
            .as_str()
            .ok_or(RequestError::Login)?
            .to_owned();
        if let Some(refresh) = value["refresh_token"].as_str() {
            if refresh.len() < 4 {
                return Err(RequestError::Login);
            }
            credential.refresh_token = refresh.to_owned();
        }
        credential.expires_at = expiry(&value)?;
        keys.write(
            id,
            &serde_json::to_string(&credential).map_err(|_| RequestError::Invalid)?,
        )
        .map_err(|_| RequestError::MissingKey)?;
    }
    Ok(Some(credential.access_token))
}

/// Include each component, rather than only the serialized credential bundle.
pub fn redaction_values(value: &str) -> Vec<String> {
    fn visit(value: &Value, values: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if matches!(
                        key.as_str(),
                        "client_secret" | "refresh_token" | "access_token" | "verifier"
                    ) {
                        if let Some(secret) = value.as_str().filter(|s| s.len() >= 4) {
                            values.push(secret.to_owned());
                        }
                    } else {
                        visit(value, values);
                    }
                }
            }
            Value::Array(array) => {
                for item in array {
                    visit(item, values);
                }
            }
            _ => {}
        }
    }
    let mut values = vec![value.to_owned()];
    if let Ok(json) = serde_json::from_str::<Value>(value) {
        visit(&json, &mut values);
    }
    values
}

/// Resolve/refresh privately, retry an unexpectedly expired access token once,
/// and scrub both the dispatched and newly stored credentials before returning.
pub async fn execute(
    keys: &KeyStore,
    id: &str,
    request: &Request,
    cursor: Option<&str>,
    timeout: std::time::Duration,
    host: &crate::host::HostRunner,
    gate: &impl crate::request::RequestGate,
) -> Result<Value, RequestError> {
    gate.check().await?;
    let old = keys
        .redaction_keys()
        .map_err(|_| RequestError::MissingKey)?;
    let oauth = keys
        .read(id)
        .map_err(|_| RequestError::MissingKey)?
        .as_deref()
        .map(Credential::parse)
        .transpose()?
        .flatten()
        .is_some();
    let mut key = access_key(keys, id, request, false).await?;
    let mut response = request
        .execute_guarded(
            crate::request::Placeholders {
                key: key.as_deref(),
                cursor,
            },
            timeout,
            host,
            gate,
        )
        .await;
    if oauth && matches!(response, Err(RequestError::Status(401))) {
        gate.check().await?;
        key = access_key(keys, id, request, true).await?;
        response = request
            .execute_guarded(
                crate::request::Placeholders {
                    key: key.as_deref(),
                    cursor,
                },
                timeout,
                host,
                gate,
            )
            .await;
    }
    let value = response?;
    let mut secrets = keys
        .redaction_keys()
        .map_err(|_| RequestError::MissingKey)?;
    secrets.extend(old);
    if let Some(key) = key {
        secrets.push(key);
    }
    crate::scrub::Scrubber::new(secrets)
        .map_err(|_| RequestError::Invalid)
        .map(|s| s.scrub_json(&value))
}
