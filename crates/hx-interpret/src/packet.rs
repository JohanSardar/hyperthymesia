use crate::*;
use hx_schema::Sensitivity;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TEXT: usize = 2048;
/// The limit covers serialized packet UTF-8 bytes, a conservative character bound.
/// Budgets below 4096 bytes are raised to 4096 to leave room for provenance.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct PacketBudget {
    pub max_chars: usize,
    pub max_item_chars: usize,
}
impl Default for PacketBudget {
    fn default() -> Self {
        Self {
            max_chars: 400_000,
            max_item_chars: MAX_TEXT,
        }
    }
}

// Breadth-first bisection spreads even a short prefix over the entire time range.
fn spread(items: &[EvidenceItem]) -> Vec<EvidenceItem> {
    if items.is_empty() {
        return vec![];
    }
    let mut indices = vec![0];
    if items.len() > 1 {
        indices.push(items.len() - 1);
    }
    let mut ranges = std::collections::VecDeque::from([(1, items.len().saturating_sub(1))]);
    while let Some((lo, hi)) = ranges.pop_front() {
        if lo >= hi {
            continue;
        }
        let mid = (lo + hi) / 2;
        indices.push(mid);
        ranges.push_back((lo, mid));
        ranges.push_back((mid + 1, hi));
    }
    // Prefer the user request nearest each temporal stratum, rather than taking
    // a chronological prefix of prompts (which can itself be front-loaded).
    let users: Vec<_> = (0..items.len())
        .filter(|i| {
            serde_json::from_str::<serde_json::Value>(&items[*i].text)
                .ok()
                .is_some_and(|v| v["payload"]["role"] == "user" || v["role"] == "user")
        })
        .collect();
    let mut ordered = indices.iter().take(2).copied().collect::<Vec<_>>();
    let bucket = |i: usize| {
        let elapsed = (items[i].at - items[0].at).num_milliseconds().max(0) as u64;
        let extent = (items[items.len() - 1].at - items[0].at)
            .num_milliseconds()
            .max(1) as u64;
        (elapsed.saturating_mul(4) / extent).min(3)
    };
    let mut covered: BTreeSet<_> = ordered.iter().map(|i| bucket(*i)).collect();
    let mut seen: BTreeSet<_> = ordered.iter().copied().collect();
    for index in indices.iter().skip(2) {
        if let Some(nearest) = users.iter().filter(|i| !seen.contains(*i)).min_by_key(|i| {
            (
                covered.contains(&bucket(**i)),
                (items[**i].at - items[*index].at)
                    .num_milliseconds()
                    .unsigned_abs(),
                **i,
            )
        }) {
            seen.insert(*nearest);
            covered.insert(bucket(*nearest));
            ordered.push(*nearest);
        }
    }
    ordered.extend(indices.into_iter().filter(|i| !seen.contains(i)));
    ordered.into_iter().map(|i| items[i].clone()).collect()
}

fn group_key(e: &EvidenceItem, fallback: &BTreeMap<EvidenceId, String>) -> String {
    let key = serde_json::from_str::<serde_json::Value>(&e.text)
        .ok()
        .and_then(|v| {
            let connector = v["connector_instance"].as_str()?;
            Some(
                serde_json::json!([
                    connector,
                    v["subject"],
                    v["resource"],
                    v["payload"]["thread_id"],
                    v["payload"]["session_id"],
                    v["payload"]["repo"]
                ])
                .to_string(),
            )
        })
        .unwrap_or_else(|| {
            fallback
                .get(&e.id)
                .cloned()
                .unwrap_or_else(|| e.kind.clone())
        });
    if e.kind.starts_with("sensitive:") {
        format!(
            "sensitive-group:{}",
            hex::encode(Sha256::digest(key.as_bytes()))
        )
    } else {
        key
    }
}

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
            effective_class: sensitivity,
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

/// The timeline and memory/task inputs must already have passed the server egress policy.
/// No sensitivity metadata exists for these frozen inputs.
pub fn build_packet(
    timeline: &Timeline,
    evidence: &[EvidenceItem],
    entities: &[String],
    open_tasks: &[Task],
) -> Packet {
    build_packet_with_budget(
        timeline,
        evidence,
        entities,
        open_tasks,
        PacketBudget::default(),
    )
}

