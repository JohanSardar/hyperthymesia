//! Observation time. Rules:
//! - Intervals are half-open `[start, end)`.
//! - Durations within one boot and clock domain come from the monotonic clock.
//! - Across boots, or when monotonic data is missing, durations come from UTC and are
//!   marked [`DurationBasis::WallClock`]. Unrelated clocks are never subtracted.

use crate::{BootId, EnvelopeError};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClockDomain {
    /// `mach_continuous_time`: advances during sleep.
    MachContinuous,
    /// `mach_absolute_time`: stops during sleep.
    MachAbsolute,
    /// A source-specific monotonic counter.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MonoStamp {
    pub domain: ClockDomain,
    pub boot_id: BootId,
    pub nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationTime {
    /// When the event happened according to the source. `None` if the source has no time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_start: Option<DateTime<Utc>>,
    /// End of a span; `None` for point events or open spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_end: Option<DateTime<Utc>>,
    /// Resolution of the source timestamps, in milliseconds (1000 for whole seconds).
    pub resolution_ms: u32,
    /// When the producer collected it.
    pub observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_start: Option<MonoStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_end: Option<MonoStamp>,
    /// Id of the latest clock-calibration observation for this device, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<String>,
    /// UTC offset of the device at capture, in minutes. Needed for civil days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_min: Option<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DurationBasis {
    Monotonic,
    WallClock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Duration {
    pub nanos: u64,
    pub basis: DurationBasis,
    /// Bound on the error from timestamp resolution alone.
    pub resolution_ms: u32,
}

impl ObservationTime {
    pub fn point(observed_at: DateTime<Utc>) -> Self {
        Self {
            source_start: Some(observed_at),
            source_end: None,
            resolution_ms: 1,
            observed_at,
            mono_start: None,
            mono_end: None,
            calibration: None,
            utc_offset_min: None,
        }
    }

    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if let (Some(s), Some(e)) = (self.source_start, self.source_end)
            && e < s
        {
            return Err(EnvelopeError::NegativeInterval);
        }
        if let (Some(s), Some(e)) = (&self.mono_start, &self.mono_end)
            && s.boot_id == e.boot_id
            && s.domain == e.domain
            && e.nanos < s.nanos
        {
            return Err(EnvelopeError::NegativeInterval);
        }
        Ok(())
    }

    /// Duration of a span, preferring the monotonic clock. `None` for point events.
    pub fn duration(&self) -> Option<Duration> {
        if let (Some(s), Some(e)) = (&self.mono_start, &self.mono_end)
            && s.boot_id == e.boot_id
            && s.domain == e.domain
            && e.nanos >= s.nanos
        {
            return Some(Duration {
                nanos: e.nanos - s.nanos,
                basis: DurationBasis::Monotonic,
                resolution_ms: 0,
            });
        }
        let (s, e) = (self.source_start?, self.source_end?);
        let nanos = (e - s).num_nanoseconds().filter(|n| *n >= 0)? as u64;
        Some(Duration {
            nanos,
            basis: DurationBasis::WallClock,
            resolution_ms: self.resolution_ms,
        })
    }
}

/// A half-open UTC interval `[start, end)`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct Interval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Interval {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Option<Self> {
        (end >= start).then_some(Self { start, end })
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
    pub fn len(&self) -> chrono::Duration {
        self.end - self.start
    }
    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        self.start <= t && t < self.end
    }
    pub fn intersect(&self, o: &Interval) -> Option<Interval> {
        let s = self.start.max(o.start);
        let e = self.end.min(o.end);
        (s < e).then_some(Interval { start: s, end: e })
    }

    /// Split at every civil-midnight boundary for a fixed UTC offset.
    pub fn split_civil_days(&self, utc_offset_min: i16) -> Vec<Interval> {
        let off = chrono::Duration::minutes(utc_offset_min.into());
        let mut out = Vec::new();
        let mut cur = self.start;
        while cur < self.end {
            let local = (cur + off).date_naive();
            let next_midnight = local
                .succ_opt()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|n| n.and_utc() - off)
                .unwrap_or(self.end);
            let e = next_midnight.min(self.end);
            out.push(Interval { start: cur, end: e });
            cur = e;
        }
        out
    }
}

/// Total covered time of possibly overlapping intervals (their union).
pub fn union_len(intervals: &[Interval]) -> chrono::Duration {
    let mut v: Vec<Interval> = intervals
        .iter()
        .copied()
        .filter(|i| !i.is_empty())
        .collect();
    v.sort();
    let mut total = chrono::Duration::zero();
    let mut cur: Option<Interval> = None;
    for i in v {
        match cur {
            Some(c) if i.start <= c.end => {
                cur = Some(Interval {
                    start: c.start,
                    end: c.end.max(i.end),
                })
            }
            Some(c) => {
                total += c.len();
                cur = Some(i);
            }
            None => cur = Some(i),
        }
    }
    if let Some(c) = cur {
        total += c.len();
    }
    total
}
