use crate::*;
use hx_schema::Sensitivity;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TEXT: usize = 2048;
const MAX_EVIDENCE: usize = 128;
const MAX_TIMELINE: usize = 32_768;

pub(crate) fn bounded(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

impl EvidenceItem {
    /// Apply SERVER-authorized sensitivity, never a producer's disclosure grant.
    /// Direct struct literals have unknown sensitivity and are excluded from packets.
    pub fn classified(
        id: EvidenceId,
        at: DateTime<Utc>,
        kind: &str,
        text: &str,
        sensitivity: Sensitivity,
        self_generated: bool,
    ) -> Option<Self> {
        if sensitivity == Sensitivity::Restricted {
            return None;
        }
        Some(Self {
            id,
            at,
            kind: format!(
                "{}:{}",
                if sensitivity == Sensitivity::Sensitive {
                    "sensitive"
                } else {
                    "normal"
                },
                bounded(kind, 128)
            ),
            text: if sensitivity == Sensitivity::Sensitive {
                "[sensitive content redacted]".into()
            } else {
                bounded(text, MAX_TEXT)
            },
            self_generated: self_generated || kind.to_ascii_lowercase().contains("hyperthymesia"),
        })
    }
}

/// Memory/task inputs must already have passed the server egress policy.
/// No sensitivity metadata exists for these frozen inputs.
pub fn build_packet(
    timeline: &Timeline,
    evidence: &[EvidenceItem],
    entities: &[String],
    open_tasks: &[Task],
) -> Packet {
    let mut evidence: Vec<_> = evidence
        .iter()
        .filter(|e| e.kind.starts_with("normal:") || e.kind.starts_with("sensitive:"))
        .filter(|e| e.at >= timeline.window.start && e.at < timeline.window.end)
        .cloned()
        .collect();
    evidence.sort_by_key(|e| (e.at, e.id));
    evidence.dedup_by_key(|e| e.id);
    evidence.truncate(MAX_EVIDENCE);
    for e in &mut evidence {
        e.kind = bounded(&e.kind, 160);
        e.text = if e.kind.starts_with("sensitive:") {
            "[sensitive content redacted]".into()
        } else {
            bounded(&e.text, MAX_TEXT)
        };
        e.self_generated |= e.kind.to_ascii_lowercase().contains("hyperthymesia");
    }
    let allowed: BTreeMap<_, _> = evidence.iter().map(|e| (e.id, e)).collect();
    let label = |text: &str, ids: &[EvidenceId]| {
        if ids.is_empty() || ids.iter().any(|id| !allowed.contains_key(id)) {
            "[unavailable or restricted]".into()
        } else if ids
            .iter()
            .any(|id| allowed[id].kind.starts_with("sensitive:"))
        {
            "[sensitive content redacted]".into()
        } else {
            bounded(text, 512)
        }
    };
    let mut support: Vec<_> = timeline
        .lanes
        .iter()
        .flat_map(|lane| &lane.intervals)
        .filter(|span| {
            !span.evidence.is_empty() && span.evidence.iter().all(|id| allowed.contains_key(id))
        })
        .map(|span| span.interval)
        .collect();
    support.extend(
        timeline
            .points
            .iter()
            .filter(|p| allowed.contains_key(&p.evidence))
            .map(|p| Interval {
                start: p.at,
                end: p.at,
            }),
    );
    let mut lines = vec![
        format!("compiler_version={}", timeline.compiler_version),
        format!(
            "time_support={}",
            serde_json::to_string(&support).expect("intervals serialize")
        ),
        "Unlisted time has unknown coverage; never infer idle or engagement.".into(),
    ];
    for (index, lane) in timeline.lanes.iter().enumerate() {
        for span in &lane.intervals {
            lines.push(format!(
                "lane {index} {} {} uncertain_edges={} evidence={:?} {}",
                span.interval.start,
                span.interval.end,
                span.uncertain_edges,
                span.evidence
                    .iter()
                    .filter(|id| allowed.contains_key(id))
                    .collect::<Vec<_>>(),
                label(&span.label, &span.evidence)
            ));
        }
    }
    for span in &timeline.coverage {
        lines.push(format!(
            "coverage {} {} {:?}",
            span.interval.start, span.interval.end, span.state
        ));
    }
    for point in &timeline.points {
        lines.push(format!(
            "point {} evidence={:?} {}",
            point.at,
            allowed.get(&point.evidence).map(|e| e.id),
            label(&point.label, &[point.evidence])
        ));
    }
    for card in &timeline.aggregates {
        lines.push(format!(
            "aggregate (no second-level placement) seconds_estimated={} {}",
            card.seconds_estimated,
            label(&card.label, &card.evidence)
        ));
    }
    let mut packet = Packet {
        window: timeline.window,
        timeline_text: bounded(&lines.join("\n"), MAX_TIMELINE),
        evidence,
        entities: entities.iter().take(64).map(|s| bounded(s, 512)).collect(),
        open_tasks: open_tasks
            .iter()
            .take(64)
            .cloned()
            .map(|mut task| {
                task.id = bounded(&task.id, 128);
                task.title = bounded(&task.title, 512);
                task.description = bounded(&task.description, MAX_TEXT);
                task.intervals.truncate(128);
                task.episode_ids = task
                    .episode_ids
                    .into_iter()
                    .take(128)
                    .map(|id| bounded(&id, 128))
                    .collect();
                task.aliases = task
                    .aliases
                    .into_iter()
                    .take(64)
                    .map(|id| bounded(&id, 128))
                    .collect();
                task.goal = task.goal.map(|goal| bounded(&goal, 512));
                task
            })
            .collect(),
        input_digest: String::new(),
    };
    // serde_json's ordered object maps give stable key order, including nested attrs.
    let inputs = serde_json::json!({"timeline": timeline, "evidence": packet.evidence, "entities": packet.entities, "open_tasks": packet.open_tasks, "prompt_version": PROMPT_VERSION});
    packet.input_digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&inputs).expect("inputs serialize"),
    ));
    packet
}

