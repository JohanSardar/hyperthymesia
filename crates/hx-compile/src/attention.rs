//! Deterministic, conservative attention inference. Silence is meaningful only
//! during fresh running collector health, never across a capture outage.
use crate::*;
use chrono::Duration;
use hx_schema::kinds::{CollectorStatus, CorePayload as Payload, DeviceState};
use std::collections::{BTreeMap, BTreeSet};

/// All attention thresholds, shared by replay and production compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionConfig {
    pub input_gap_s: u32,
    pub reading_max_s: u32,
    pub substantial_text_chars: usize,
    /// Collector heartbeats expire so a stopped agent cannot imply absence.
    pub health_max_s: u32,
    /// A changed frame proves watching only for this bounded period.
    pub visual_change_s: u32,
}
impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            input_gap_s: 30,
            reading_max_s: 120,
            substantial_text_chars: 200,
            health_max_s: 60,
            visual_change_s: 30,
        }
    }
}
fn at(o: &Observation) -> DateTime<Utc> {
    o.envelope
        .time
        .source_start
        .unwrap_or(o.envelope.time.observed_at)
}
fn end(o: &Observation) -> DateTime<Utc> {
    o.envelope.time.source_end.unwrap_or(at(o))
}
fn after(t: DateTime<Utc>, seconds: u32) -> DateTime<Utc> {
    t.checked_add_signed(Duration::seconds(i64::from(seconds)))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}
fn input(o: &Observation) -> bool {
    matches!(o.envelope.core_payload(), Some(Ok(Payload::InputBucket(p)))
        if p.keys > 0 || p.clicks > 0 || p.mouse_px > 0 || p.scroll_events > 0)
}

