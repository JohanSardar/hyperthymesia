//! Owner-authored transport credentials. Stored only in the private key store.
use crate::request::{Request, RequestError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub hx_connection_v1: bool,
    pub key: Option<String>,
    pub origin: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub redactions: Vec<String>,
}
impl Credentials {
    pub fn parse(value: Option<&str>) -> Result<Option<Self>, RequestError> {
        let Some(value) = value else { return Ok(None) };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(value) else {
            return Ok(None);
        };
        if json.get("hx_connection_v1").is_none() {
            return Ok(None);
        }
        let credentials: Self = serde_json::from_value(json).map_err(|_| RequestError::Invalid)?;
        credentials.validate()?;
        Ok(Some(credentials))
    }
    pub fn validate(&self) -> Result<(), RequestError> {
        if self
            .key
            .as_ref()
            .is_some_and(|s| s.len() < 4 || s.chars().any(char::is_control))
        {
            return Err(RequestError::Invalid);
        }
        for (name, value) in &self.headers {
            if name.is_empty()
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                || matches!(
                    name.to_ascii_lowercase().as_str(),
                    "host"
                        | "content-length"
                        | "connection"
                        | "transfer-encoding"
                        | "mcp-session-id"
                        | "mcp-protocol-version"
                )
                || value.chars().any(char::is_control)
            {
                return Err(RequestError::Invalid);
            }
        }
        for (name, value) in &self.env {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                || name.as_bytes()[0].is_ascii_digit()
                || matches!(
                    name.as_str(),
                    "HOME"
                        | "PATH"
                        | "ENV"
                        | "BASH_ENV"
                        | "SHELLOPTS"
                        | "NODE_OPTIONS"
                        | "PYTHONPATH"
                        | "PYTHONHOME"
                        | "SOURCE_KEY"
                )
                || ["HX_", "FX_", "LD_", "DYLD_", "XDG_"]
                    .iter()
                    .any(|p| name.starts_with(p))
                || value.contains('\0')
            {
                return Err(RequestError::Invalid);
            }
        }
        Ok(())
    }
    pub fn authorize(&self, request: &Request) -> Result<(), RequestError> {
        if let Some(origin) = &self.origin {
            let target = match request {
                Request::Http { url, .. }
                | Request::Mcp {
                    transport: crate::request::McpTransport::Http { url },
                    ..
                } => url,
                _ => return Err(RequestError::Destination),
            };
            let owner = reqwest::Url::parse(origin).map_err(|_| RequestError::Invalid)?;
            let target = reqwest::Url::parse(target).map_err(|_| RequestError::Invalid)?;
            if owner.origin() != target.origin() {
                return Err(RequestError::Destination);
            }
        }
        Ok(())
    }
}
