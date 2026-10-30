//! Envelope v1, core observation kinds, and the connector protocol.
//!
//! Everything that enters Hyperthymesia is an [`Envelope`]. The core understands
//! only the envelope and the payloads in [`kinds`]; everything else is opaque.

pub mod ids;
pub mod kinds;
pub mod protocol;
pub mod time;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use ids::*;
pub use time::*;

pub const ENVELOPE_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// A new immutable observation.
    Observe,
    /// A new revision of a mutable source object.
    Upsert,
    /// The source object was deleted at the source.
    Delete,
}

/// Disclosure class. Unknown data is `Restricted`; a producer label can only
/// narrow disclosure, never grant it (the server applies its own rules).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Normal,
    Sensitive,
    #[default]
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    Partial,
    Truncated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Coverage {
    /// What this observation claims to cover, e.g. `focused_window` or `backfill:2026-10-30`.
    pub scope: String,
    pub completeness: Completeness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Link {
    /// Relationship name, e.g. `session`, `parent`, `caused_by`, `same_as`.
    pub rel: String,
    /// Target identity, usually a `source_key` of another observation.
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BlobRef {
    /// Lowercase hex SHA-256 of the blob bytes.
    pub sha256: String,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    pub envelope_version: u16,
    /// Immutable identity of this observation, persisted by the producer before upload.
    pub observation_id: ObservationId,
    /// Installation/account/stream that produced it. Authenticated by the server.
    pub connector_instance: ConnectorInstance,
    /// Native identity of the source object or event.
    pub source_key: SourceKey,
    /// Source revision or content digest. Append-only sources use `"0"`.
    pub revision: Revision,
    pub operation: Operation,
    /// Producer sequence number, monotonically increasing per connector instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// Namespaced kind, e.g. `focus.span`. See [`kinds`].
    pub kind: Kind,
    pub schema_version: u16,
    pub time: ObservationTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    #[serde(default)]
    pub sensitivity: Sensitivity,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<BlobRef>,
}

/// The server's deduplication key: the same source object at the same revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DedupKey {
    pub connector_instance: ConnectorInstance,
    pub source_key: SourceKey,
    pub revision: Revision,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnvelopeError {
    #[error("unsupported envelope_version {0}")]
    Version(u16),
    #[error("kind `{0}` is not `namespace.name` lowercase")]
    Kind(String),
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("interval end precedes start")]
    NegativeInterval,
    #[error("blob sha256 `{0}` is not 64 lowercase hex chars")]
    BlobHash(String),
}

impl Envelope {
    pub fn dedup_key(&self) -> DedupKey {
        DedupKey {
            connector_instance: self.connector_instance.clone(),
            source_key: self.source_key.clone(),
            revision: self.revision.clone(),
        }
    }

    /// Boundary validation. Everything past ingest may trust a validated envelope.
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.envelope_version != ENVELOPE_VERSION {
            return Err(EnvelopeError::Version(self.envelope_version));
        }
        if !Kind::is_valid(self.kind.as_str()) {
            return Err(EnvelopeError::Kind(self.kind.as_str().to_owned()));
        }
        if self.connector_instance.as_str().is_empty() {
            return Err(EnvelopeError::Empty("connector_instance"));
        }
        if self.source_key.as_str().is_empty() {
            return Err(EnvelopeError::Empty("source_key"));
        }
        if self.revision.as_str().is_empty() {
            return Err(EnvelopeError::Empty("revision"));
        }
        self.time.validate()?;
        for b in &self.blobs {
            if b.sha256.len() != 64
                || !b
                    .sha256
                    .bytes()
                    .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
            {
                return Err(EnvelopeError::BlobHash(b.sha256.clone()));
            }
        }
        Ok(())
    }
}

/// One upload from a producer. Idempotent: replaying a batch is a no-op.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UploadBatch {
    #[schemars(with = "String")]
    pub batch_id: ulid::Ulid,
    pub envelopes: Vec<Envelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadAck {
    #[schemars(with = "String")]
    pub batch_id: ulid::Ulid,
    pub accepted: u32,
    pub duplicates: u32,
    /// Envelopes rejected at the boundary, with the reason. Never silently dropped.
    pub rejected: Vec<Rejected>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rejected {
    pub observation_id: ObservationId,
    pub reason: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// JSON Schemas for every wire type, keyed by file name under `schema/`.
pub fn json_schemas() -> Vec<(&'static str, serde_json::Value)> {
    let gen_ = |s: schemars::Schema| serde_json::to_value(s).expect("schema serializes");
    vec![
        ("envelope.v1.json", gen_(schemars::schema_for!(Envelope))),
        (
            "upload-batch.v1.json",
            gen_(schemars::schema_for!(UploadBatch)),
        ),
        ("upload-ack.v1.json", gen_(schemars::schema_for!(UploadAck))),
        (
            "connector-protocol.v1.json",
            gen_(schemars::schema_for!(protocol::ConnectorMessage)),
        ),
        (
            "kinds.v1.json",
            gen_(schemars::schema_for!(kinds::CorePayload)),
        ),
        (
            "companion-event.v1.json",
            gen_(schemars::schema_for!(CompanionEvent)),
        ),
    ]
}

impl Envelope {
    /// Build an envelope carrying a core payload. Callers set the optional fields after.
    pub fn core(
        connector_instance: ConnectorInstance,
        source_key: SourceKey,
        revision: Revision,
        operation: Operation,
        payload: &kinds::CorePayload,
        time: ObservationTime,
    ) -> Self {
        Self {
            envelope_version: ENVELOPE_VERSION,
            observation_id: ObservationId::generate(),
            connector_instance,
            source_key,
            revision,
            operation,
            sequence: None,
            kind: Kind::new(payload.kind()),
            schema_version: 1,
            time,
            device: None,
            actor: None,
            subject: None,
            resource: None,
            links: Vec::new(),
            coverage: None,
            sensitivity: Sensitivity::default(),
            payload: payload.payload_value(),
            blobs: Vec::new(),
        }
    }

    /// Typed core payload, if this is a core kind.
    pub fn core_payload(&self) -> Option<Result<kinds::CorePayload, serde_json::Error>> {
        kinds::CorePayload::decode(self.kind.as_str(), &self.payload)
    }
}

/// What a companion (browser extension, shell hook, editor extension) sends to the
/// local agent socket as one NDJSON line. The agent wraps it into an [`Envelope`]
/// with its own identity, sequence, clocks and capture policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompanionEvent {
    /// `browser`, `shell`, `editor`.
    pub companion: String,
    pub kind: Kind,
    /// Stable native identity, e.g. `tab:12:nav:5` or `shell:<pid>:<n>`.
    pub source_key: SourceKey,
    #[serde(default)]
    pub revision: Option<Revision>,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
    pub payload: serde_json::Value,
}