pub(super) fn derive(
    active: &[&Observation],
    window: Interval,
    config: AttentionConfig,
) -> Vec<Lane> {
    let mut groups = BTreeMap::<String, Vec<&Observation>>::new();
    for o in active {
        if let Some(device) = &o.envelope.device {
            groups
                .entry(format!("device:{device}"))
                .or_default()
                .push(o);
        }
    }
    let mut lanes = Vec::new();
    for (key, mut events) in groups {
        // Capture-capable devices get explicit unknown support even before
        // input health arrives. A lock/sleep record is sufficient on its own.
        if !events.iter().any(|o| matches!(o.envelope.kind.as_str(), "input.bucket" | "focus.span" | "presence.span")
            || matches!(o.envelope.core_payload(), Some(Ok(Payload::CollectorHealth(p))) if p.collector == "input")) {
            continue;
        }
        events.sort_by_key(|o| (at(o), o.envelope.sequence, o.id));
        let inputs: Vec<_> = events.iter().copied().filter(|o| input(o)).collect();
        let gaps: Vec<_> = events
            .iter()
            .copied()
            .filter(|o| o.envelope.kind.as_str() == "policy.gap")
            .collect();
        let mut bounds = BTreeSet::from([window.start, window.end]);
        for o in &events {
            for t in [
                at(o),
                end(o),
                after(at(o), config.health_max_s),
                after(at(o), config.input_gap_s),
                after(at(o), config.reading_max_s),
                after(at(o), config.visual_change_s),
            ] {
                if t > window.start && t < window.end {
                    bounds.insert(t);
                }
            }
        }
        let bounds: Vec<_> = bounds.into_iter().collect();
        let mut health = BTreeMap::<String, (&Observation, CollectorStatus)>::new();
        let mut focus: Option<&Observation> = None;
        let mut text: Option<&Observation> = None;
        let mut presence: Option<&Observation> = None;
        let mut last_input: Option<&Observation> = None;
        let mut still: Option<&Observation> = None;
        let mut changed: Option<(&Observation, &Observation)> = None;
        let mut hashes = BTreeMap::new();
        let mut input_healthy_since = None;
        let mut cursor = 0;
        let mut spans: Vec<LaneInterval> = Vec::new();
        for pair in bounds.windows(2) {
            let start = pair[0];
            while let Some(o) = events.get(cursor).copied().filter(|o| at(o) <= start) {
                if o.envelope.kind.as_str() == "policy.gap" {
                    still = None;
                    changed = None;
                    hashes.clear();
                }
                match o.envelope.core_payload() {
                    Some(Ok(Payload::CollectorHealth(p))) => {
                        let collector = p.collector.trim_end_matches(".span");
                        let previous = health.get(collector);
                        let continuous = previous.is_some_and(|(old, status)| {
                            *status == CollectorStatus::Running
                                && after(at(old), config.health_max_s) >= at(o)
                        });
                        let dropped = previous.map_or(p.dropped > 0, |(old, _)| {
                            old.envelope.payload["dropped"]
                                .as_u64()
                                .is_some_and(|n| p.dropped > n)
                        });
                        let status = if dropped {
                            CollectorStatus::Failed
                        } else {
                            p.status
                        };
                        if collector == "input"
                            && (!continuous || status != CollectorStatus::Running)
                        {
                            input_healthy_since =
                                (status == CollectorStatus::Running).then_some(at(o));
                        }
                        if collector == "media"
                            && (!continuous || status != CollectorStatus::Running)
                            && (previous.is_some() || still.is_none_or(|s| at(s) < at(o)))
                        {
                            if still.is_some_and(|s| at(s) < at(o)) {
                                still = None;
                            }
                            changed = None;
                            hashes.clear();
                        }
                        health.insert(collector.into(), (o, status));
                    }
                    Some(Ok(Payload::FocusSpan(_))) => {
                        focus = Some(o);
                        text = None;
                        still = None;
                        changed = None;
                    }
                    Some(Ok(Payload::TextSnapshot(_))) => text = Some(o),
                    Some(Ok(Payload::PresenceSpan(_))) => presence = Some(o),
                    Some(Ok(Payload::InputBucket(_))) if input(o) => last_input = Some(o),
                    Some(Ok(Payload::MediaStill(_))) | Some(Ok(Payload::MediaChunk(_))) => {
                        let display = o.envelope.payload["display"].as_str().unwrap_or("");
                        let hash_key = (o.envelope.kind.to_string(), display.to_owned());
                        if let Some(hash) = o.envelope.blobs.first() {
                            if let Some((previous_hash, previous)) =
                                hashes.insert(hash_key, (hash.sha256.clone(), o))
                                && previous_hash != hash.sha256
                            {
                                changed = Some((previous, o));
                            }
                            still = Some(o);
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
            let healthy = |collector: &str| {
                health.get(collector).is_some_and(|(o, status)| {
                    *status == CollectorStatus::Running && after(at(o), config.health_max_s) > start
                })
            };
            let mut evidence = BTreeSet::new();
            for (o, _) in health.values() {
                evidence.insert(o.id);
            }
            for o in [focus, text, presence, last_input, still]
                .into_iter()
                .flatten()
            {
                evidence.insert(o.id);
            }
            // Include the next nonzero bucket: it bounds the inferred silent run.
            if let Some(next) = inputs.get(inputs.partition_point(|o| at(o) <= start)) {
                evidence.insert(next.id);
            }
            let gap = gaps.iter().find(|o| {
                at(o) <= start
                    && (end(o) > start
                        || (end(o) == at(o) && after(at(o), config.health_max_s) > start))
            });
            if let Some(o) = gap {
                evidence.insert(o.id);
            }
            let locked = presence.is_some_and(|o| {
                (end(o) == at(o) || end(o) > start) && matches!(o.envelope.core_payload(), Some(Ok(Payload::PresenceSpan(p)))
                    if matches!(p.state, DeviceState::Locked | DeviceState::DeviceAsleep | DeviceState::DisplayOff))
            });
            let focused = focus.filter(|o| end(o) == at(o) || end(o) > start);
            let audio = focused.is_some_and(|o| {
                o.envelope.payload["window_title"]
                    .as_str()
                    .is_some_and(|s| s.to_ascii_lowercase().contains("audio playing"))
            });
            let recent_change =
                changed.filter(|(_, o)| after(at(o), config.visual_change_s) > start);
            if let Some((previous, o)) = recent_change {
                evidence.extend([previous.id, o.id]);
            }
            let engaged = last_input.is_some_and(|o| after(at(o), config.input_gap_s) > start);
            let reading = last_input.is_some_and(|o| after(at(o), config.reading_max_s) > start)
                && text.is_some_and(|o| matches!(o.envelope.core_payload(), Some(Ok(Payload::TextSnapshot(p)))
                    if p.visible != Some(false)
                        && focused.is_some_and(|f| f.envelope.payload["app_bundle_id"].as_str() == Some(p.surface.as_str()))
                        && p.text.chars().count() >= config.substantial_text_chars));
            let insufficient_silence = last_input.is_none()
                && input_healthy_since.is_none_or(|s| after(s, config.input_gap_s) > start);
            let state = if gap.is_some() {
                "unknown"
            } else if locked {
                "locked_or_asleep"
            } else if !healthy("input") {
                "unknown"
            } else if engaged {
                "engaged"
            } else if (audio && healthy("focus")) || recent_change.is_some() {
                "watching"
            } else if insufficient_silence
                || !healthy("media")
                || !healthy("focus")
                || focused.is_none()
                || still.is_none()
            {
                "unknown"
            } else if reading {
                "reading"
            } else {
                "probably_away"
            };
            let evidence: Vec<_> = evidence.into_iter().collect();
            if let Some(last) = spans.last_mut()
                && last.label == state
                && last.evidence == evidence
                && last.interval.end == start
            {
                last.interval.end = pair[1];
            } else {
                spans.push(LaneInterval {
                    interval: Interval {
                        start,
                        end: pair[1],
                    },
                    label: state.into(),
                    attrs: serde_json::json!({"basis":"inferred", "config":config}),
                    evidence,
                    uncertain_edges: true,
                });
            }
        }
        lanes.push(Lane {
            id: LaneId {
                key,
                kind: "attention".into(),
            },
            intervals: spans,
        });
    }
    lanes
}
