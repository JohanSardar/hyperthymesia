use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn check(interpretation: &Interpretation, packet: &Packet) -> Vec<CheckFailure> {
    check_and_repair(&mut interpretation.clone(), packet)
}

fn valid_intervals(intervals: &[Interval], window: Interval) -> bool {
    let mut sorted = intervals.to_vec();
    sorted.sort_by_key(|i| i.start);
    !sorted.is_empty()
        && sorted.iter().all(|i| {
            i.start <= i.end
                && i.start >= window.start
                && i.start < window.end
                && i.end <= window.end
        })
        && sorted.windows(2).all(|pair| pair[0].end <= pair[1].start)
}

/// Repairs are deterministic and every removal/downgrade is audited.
pub fn check_and_repair(value: &mut Interpretation, packet: &Packet) -> Vec<CheckFailure> {
    let mut failures = Vec::new();
    let mut fail = |item: &str, reason: &str| {
        failures.push(CheckFailure {
            item: item.into(),
            reason: reason.into(),
        })
    };
    let evidence: BTreeMap<_, _> = packet.evidence.iter().map(|e| (e.id, e)).collect();
    let known = |ids: &[EvidenceId]| ids.iter().all(|id| evidence.contains_key(id));
    let independent = |id: &EvidenceId| {
        evidence.get(id).is_some_and(|e| {
            !e.self_generated && !e.kind.starts_with("sensitive:") && !instruction_only(&e.text)
        })
    };
    if value.window != packet.window {
        fail("window", "replaced mismatched window");
        value.window = packet.window;
    }
    let support: Vec<Interval> = packet
        .timeline_text
        .lines()
        .find_map(|line| line.strip_prefix("time_support="))
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let supported = |intervals: &[Interval]| {
        intervals.iter().all(|i| {
            support
                .iter()
                .any(|s| s.start == i.start || s.end == i.start)
                && support.iter().any(|s| s.start == i.end || s.end == i.end)
                && hx_schema::union_len(
                    &support
                        .iter()
                        .filter_map(|s| s.intersect(i))
                        .collect::<Vec<_>>(),
                ) == i.end - i.start
        })
    };
    let mut episode_ids = BTreeSet::new();
    value.episodes.retain_mut(|episode| {
        if !episode_ids.insert(episode.id.clone()) || episode.id.is_empty() {
            fail(&episode.id, "dropped duplicate or empty episode id");
            return false;
        }
        if episode.evidence.is_empty() {
            fail(&episode.id, "dropped episode without evidence");
            return false;
        }
        if !known(&episode.evidence) {
            fail(&episode.id, "rejected episode with unknown evidence id");
            return false;
        }
        if !valid_intervals(&episode.intervals, packet.window) || !supported(&episode.intervals) {
            fail(&episode.id, "rejected invalid episode intervals");
            return false;
        }
        if !episode.confidence.is_finite() || !(0.0..=1.0).contains(&episode.confidence) {
            fail(&episode.id, "rejected invalid confidence");
            return false;
        }
        episode.claims.retain_mut(|claim| {
            let item = format!("{}/{}", episode.id, claim.action);
            if !known(&claim.evidence) || !known(&claim.outcome_evidence) {
                fail(&item, "rejected claim with unknown evidence id");
                return false;
            }
            if !claim.evidence.iter().any(independent) {
                fail(
                    &item,
                    "dropped claim without independent non-instruction evidence",
                );
                return false;
            }
            if !["completed", "attempted", "unknown"].contains(&claim.outcome.as_str()) {
                fail(&item, "rejected unknown outcome");
                return false;
            }
            if claim.outcome == "completed" && !claim.outcome_evidence.iter().any(independent) {
                claim.outcome = "attempted".into();
                fail(
                    &item,
                    "downgraded completion without independent outcome evidence",
                );
            }
            true
        });
        true
    });
    let episode_ids: BTreeSet<_> = value.episodes.iter().map(|e| e.id.clone()).collect();
    let mut task_ids = BTreeSet::new();
    value.tasks.retain(|task| {
        let reason = if task.id.is_empty() || !task_ids.insert(task.id.clone()) {
            Some("rejected duplicate or empty task id")
        } else if !valid_intervals(&task.intervals, packet.window) || !supported(&task.intervals) {
            Some("rejected task intervals outside window or overlapping")
        } else if task.episode_ids.is_empty()
            || task.episode_ids.iter().any(|id| !episode_ids.contains(id))
        {
            Some("rejected task without valid episodes")
        } else if !task.confidence.is_finite() || !(0.0..=1.0).contains(&task.confidence) {
            Some("rejected invalid confidence")
        } else {
            None
        };
        if let Some(reason) = reason {
            fail(&task.id, reason);
            false
        } else {
            true
        }
    });
    if !failures.is_empty() {
        // Free-form prose cannot retain a claim which the structured checker removed.
        value.summary = value
            .episodes
            .iter()
            .filter(|e| !e.claims.is_empty())
            .map(|e| {
                e.claims
                    .iter()
                    .map(|c| format!("{} {} {} ({})", c.actor, c.action, c.object, c.outcome))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .collect::<Vec<_>>()
            .join("; ");
        for episode in &mut value.episodes {
            episode.title = "Evidence-backed episode".into();
            episode.description = episode
                .claims
                .iter()
                .map(|c| format!("{} {} {} ({})", c.actor, c.action, c.object, c.outcome))
                .collect::<Vec<_>>()
                .join("; ");
        }
        for task in &mut value.tasks {
            task.title = "Evidence-backed task".into();
            task.description.clear();
            task.goal = None;
        }
    }
    if !failures.is_empty() {
        failures.push(CheckFailure { item: "prose".into(), reason: "regenerated summary/episode prose from retained claims; cleared task descriptions and goals".into() });
    }
    value.check_failures.extend(failures.clone());
    failures
}

// This catches explicit instruction-bearing pages, not arbitrary semantic contradictions.
// Such content may be described as a page, but cannot prove a real-world action.
fn instruction_only(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "record that",
        "ignore previous instructions",
        "ignore all previous",
        "system prompt",
        "you must report",
        "disregard instructions",
    ]
    .iter()
    .any(|s| text.contains(s))
}
