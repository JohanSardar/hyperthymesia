use crate::config::Config;
use regex::Regex;
#[derive(Clone)]
pub struct Policy {
    bundles: Vec<String>,
    titles: Vec<Regex>,
}
impl Policy {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            bundles: config.excluded_bundle_ids.clone(),
            titles: config
                .excluded_title_regexes
                .iter()
                .map(|s| Regex::new(s))
                .collect::<Result<_, _>>()?,
        })
    }
    pub fn allows(&self, bundle: &str, title: Option<&str>) -> bool {
        !self.bundles.iter().any(|b| b == bundle)
            && (self.titles.is_empty()
                || title.is_some_and(|t| !self.titles.iter().any(|r| r.is_match(t))))
    }
    pub fn allows_payload(&self, value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => {
                if map.iter().any(|(k, v)| {
                    ["incognito", "private", "is_private"].contains(&k.as_str())
                        && v.as_bool() != Some(false)
                }) {
                    return false;
                }
                if map.iter().any(|(k, v)| {
                    ["bundle_id", "app_bundle_id", "bundleId"].contains(&k.as_str())
                        && v.as_str()
                            .is_some_and(|s| self.bundles.iter().any(|b| b == s))
                }) {
                    return false;
                }
                if map.iter().any(|(k, v)| {
                    ["title", "window_title"].contains(&k.as_str())
                        && v.as_str()
                            .is_some_and(|s| self.titles.iter().any(|r| r.is_match(s)))
                }) {
                    return false;
                }
                map.values().all(|v| self.allows_payload(v))
            }
            serde_json::Value::Array(items) => items.iter().all(|v| self.allows_payload(v)),
            _ => true,
        }
    }
}

/// The only content persistence gate. Callers must supply the capture-time
/// context, not infer permission from a producer-provided sensitivity label.
pub fn persist(
    outbox: &mut crate::outbox::Outbox,
    policy: &Policy,
    focus: Option<&hx_macos_sys::Focus>,
    secure: bool,
    envelope: &hx_schema::Envelope,
    blobs: &[(hx_schema::BlobRef, Vec<u8>)],
) -> anyhow::Result<bool> {
    let content = matches!(
        envelope.kind.as_str(),
        "text.snapshot" | "ui.click" | "media.chunk" | "media.still"
    ) || envelope
        .connector_instance
        .as_str()
        .starts_with("companion:");
    let browser_unknown = envelope
        .connector_instance
        .as_str()
        .starts_with("companion:browser:")
        && !has_public_window_signal(&envelope.payload);
    if content
        && (secure
            || browser_unknown
            || (!envelope
                .connector_instance
                .as_str()
                .starts_with("companion:")
                && focus.is_some_and(hx_macos_sys::unverifiable_private_state))
            || !focus.is_some_and(|f| policy.allows(&f.bundle_id, f.title.as_deref()))
            || !policy.allows_payload(&envelope.payload))
    {
        return Ok(false);
    }
    outbox.enqueue(envelope, blobs)?;
    Ok(true)
}

fn has_public_window_signal(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.iter().any(|(k, v)| {
                ["incognito", "private", "is_private"].contains(&k.as_str())
                    && v.as_bool() == Some(false)
            }) || map.values().any(has_public_window_signal)
        }
        serde_json::Value::Array(values) => values.iter().any(has_public_window_signal),
        _ => false,
    }
}