/// Explicit configuration avoids process-global environment changes during replay.
pub fn build_packet_with_budget(
    timeline: &Timeline,
    evidence: &[EvidenceItem],
    entities: &[String],
    open_tasks: &[Task],
    budget: PacketBudget,
) -> Packet {
    let limit = budget.max_chars.max(4096);
    let canonical_inputs = serde_json::json!({"timeline": timeline, "evidence": evidence, "entities": entities, "open_tasks": open_tasks, "prompt_version": PROMPT_VERSION, "budget": budget});
    let mut fallback = BTreeMap::new();
    for lane in &timeline.lanes {
        for span in &lane.intervals {
            let identity = serde_json::json!([
                lane.id.key,
                lane.id.kind,
                span.attrs["thread_id"],
                span.attrs["session_id"],
                span.attrs["repo"]
            ])
            .to_string();
            for id in &span.evidence {
                fallback.entry(*id).or_insert_with(|| identity.clone());
            }
        }
    }
    for point in &timeline.points {
        fallback
            .entry(point.evidence)
            .or_insert_with(|| point.lane_key.clone());
    }
    let mut groups: BTreeMap<String, Vec<EvidenceItem>> = BTreeMap::new();
    let mut lead = Vec::new();
    let mut seen = BTreeSet::new();
    let mut sorted = evidence.to_vec();
    sorted.sort_by(|a, b| {
        (a.at, a.id, &a.kind, &a.text, a.self_generated).cmp(&(
            b.at,
            b.id,
            &b.kind,
            &b.text,
            b.self_generated,
        ))
    });
    for mut e in sorted {
        if e.effective_class == Sensitivity::Restricted
            || !(e.kind.starts_with("normal:") || e.kind.starts_with("sensitive:"))
            || e.at < timeline.window.start - chrono::Duration::minutes(30)
            || e.at >= timeline.window.end
            || !seen.insert(e.id)
        {
            continue;
        }
        let key = group_key(&e, &fallback);
        let instruction_only =
            crate::validation::instruction_only(&e.text) || e.kind.ends_with(":instruction-only");
        e.kind = bounded(&e.kind, 160);
        if instruction_only {
            e.kind.push_str(":instruction-only");
        }
        e.text = if e.kind.starts_with("sensitive:") {
            "[sensitive content redacted]".into()
        } else {
            e.text
        };
        e.self_generated |= e.kind.to_ascii_lowercase().contains("hyperthymesia");
        if e.at < timeline.window.start {
            lead.push(e);
        } else {
            groups.entry(key).or_default().push(e);
        }
    }
    let digest_groups = groups.clone();
    let queues: Vec<_> = groups.values().map(|g| spread(g)).collect();
    let mut evidence = Vec::new();
    let mut used = 0;
    let evidence_limit = limit / 4 * 3;
    let window_limit = evidence_limit - if lead.is_empty() { 0 } else { limit / 20 };
    // Reserve an equal first-round share so a large first item cannot starve later groups.
    for round in 0..queues.iter().map(Vec::len).max().unwrap_or(0) {
        for queue in &queues {
            if let Some(item) = queue.get(round) {
                let mut item = item.clone();
                item.text = bounded(&item.text, budget.max_item_chars);
                if round == 0 {
                    item.text = bounded(
                        &item.text,
                        window_limit
                            .saturating_div(queues.len().max(1))
                            .saturating_sub(256),
                    );
                }
                if round == 0 {
                    let share = window_limit / queues.len().max(1);
                    while serde_json::to_vec(&item)
                        .expect("evidence serializes")
                        .len()
                        + 1
                        > share
                        && !item.text.is_empty()
                    {
                        item.text = bounded(&item.text, item.text.len() / 2);
                    }
                }
                let cost = serde_json::to_vec(&item)
                    .expect("evidence serializes")
                    .len()
                    + 1;
                if used + cost <= window_limit {
                    used += cost;
                    evidence.push(item);
                }
            }
        }
    }
    // Context is at most 5% by item count, and at most 5% of the total budget.
    let mut lead_bytes = 0;
    for mut item in spread(&lead).into_iter().take(evidence.len() / 19) {
        item.text = bounded(&item.text, budget.max_item_chars);
        let cost = serde_json::to_vec(&item)
            .expect("evidence serializes")
            .len()
            + 1;
        if used + cost <= evidence_limit && lead_bytes + cost <= limit / 20 {
            used += cost;
            lead_bytes += cost;
            evidence.push(item);
        }
    }
    evidence.sort_by_key(|e| (e.at, e.id));
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
        // The caller compiles only egress-cleared observations, so a span is citable once
        // any of its evidence made the packet; selection merely sampled the rest.
        .filter(|span| {
            span.evidence.iter().any(|id| {
                allowed
                    .get(id)
                    .is_some_and(|e| e.at >= timeline.window.start)
            })
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
    support = support
        .into_iter()
        .filter_map(|s| {
            if s.start == s.end {
                (s.start >= timeline.window.start && s.start < timeline.window.end).then_some(s)
            } else {
                s.intersect(&timeline.window)
            }
        })
        .collect();
    support.sort_by_key(|s| (s.start, s.end));
    support.dedup();
    // Select evenly across support boundaries too; never invent support between samples.
    let support_slots = (limit / 12 / 100).max(1).min(support.len());
    if support_slots < support.len() {
        support = (0..support_slots)
            .map(|i| support[i * (support.len() - 1) / support_slots.saturating_sub(1).max(1)])
            .collect();
    }
    let mut lines = vec![
        format!("compiler_version={}", timeline.compiler_version),
        format!(
            "time_support={}",
            serde_json::to_string(&support).expect("intervals serialize")
        ),
        "Unlisted time has unknown coverage; never infer idle or engagement.".into(),
    ];
    for (key, items) in &digest_groups {
        let ids: Vec<_> = items
            .iter()
            .filter(|e| allowed.contains_key(&e.id))
            .map(|e| e.id)
            .collect();
        if !ids.is_empty() {
            lines.push(format!(
                "group_digest={} in_window_count={} source_span={}..{} retained_evidence={:?}",
                bounded(key, 512),
                items.len(),
                items.first().unwrap().at,
                items.last().unwrap().at,
                ids
            ));
        }
    }
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
        timeline_text: String::new(),
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
    // Keep complete lines, especially time_support JSON, within the serialized budget.
    // Group digests precede detailed lanes/points so high-volume noise cannot hide them.
    for line in lines {
        let cost = serde_json::to_string(&line).expect("line serializes").len() + 2;
        if packet.timeline_text.len() + cost > limit / 5 {
            continue;
        }
        packet.timeline_text.push_str(&line);
        packet.timeline_text.push('\n');
    }
    // serde_json's ordered object maps give stable key order, including nested attrs.
    packet.input_digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical_inputs).expect("inputs serialize"),
    ));
    while serde_json::to_vec(&packet)
        .expect("packet serializes")
        .len()
        > limit
    {
        if packet.open_tasks.pop().is_some() || packet.entities.pop().is_some() {
            continue;
        }
        if let Some(end) = packet.timeline_text.trim_end().rfind('\n') {
            packet.timeline_text.truncate(end + 1);
        } else {
            packet.timeline_text.clear();
        }
    }
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