pub(crate) fn compiler_version(packet: &Packet) -> String {
    packet
        .timeline_text
        .lines()
        .next()
        .and_then(|s| s.strip_prefix("compiler_version="))
        .unwrap_or("unknown")
        .into()
}

/// Assertion JSON lines, grounded in claim objects explicitly typed project/repo/person.
/// Prior tasks supply first-seen bounds only when linked by the same stable task id.
pub fn entity_assertions(prior_tasks: &[Task], interpretation: &Interpretation) -> Vec<String> {
    let mut assertions =
        BTreeMap::<String, (DateTime<Utc>, DateTime<Utc>, BTreeSet<EvidenceId>)>::new();
    for episode in &interpretation.episodes {
        for claim in &episode.claims {
            if !["project:", "repo:", "person:"]
                .iter()
                .any(|prefix| claim.object.starts_with(prefix))
                || claim.evidence.is_empty()
            {
                continue;
            }
            let Some(first) = episode.intervals.iter().map(|i| i.start).min() else {
                continue;
            };
            let last = episode
                .intervals
                .iter()
                .map(|i| i.end)
                .max()
                .unwrap_or(first);
            let prior_first = interpretation
                .tasks
                .iter()
                .filter(|t| t.episode_ids.contains(&episode.id))
                .flat_map(|task| prior_tasks.iter().filter(move |p| p.id == task.id))
                .flat_map(|t| t.intervals.iter().map(|i| i.start))
                .min()
                .unwrap_or(first);
            let entry = assertions.entry(claim.object.clone()).or_insert((
                first.min(prior_first),
                last,
                BTreeSet::new(),
            ));
            entry.0 = entry.0.min(first.min(prior_first));
            entry.1 = entry.1.max(last);
            entry.2.extend(&claim.evidence);
        }
    }
    assertions.into_iter().map(|(entity, (first_seen, last_seen, evidence))| serde_json::json!({"entity":entity,"first_seen":first_seen,"last_seen":last_seen,"evidence":evidence}).to_string()).collect()
}
