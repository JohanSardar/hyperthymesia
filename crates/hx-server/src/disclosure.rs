//! Server-owner disclosure grants. Only authenticated, namespace-checked stored
//! observations may be passed to this policy; producers never choose its rules.
use anyhow::{Context, Result, ensure};
use hx_schema::{Envelope, Sensitivity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
    /// Current owner grants, separate from the serialized base policy and digest.
    #[serde(skip)]
    pub source_grants: BTreeSet<String>,
}

impl DisclosurePolicy {
    /// No file selects the owner's laptop defaults; `off` selects no base rules.
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
        // Stills are only a lineage source for server OCR text; image blobs never leave.
        add("macos-media:", "media.still", Sensitivity::Sensitive, None);
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
        Self {
            rules,
            ..Self::default()
        }
    }

    /// Ingest has already bound each stored connector instance to its token's
    /// permitted prefixes. Never invoke this on an unverified incoming envelope.
    pub fn effective_class(&self, envelope: &Envelope) -> Sensitivity {
        // Gap payloads are never disclosed, even under an owner wildcard grant.
        if envelope.kind.as_str() == "policy.gap" {
            return envelope.sensitivity;
        }
        let source = envelope.connector_instance.as_str().starts_with("source:");
        if source
            && !self
                .source_grants
                .contains(envelope.connector_instance.as_str())
        {
            return Sensitivity::Restricted;
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
            .map_or_else(
                || {
                    if source {
                        Sensitivity::Sensitive
                    } else if hx_schema::kinds::is_core(envelope.kind.as_str()) {
                        envelope.sensitivity
                    } else {
                        // Opaque content needs an owner rule, regardless of its producer label.
                        Sensitivity::Restricted
                    }
                },
                |rule| rule.class,
            )
    }

    pub fn digest(&self) -> String {
        hx_store::hash(&serde_json::to_vec(&(POLICY_VERSION, self)).expect("policy serializes"))
    }
}

/// Grants are current owner state, outside the base policy's identity.
pub async fn current(store: &hx_store::Store) -> Result<DisclosurePolicy> {
    let mut policy = DisclosurePolicy::from_env()?;
    policy.source_grants = store.source_grants().await?;
    Ok(policy)
}

