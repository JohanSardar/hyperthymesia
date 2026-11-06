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
    /// An interrupted rotation has an uncertain provider-side outcome. Require
    /// reconnect instead of repeatedly spending the possibly invalid old token.
    #[serde(default)]
    pub refresh_in_flight: bool,
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
    token_guarded(config, fields, &crate::request::Unrestricted, &mut false).await
}

async fn token_guarded(
    config: &Config,
    fields: &[(&str, &str)],
    gate: &impl crate::request::RequestGate,
    dispatched: &mut bool,
) -> Result<Value, RequestError> {
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
    let request = client
        .post(&config.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form.query().unwrap_or("").to_owned());
    gate.check().await?;
    // No cancellable await between this final check and dispatch.
    *dispatched = true;
    let mut response = request.send().await.map_err(|_| RequestError::Transport)?;
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

// Serialize only this credential's rotation. A slow provider must not block
// unrelated sources or state folders. Weak entries do not retain idle locks.
static REFRESH: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<std::path::PathBuf, std::sync::Weak<tokio::sync::Mutex<()>>>,
    >,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
async fn access_key(
    keys: &KeyStore,
    id: &str,
    request: &Request,
    force: bool,
    gate: &impl crate::request::RequestGate,
) -> Result<Option<String>, RequestError> {
    let identity = keys.identity(id).map_err(|_| RequestError::MissingKey)?;
    let lock = {
        let mut locks = REFRESH.lock().expect("refresh locks");
        locks.retain(|_, lock| lock.strong_count() > 0);
        let lock = locks.entry(identity).or_default();
        if let Some(lock) = lock.upgrade() {
            lock
        } else {
            let fresh = std::sync::Arc::new(tokio::sync::Mutex::new(()));
            *lock = std::sync::Arc::downgrade(&fresh);
            fresh
        }
    };
    let _lock = tokio::select! {
        biased;
        error = gate.cancelled() => return Err(error),
        lock = lock.lock() => lock,
    };
    gate.check().await?;
    let Some(saved) = keys.read(id).map_err(|_| RequestError::MissingKey)? else {
        return Ok(None);
    };
    let Some(mut credential) = Credential::parse(&saved)? else {
        return Ok(Some(saved));
    };
    credential.authorize(request)?;
    if credential.refresh_in_flight {
        return Err(RequestError::Login);
    }
    if force || credential.expires_at <= Utc::now() + Duration::seconds(30) {
        if credential.refresh_token.len() < 4 {
            return Err(RequestError::Login);
        }
        let save = |credential: &Credential| {
            keys.write(
                id,
                &serde_json::to_string(credential).map_err(|_| RequestError::Invalid)?,
            )
            .map_err(|_| RequestError::MissingKey)
        };
        credential.refresh_in_flight = true;
        save(&credential)?;
        // The containing registered exchange owns this future independently of
        // its caller. Once dispatched, the bounded response and durable save
        // complete even if a switch or caller deadline occurs.
        let mut dispatched = false;
        let value = match token_guarded(
            &credential.config,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &credential.refresh_token),
            ],
            gate,
            &mut dispatched,
        )
        .await
        {
            Ok(value) => value,
            Err(error) if !dispatched => {
                credential.refresh_in_flight = false;
                save(&credential)?;
                return Err(error);
            }
            Err(_) => return Err(RequestError::Login),
        };
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
        credential.refresh_in_flight = false;
        save(&credential)?;
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
    let mut key = access_key(keys, id, request, false, gate).await?;
    let mut response = tokio::select! {
        biased;
        error = gate.cancelled() => Err(error),
        response = request.execute_guarded(
            crate::request::Placeholders {
                key: key.as_deref(),
                cursor,
            },
            timeout,
            host,
            gate,
        ) => response,
    };
    if oauth && matches!(response, Err(RequestError::Status(401))) {
        gate.check().await?;
        key = access_key(keys, id, request, true, gate).await?;
        response = tokio::select! {
            biased;
            error = gate.cancelled() => Err(error),
            response = request.execute_guarded(
                crate::request::Placeholders {
                    key: key.as_deref(),
                    cursor,
                },
                timeout,
                host,
                gate,
            ) => response,
        };
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
