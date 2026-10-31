//! Server-owner disclosure grants. Only authenticated, namespace-checked stored
//! observations may be passed to this policy; producers never choose its rules.
use anyhow::{Context, Result, ensure};
use hx_schema::{Envelope, Sensitivity};
use serde::{Deserialize, Serialize};

pub const POLICY_VERSION: &str = "hx-disclosure/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureRule {
    /// Literal connector-instance prefix, including the namespace delimiter.
    pub connector_prefix: String,
    /// Exact kind, or a prefix ending in `*` (`*` matches every kind).
    pub kind: String,
    pub class: Sensitivity,
    /// Optional non-null payload field condition, used for title-bearing focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_present: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosurePolicy {
    /// First matching rule wins. Put specific rules before broader rules.
    pub rules: Vec<DisclosureRule>,
}

impl DisclosurePolicy {
    /// No file selects the owner's laptop defaults; `off` retains producer labels.
    /// A file replaces the defaults entirely. Invalid configuration fails closed.
    pub fn from_env() -> Result<Self> {
        match std::env::var("HX_DISCLOSURE") {
            Err(std::env::VarError::NotPresent) => Ok(Self::laptop()),
            Ok(path) if path == "off" => Ok(Self::default()),
            Ok(path) => Self::from_json(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("read HX_DISCLOSURE file {path}"))?,
            ),
            Err(error) => Err(error.into()),
        }
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let policy: Self = serde_json::from_str(json).context("invalid disclosure policy")?;
        for rule in &policy.rules {
            ensure!(!rule.connector_prefix.is_empty(), "empty connector prefix");
            ensure!(!rule.kind.is_empty(), "empty kind pattern");
            ensure!(
                !rule.kind.trim_end_matches('*').contains('*')
                    && rule.kind.matches('*').count() <= 1,
                "kind patterns support only a trailing wildcard"
            );
            ensure!(
                rule.field_present.as_ref().is_none_or(|s| !s.is_empty()),
                "empty field condition"
            );
        }
        Ok(policy)
    }

    pub fn laptop() -> Self {
        let mut rules = Vec::new();
        let mut add = |prefix: &str, kind: &str, class, field: Option<&str>| {
            rules.push(DisclosureRule {
                connector_prefix: prefix.into(),
                kind: kind.into(),
                class,
                field_present: field.map(str::to_owned),
            });
        };
        // Classify an indivisible title-bearing record at its most sensitive field.
        add(
            "macos-agent:",
            "focus.span",
            Sensitivity::Sensitive,
            Some("window_title"),
        );
        for kind in [
            "focus.span",
            "presence.span",
            "input.bucket",
            "clock.calibration",
            "collector.health",
        ] {
            add("macos-agent:", kind, Sensitivity::Normal, None);
        }
        for kind in ["ui.*", "text.*"] {
            add("macos-agent:", kind, Sensitivity::Sensitive, None);
        }
        add("macos-media:", "text.*", Sensitivity::Sensitive, None);
        add(
            "macos-media:",
            "collector.health",
            Sensitivity::Normal,
            None,
        );
        for prefix in [
            "companion:browser:",
            "companion:editor:",
            "companion:shell:",
        ] {
            add(prefix, "*", Sensitivity::Sensitive, None);
        }
        Self { rules }
    }

    /// Ingest has already bound each stored connector instance to its token's
    /// permitted prefixes. Never invoke this on an unverified incoming envelope.
    pub fn effective_class(&self, envelope: &Envelope) -> Sensitivity {
        // Gap payloads are never disclosed, even under an owner wildcard grant.
        if envelope.kind.as_str() == "policy.gap" {
            return envelope.sensitivity;
        }
        self.rules
            .iter()
            .find(|rule| {
                envelope
                    .connector_instance
                    .as_str()
                    .starts_with(&rule.connector_prefix)
                    && rule.kind.strip_suffix('*').map_or_else(
                        || envelope.kind.as_str() == rule.kind,
                        |prefix| envelope.kind.as_str().starts_with(prefix),
                    )
                    && rule.field_present.as_ref().is_none_or(|field| {
                        envelope
                            .payload
                            .get(field)
                            .is_some_and(|value| !value.is_null())
                    })
            })
            .map_or(envelope.sensitivity, |rule| rule.class)
    }

    pub fn digest(&self) -> String {
        hx_store::hash(&serde_json::to_vec(&(POLICY_VERSION, self)).expect("policy serializes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(connector: &str, kind: &str) -> Envelope {
        let mut e = Envelope::core(
            hx_schema::ConnectorInstance::new(connector),
            hx_schema::SourceKey::new("synthetic"),
            hx_schema::Revision::new("0"),
            hx_schema::Operation::Observe,
            &hx_schema::kinds::CorePayload::InputBucket(Default::default()),
            hx_schema::ObservationTime::point(chrono::DateTime::UNIX_EPOCH),
        );
        e.kind = hx_schema::Kind::new(kind);
        e.sensitivity = Sensitivity::Restricted;
        e
    }

    #[test]
    fn defaults_cover_native_metadata_and_content_without_namespace_overreach() {
        let policy = DisclosurePolicy::laptop();
        for kind in [
            "focus.span",
            "presence.span",
            "input.bucket",
            "clock.calibration",
            "collector.health",
        ] {
            assert_eq!(
                policy.effective_class(&envelope("macos-agent:synthetic", kind)),
                Sensitivity::Normal
            );
        }
        for (connector, kind) in [
            ("macos-agent:synthetic", "ui.click"),
            ("macos-agent:synthetic", "text.snapshot"),
            ("macos-media:synthetic", "text.snapshot"),
            ("companion:browser:synthetic", "browser.navigation"),
            ("companion:editor:synthetic", "editor.selection"),
            ("companion:shell:synthetic", "shell.command"),
        ] {
            assert_eq!(
                policy.effective_class(&envelope(connector, kind)),
                Sensitivity::Sensitive
            );
        }
        for (connector, kind) in [
            ("macos-agent-spoof:synthetic", "focus.span"),
            ("unrelated:synthetic", "text.snapshot"),
            ("companion:unknown:synthetic", "shell.command"),
            ("macos-agent:synthetic", "unknown.content"),
            ("macos-media:synthetic", "media.still"),
            ("macos-agent:synthetic", "policy.gap"),
        ] {
            assert_eq!(
                policy.effective_class(&envelope(connector, kind)),
                Sensitivity::Restricted
            );
        }
        let mut e = envelope("unrelated:synthetic", "text.snapshot");
        e.sensitivity = Sensitivity::Sensitive;
        assert_eq!(policy.effective_class(&e), Sensitivity::Sensitive);
    }

    #[test]
    fn ordered_rules_override_defaults_and_digest_identifies_configuration() {
        let policy = DisclosurePolicy::from_json(r#"{"rules":[
            {"connector_prefix":"macos-agent:synthetic", "kind":"text.snapshot", "class":"restricted"},
            {"connector_prefix":"macos-agent:", "kind":"text.*", "class":"sensitive"}
        ]}"#).unwrap();
        assert_eq!(
            policy.effective_class(&envelope("macos-agent:synthetic", "text.snapshot")),
            Sensitivity::Restricted
        );
        assert_eq!(
            policy.effective_class(&envelope("macos-agent:other", "text.snapshot")),
            Sensitivity::Sensitive
        );
        assert_eq!(
            policy.effective_class(&envelope("macos-agent:synthetic", "text.other")),
            Sensitivity::Sensitive
        );
        assert_eq!(
            policy.effective_class(&envelope("macos-agent:synthetic", "focus.span")),
            Sensitivity::Restricted
        );
        assert_eq!(
            policy.digest(),
            DisclosurePolicy::from_json(&serde_json::to_string_pretty(&policy).unwrap())
                .unwrap()
                .digest()
        );
        let mut reordered = policy.clone();
        reordered.rules.reverse();
        assert_ne!(policy.digest(), reordered.digest());
        assert_eq!(
            reordered.effective_class(&envelope("macos-agent:synthetic", "text.snapshot")),
            Sensitivity::Sensitive
        );
    }

    #[test]
    fn reject_ambiguous_configuration() {
        for json in [
            r#"{"rules":[{"connector_prefix":"","kind":"*","class":"normal"}]}"#,
            r#"{"rules":[{"connector_prefix":"macos-agent:","kind":"fo*o","class":"normal"}]}"#,
            r#"{"rules":[{"connector_prefix":"macos-agent:","kind":"*","class":"public"}]}"#,
            r#"{"rules":[],"typo":true}"#,
        ] {
            assert!(DisclosurePolicy::from_json(json).is_err());
        }
        assert!(DisclosurePolicy::from_json(r#"{"rules":[]}"#).is_ok());
    }
}