/// Prose is indivisible: an uncited source is still an input dependency.
pub async fn revision_disclosable(
    store: &hx_store::Store,
    value: &serde_json::Value,
    policy: &DisclosurePolicy,
    sensitive: crate::processing::SensitiveEgress,
) -> Result<bool> {
    if value["input_manifest"]["disclosure_policy"]["digest"] != policy.digest()
        || value["input_manifest"]["sensitive_egress"] != serde_json::json!(sensitive)
        || input_namespaces(value)
            .any(|ns| ns.starts_with("source:") && !policy.source_grants.contains(ns))
    {
        return Ok(false);
    }
    let ids = crate::mcp::revision_evidence(value);
    Ok(ids.is_empty() || crate::mcp::owner_allowed_ids(store, policy, sensitive, &ids).await?)
}
pub(crate) fn input_namespaces(value: &serde_json::Value) -> impl Iterator<Item = &str> {
    value["input_manifest"]["input_namespaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
}
/// Convert the store's transactional switch fence into a free deferred job.
pub(crate) fn publish_error(error: anyhow::Error) -> anyhow::Error {
    if error
        .downcast_ref::<hx_store::sources::SourceAccessChanged>()
        .is_some()
    {
        hx_jobs::Deferred::InputChanged.into()
    } else {
        error
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
    fn laptop_digest_stays_compatible_with_existing_revisions() {
        // Recorded on main at cb5dde5, before adding source grants.
        let golden = "3b37fcc74ece98e904e8bafa9d29fc0916c1cdbdbbc7fa8129b0c72345c747c1";
        let mut policy = DisclosurePolicy::laptop();
        assert!(policy.source_grants.is_empty());
        assert_eq!(policy.digest(), golden);
        let base_json = serde_json::to_string(&policy).unwrap();
        policy
            .source_grants
            .extend(["source:ab".into(), "source:abc".into()]);
        assert_eq!(policy.digest(), golden);
        assert_eq!(serde_json::to_string(&policy).unwrap(), base_json);
        let reloaded = DisclosurePolicy::from_json(&base_json).unwrap();
        assert!(reloaded.source_grants.is_empty());
        assert_eq!(reloaded.digest(), golden);
    }

    #[test]
    fn an_off_source_beats_a_wildcard_base_rule() {
        let mut policy = DisclosurePolicy::from_json(
            r#"{"rules":[
            {"connector_prefix":"source:", "kind":"*", "class":"normal"}
        ]}"#,
        )
        .unwrap();
        let mut source = envelope("source:ab", "source.document");
        for kind in ["source.document", "web.visit"] {
            source.kind = hx_schema::Kind::new(kind);
            assert_eq!(policy.effective_class(&source), Sensitivity::Restricted);
        }
        policy.source_grants.insert("source:ab".into());
        assert_eq!(policy.effective_class(&source), Sensitivity::Normal);
        source.kind = hx_schema::Kind::new("policy.gap");
        assert_eq!(policy.effective_class(&source), Sensitivity::Restricted);
    }

    #[test]
    fn source_grants_require_an_exact_current_namespace() {
        for mut policy in [DisclosurePolicy::default(), DisclosurePolicy::laptop()] {
            assert!(policy.source_grants.is_empty());
            policy.source_grants.insert("source:ab".into());
            let mut granted = envelope("source:ab", "source.document");
            for declared in [
                Sensitivity::Normal,
                Sensitivity::Sensitive,
                Sensitivity::Restricted,
            ] {
                granted.sensitivity = declared;
                assert_eq!(policy.effective_class(&granted), Sensitivity::Sensitive);
            }
            for namespace in ["source:abc", "source:AB", "source:ab:other"] {
                let mut off = envelope(namespace, "source.document");
                off.sensitivity = Sensitivity::Sensitive;
                assert_eq!(policy.effective_class(&off), Sensitivity::Restricted);
            }
            policy.source_grants.remove("source:ab");
            assert_eq!(policy.effective_class(&granted), Sensitivity::Restricted);
        }
    }

    #[test]
    fn base_rules_can_tighten_a_granted_source() {
        let mut policy = DisclosurePolicy::from_json(
            r#"{"rules":[
            {"connector_prefix":"source:ab", "kind":"source.document", "class":"restricted"},
            {"connector_prefix":"source:", "kind":"*", "class":"normal"}
        ]}"#,
        )
        .unwrap();
        policy.source_grants.insert("source:ab".into());
        assert_eq!(
            policy.effective_class(&envelope("source:ab", "source.document")),
            Sensitivity::Restricted
        );
        assert_eq!(
            policy.effective_class(&envelope("source:ab", "source.other")),
            Sensitivity::Normal
        );
    }

    #[test]
    fn unknown_kinds_stay_withheld_until_a_rule_grants_them() {
        let mut unknown = envelope("mcp-source:synthetic", "synthetic.document");
        for declared in [
            Sensitivity::Normal,
            Sensitivity::Sensitive,
            Sensitivity::Restricted,
        ] {
            unknown.sensitivity = declared;
            for policy in [DisclosurePolicy::default(), DisclosurePolicy::laptop()] {
                assert_eq!(policy.effective_class(&unknown), Sensitivity::Restricted);
            }
        }
        let mut known = envelope("mcp-source:synthetic", "web.visit");
        known.sensitivity = Sensitivity::Normal;
        assert_eq!(
            DisclosurePolicy::default().effective_class(&known),
            Sensitivity::Normal
        );
        let grant = DisclosurePolicy {
            rules: vec![DisclosureRule {
                connector_prefix: "mcp-source:".into(),
                kind: "synthetic.*".into(),
                class: Sensitivity::Normal,
                field_present: None,
            }],
            ..DisclosurePolicy::default()
        };
        assert_eq!(grant.effective_class(&unknown), Sensitivity::Normal);
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
            ("macos-media:synthetic", "media.still"),
            ("companion:browser:synthetic", "browser.navigation"),
            ("companion:editor:synthetic", "editor.focus"),
            ("companion:editor:synthetic", "editor.debug"),
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
            r#"{"rules":[],"source_grants":["source:ab"]}"#,
        ] {
            assert!(DisclosurePolicy::from_json(json).is_err());
        }
        assert!(DisclosurePolicy::from_json(r#"{"rules":[]}"#).is_ok());
    }
}
