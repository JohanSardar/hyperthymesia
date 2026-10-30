//! Pure L1 → L2 compiler: a lossless interval index over observed transitions.
//!
//! INTERFACE (frozen for parallel work; the implementation fills the bodies):
//! - [`compile`] is a pure function of its inputs. Same observations in any order,
//!   with duplicates, produce the same [`Timeline`].
//! - Every input observation that overlaps the window is referenced by exactly one or
//!   more [`LaneInterval::evidence`] or [`Timeline::points`] entries; nothing is dropped.
//! - Short switches are kept as their own intervals. No semantic merging here.

use chrono::{DateTime, Utc};
use hx_schema::{Envelope, Interval};
use serde::{Deserialize, Serialize};

/// An L0 envelope as stored by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Server row id, used as the evidence reference everywhere downstream.
    pub id: i64,
    pub received_at: DateTime<Utc>,
    pub envelope: Envelope,
}

pub type EvidenceId = i64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LaneId {
    /// `device:<id>` for device lanes, or `source:<connector_instance>` for others.
    pub key: String,
    /// `focus`, `presence`, `browser`, `agent`, `shell`, `messages`, ...
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    Observed,
    /// No data and no explanation (agent down, network, crash). Not "idle".
    Unknown,
    /// Content collection intentionally off (pause, exclusion, secure input); metadata may exist.
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaneInterval {
    pub interval: Interval,
    /// Human-readable label of what was observed, e.g. `Safari — Amazon.com: standing desk`.
    pub label: String,
    /// Structured attributes (app bundle, url, device state, thread id ...).
    pub attrs: serde_json::Value,
    pub evidence: Vec<EvidenceId>,
    /// True when the start or end was not directly observed (open span, inferred end).
    pub uncertain_edges: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub id: LaneId,
    pub intervals: Vec<LaneInterval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageSpan {
    pub lane_key: String,
    pub interval: Interval,
    pub state: CoverageState,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Point observations (commits, clicks, messages, stills, text snapshots) on the axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub at: DateTime<Utc>,
    pub lane_key: String,
    pub kind: String,
    pub label: String,
    pub evidence: EvidenceId,
}

/// Aggregates that cannot be placed into seconds (e.g. phone app-day totals).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateCard {
    pub day: String,
    pub label: String,
    pub seconds_estimated: u64,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub window: Interval,
    pub compiler_version: String,
    pub lanes: Vec<Lane>,
    pub points: Vec<Point>,
    pub coverage: Vec<CoverageSpan>,
    pub aggregates: Vec<AggregateCard>,
}

pub const COMPILER_VERSION: &str = concat!("hx-compile/", env!("CARGO_PKG_VERSION"));

/// Compile observations into a timeline for `window`.
pub fn compile(observations: &[Observation], window: Interval) -> Timeline {
    compiler::compile(observations, window)
}

/// Union of observed foreground exposure per lane, for durations in the API.
pub fn exposure_by_label(timeline: &Timeline, lane_kind: &str) -> Vec<(String, chrono::Duration)> {
    use std::collections::BTreeMap;
    let mut labels: BTreeMap<String, Vec<Interval>> = BTreeMap::new();
    for lane in &timeline.lanes {
        if lane.id.kind == lane_kind {
            for span in &lane.intervals {
                if let Some(interval) = span.interval.intersect(&timeline.window) {
                    labels.entry(span.label.clone()).or_default().push(interval);
                }
            }
        }
    }
    labels
        .into_iter()
        .map(|(label, spans)| (label, hx_schema::union_len(&spans)))
        .collect()
}

mod compiler;
