//! Transport-independent connector protocol.
//!
//! A connector answers `Describe`, then streams `Changes` from a cursor (or a
//! `Backfill` over a scope). The server acknowledges a `Checkpoint` only after
//! every envelope before it is durably stored. An incomplete scan reports
//! `complete: false`; a missing row is never read as a deletion.

use crate::{Completeness, ConnectorInstance, Envelope, kinds::CollectorStatus};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    pub connector_instance: ConnectorInstance,
    pub connector: String,
    pub version: String,
    /// Kinds this connector may emit.
    pub kinds: Vec<String>,
    pub supports_backfill: bool,
    pub supports_changes: bool,
    /// The source mutates or deletes objects (so `upsert`/`delete` may appear).
    pub mutable_source: bool,
}

/// Opaque resume position plus a generation that changes when the source is
/// rebuilt (file rotated, database recreated). A generation change invalidates the cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cursor {
    pub generation: String,
    pub position: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeBatch {
    pub connector_instance: ConnectorInstance,
    pub envelopes: Vec<Envelope>,
    /// Cursor to persist once this batch is stored.
    pub next: Cursor,
    /// False when the source could not be fully read (locked, truncated, rate-limited).
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageReport {
    pub connector_instance: ConnectorInstance,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub completeness: Completeness,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub connector_instance: ConnectorInstance,
    pub status: CollectorStatus,
    pub observed_at: DateTime<Utc>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorMessage {
    Describe(Capabilities),
    Changes(ChangeBatch),
    Coverage(CoverageReport),
    Health(Health),
    /// Server → connector: stored durably up to this cursor.
    Checkpoint {
        connector_instance: ConnectorInstance,
        cursor: Cursor,
    },
}

/// Redeem a short-lived, one-use pairing code. Codes and returned tokens are secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    pub code: String,
    pub device_id: String,
}

/// Returned once by POST /v1/pair; failures uniformly return HTTP 410.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairResponse {
    pub token: String,
}

/// Which side runs OCR for newly acknowledged stills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OcrMode {
    Server,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceResponse {
    pub ocr: OcrMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OcrStatus {
    Complete,
    Sampled,
    Failed,
}

/// Device OCR has no disclosure class: only the server may decide disclosure.
/// Deliberately no Debug implementation: result may contain captured text.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OcrRequest {
    pub status: OcrStatus,
    pub engine_version: String,
    pub perceptual_hash: Option<String>,
    pub sampled_from: Option<String>,
    pub result: Option<serde_json::Value>,
}
