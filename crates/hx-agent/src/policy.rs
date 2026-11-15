use crate::config::Config;
use regex::Regex;
use std::sync::RwLock;

/// Never-record rules the person set in the app or the menu bar. One recorder
/// runs per process, and every gate below reads the same live copy.
static NEVER: RwLock<(Vec<String>, Vec<String>)> = RwLock::new((Vec::new(), Vec::new()));
pub(crate) fn set_never_record(rules: &crate::controls::NeverRecord) {
    *NEVER.write().expect("never-record rules") = (
        rules.apps.iter().map(|a| a.bundle_id.clone()).collect(),
        rules.sites.iter().map(|s| s.to_ascii_lowercase()).collect(),
    );
}
fn never_app(bundle: &str) -> bool {
    NEVER
        .read()
        .expect("never-record rules")
        .0
        .iter()
        .any(|b| b == bundle)
}
/// Whether any never-record site exists, so page addresses are worth reading.
pub fn sites_active() -> bool {
    !NEVER.read().expect("never-record rules").1.is_empty()
}
fn host_of(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    url.host_str()
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
}
/// A page on a never-record site, or its subdomains. Without an address, a
/// browser window titled with the site's name also counts.
fn never_site(url: Option<&str>, title: Option<&str>) -> bool {
    let rules = NEVER.read().expect("never-record rules");
    if rules.1.is_empty() {
        return false;
    }
    if let Some(host) = url.and_then(host_of) {
        return rules
            .1
            .iter()
            .any(|s| host == *s || host.ends_with(&format!(".{s}")));
    }
    let title = title.map(str::to_lowercase).unwrap_or_default();
    !title.is_empty() && rules.1.iter().any(|s| title.contains(s.as_str()))
}
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
            && !never_app(bundle)
            && (self.titles.is_empty()
                || title.is_some_and(|t| !self.titles.iter().any(|r| r.is_match(t))))
    }
    pub fn excludes_native(&self, focus: &hx_macos_sys::Focus) -> bool {
        !self.allows_focus(focus)
    }
    /// The app, its window title and, for browsers, the page's site.
    pub fn allows_focus(&self, focus: &hx_macos_sys::Focus) -> bool {
        self.allows(&focus.bundle_id, focus.title.as_deref())
            && !(hx_macos_sys::is_browser(&focus.bundle_id)
                && never_site(focus.url.as_deref(), focus.title.as_deref()))
    }
    pub fn allows_payload(&self, value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => {
                if map.iter().any(|(k, v)| {
                    ["bundle_id", "app_bundle_id", "bundleId"].contains(&k.as_str())
                        && v.as_str()
                            .is_some_and(|s| self.bundles.iter().any(|b| b == s) || never_app(s))
                }) {
                    return false;
                }
                if map.iter().any(|(k, v)| {
                    k == "url" && v.as_str().is_some_and(|u| never_site(Some(u), None))
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
    // No screen images or video may enter the outbox, including companion submissions.
    if matches!(envelope.kind.as_str(), "media.chunk" | "media.still")
        || envelope
            .blobs
            .iter()
            .chain(blobs.iter().map(|(blob, _)| blob))
            .any(|blob| {
                blob.media_type.to_ascii_lowercase().starts_with("video/")
                    || blob.media_type.to_ascii_lowercase().starts_with("image/")
            })
    {
        return Ok(false);
    }
    // Only the closed, content-free gap schema bypasses companion content checks.
    // A companion cannot smuggle arbitrary fields or blobs under this kind.
    let metadata_gap = envelope.kind.as_str() == "policy.gap"
        && envelope.blobs.is_empty()
        && blobs.is_empty()
        && envelope.payload.as_object().is_some_and(|map| {
            map.len() == 2
                && map.get("open").is_some_and(serde_json::Value::is_boolean)
                && map
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|reason| {
                        ["policy_exclusion", "capture_interrupted"].contains(&reason)
                    })
        });
    let content = !metadata_gap
        && (matches!(
            envelope.kind.as_str(),
            "text.snapshot" | "ui.click" | "media.chunk" | "media.still"
        ) || envelope
            .connector_instance
            .as_str()
            .starts_with("companion:"));
    if content
        && (secure
            || !focus.is_some_and(|f| policy.allows_focus(f))
            || !policy.allows_payload(&envelope.payload))
    {
        return Ok(false);
    }
    outbox.capture(envelope, blobs)
}
