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
            !e.self_generated
                && !e.kind.starts_with("sensitive:")
                && !e.kind.ends_with(":instruction-only")
                && !instruction_only(&e.text)
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
    let original_episode_count = value.episodes.len();
    let mut changed_episodes = BTreeSet::new();
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
        let original_claims = episode.claims.clone();
        let mut claim_index = 0;
        episode.claims.retain_mut(|claim| {
            let item = format!("{}/claim[{claim_index}]", episode.id);
            claim_index += 1;
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
            // A source describing somebody's action is testimony, even when the
            // citation is independent. Mixed citations need a direct observation.
            if claim.basis == Basis::Observed
                && !claim.evidence.iter().any(|id| {
                    independent(id) && evidence.get(id).is_some_and(|e| observes_actor(e, claim))
                })
            {
                claim.basis = Basis::ReportedBySource;
                fail(&item, "changed observed action to reported-by-source");
            }
            if claim.outcome == "completed"
                && !claim.outcome_evidence.iter().any(|id| {
                    independent(id)
                        && evidence
                            .get(id)
                            .is_some_and(|e| supports_completion(e, claim))
                })
            {
                claim.outcome = "attempted".into();
                fail(
                    &item,
                    "downgraded completion without supported independent outcome evidence",
                );
            }
            true
        });
        if episode.claims != original_claims {
            changed_episodes.insert(episode.id.clone());
            episode.description = episode
                .claims
                .iter()
                .map(claim_prose)
                .collect::<Vec<_>>()
                .join("; ");
            episode.title = bounded_title(&episode.description);
            if episode.title.is_empty() {
                episode.title = "Activity unknown (no supported claims)".into();
            }
            fail(
                &episode.id,
                "regenerated episode prose from retained claims",
            );
        }
        true
    });
    let original_task_count = value.tasks.len();
    let episode_ids: BTreeSet<_> = value.episodes.iter().map(|e| e.id.clone()).collect();
    let mut task_ids = BTreeSet::new();
    value.tasks.retain_mut(|task| {
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
            if task
                .episode_ids
                .iter()
                .any(|id| changed_episodes.contains(id))
            {
                let episodes = ordered_episodes(
                    value
                        .episodes
                        .iter()
                        .filter(|e| task.episode_ids.contains(&e.id)),
                );
                task.title = bounded_title(&episode_titles(&episodes));
                task.description = episodes
                    .iter()
                    .map(|e| e.description.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                task.goal = None;
                fail(
                    &task.id,
                    "regenerated task prose from retained episodes; cleared goal",
                );
            }
            true
        }
    });
    if !changed_episodes.is_empty()
        || value.episodes.len() != original_episode_count
        || value.tasks.len() != original_task_count
    {
        // Summary prose has no references proving which removed assertion it used.
        value.summary = episode_titles(&ordered_episodes(value.episodes.iter()));
        fail(
            "summary",
            "regenerated summary from retained episode titles in time order",
        );
    }
    value.check_failures.extend(failures.clone());
    failures
}

/// Free-form text is not a machine-verifiable outcome. In particular, browsing,
/// checkout clicks, source statements and planned work cannot prove completion.
/// Accept only outcomes with a typed result and a claim bound to that exact result;
/// extend this allowlist when new outcome-bearing connector schemas are available.
fn supports_completion(e: &EvidenceItem, claim: &Claim) -> bool {
    if !observes_actor(e, claim) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&e.text) else {
        return false;
    };
    let payload = value.get("payload").unwrap_or(&value);
    match e.kind.strip_prefix("normal:") {
        Some("shell.command") => {
            matches!(
                claim.action.to_ascii_lowercase().as_str(),
                "run" | "execute"
            ) && payload["command"].as_str() == Some(claim.object.as_str())
                && payload["exit_code"].as_i64().is_some()
        }
        Some("code.commit") => {
            claim.action.eq_ignore_ascii_case("commit")
                && payload["sha"]
                    .as_str()
                    .is_some_and(|sha| !sha.is_empty() && sha == claim.object)
        }
        _ => false,
    }
}

// Unbound statements about somebody else are testimony. A named actor needs
// source-owned subject attribution; a model-provided name is not that binding.
fn observes_actor(e: &EvidenceItem, claim: &Claim) -> bool {
    !source_report(e)
        && (claim.actor == "subject"
            || serde_json::from_str::<serde_json::Value>(&e.text)
                .ok()
                .is_some_and(|value| value["subject"].as_str() == Some(claim.actor.as_str())))
}

fn source_report(e: &EvidenceItem) -> bool {
    if matches!(
        e.kind.strip_prefix("normal:"),
        Some("agent.message" | "message.event")
    ) {
        return true;
    }
    let text = e.text.to_ascii_lowercase();
    [
        "source report",
        "reported by",
        "reports that",
        "says ",
        "said ",
        "no direct observation",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn claim_prose(claim: &Claim) -> String {
    // Put the outcome first so truncating a title cannot hide an attempted outcome.
    format!(
        "{}: {}{} {} {}",
        claim.outcome,
        if claim.basis == Basis::ReportedBySource {
            "reported by source: "
        } else {
            ""
        },
        claim.actor,
        claim.action,
        claim.object
    )
}

fn bounded_title(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= 120 {
        text
    } else {
        text.chars().take(119).chain(std::iter::once('…')).collect()
    }
}

fn ordered_episodes<'a>(episodes: impl Iterator<Item = &'a Episode>) -> Vec<&'a Episode> {
    let mut episodes: Vec<_> = episodes.collect();
    episodes.sort_by_key(|e| e.intervals.iter().map(|i| i.start).min());
    episodes
}

fn episode_titles(episodes: &[&Episode]) -> String {
    episodes
        .iter()
        .map(|e| e.title.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

// This catches explicit instruction-bearing pages, not arbitrary semantic contradictions.
// Such content may be described as a page, but cannot prove a real-world action.
pub(crate) fn instruction_only(text: &str) -> bool {
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
