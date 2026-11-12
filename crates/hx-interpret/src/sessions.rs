//! Work sessions: the interpreter output that replaces episodes and tasks.
//!
//! Sessions group a window's active time by project or purpose. Together their
//! ranges must account for every active second of the fact sheet exactly once:
//! no overlaps, no inactive seconds, and a total equal to the ACTIVE TOTAL.
//! [`add_up`] checks that; [`repair_add_up`] is the last resort when the model
//! could not get it right, and records every second it moved.

use crate::{CheckFailure, Provenance};
use chrono::{DateTime, Duration, NaiveTime, Utc};
use hx_schema::Interval;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Stored in every sessions revision; readers of episodes/tasks check it.
pub const FORMAT: &str = "sessions/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Work,
    Research,
    Admin,
    Wasted,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    /// The result was visible on screen or in a source record.
    Seen,
    NotEstablished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub at: DateTime<Utc>,
    pub what: String,
    pub evidence: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub what: String,
    pub status: OutcomeStatus,
    pub evidence: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blocker {
    pub what: String,
    pub evidence: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub project: String,
    pub kind: SessionKind,
    /// Disjoint whole-second ranges, sorted.
    pub ranges: Vec<Interval>,
    /// Seconds in `ranges`, computed by the server.
    pub active_s: i64,
    pub summary: String,
    pub done: Vec<Step>,
    pub outcomes: Vec<Outcome>,
    pub blockers: Vec<Blocker>,
    /// Named threads, pages, documents or channels involved.
    pub sources: Vec<String>,
    pub evidence: Vec<i64>,
    pub borderline: Option<String>,
}

/// One window's sessions as published in a revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReplica {
    pub format: String,
    pub window: Interval,
    pub active_total_s: i64,
    pub active: Vec<Interval>,
    pub sessions: Vec<Session>,
    pub summary: String,
    pub provenance: Provenance,
    /// Citation drops and add-up repairs, kept for audit.
    pub check_failures: Vec<CheckFailure>,
}

// What the model writes: clock times on the fact sheet's printed seconds.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    sessions: Vec<ModelSession>,
    summary: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSession {
    project: String,
    kind: SessionKind,
    ranges: Vec<(String, String)>,
    summary: String,
    #[serde(default)]
    done: Vec<ModelStep>,
    #[serde(default)]
    outcomes: Vec<ModelOutcome>,
    #[serde(default)]
    blockers: Vec<Blocker>,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    evidence: Vec<i64>,
    #[serde(default)]
    borderline: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelStep {
    at: String,
    what: String,
    #[serde(default)]
    evidence: Vec<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelOutcome {
    what: String,
    status: OutcomeStatus,
    #[serde(default)]
    evidence: Vec<i64>,
}

/// A printed `HH:MM:SS` UTC time inside `window` (the end may be next midnight).
pub fn clock(text: &str, window: Interval) -> Result<DateTime<Utc>, String> {
    let time = NaiveTime::parse_from_str(text.trim(), "%H:%M:%S")
        .map_err(|_| format!("time {text:?} is not HH:MM:SS"))?;
    let mut at = window.start.date_naive().and_time(time).and_utc();
    if at < window.start {
        at += Duration::days(1);
    }
    if at > window.end {
        return Err(format!("time {text} is outside the window"));
    }
    Ok(at)
}

/// Parse the model's answer. Fences are tolerated; any other shape is an error.
pub fn parse(text: &str, window: Interval) -> Result<(Vec<Session>, String), String> {
    let output: Output =
        serde_json::from_str(crate::strip_fence(text)).map_err(|e| e.to_string())?;
    let mut sessions = Vec::new();
    for s in output.sessions {
        let mut ranges = Vec::new();
        for (a, b) in &s.ranges {
            let (start, end) = (clock(a, window)?, clock(b, window)?);
            if start >= end {
                return Err(format!(
                    "range {a}-{b} of {:?} is empty or reversed",
                    s.project
                ));
            }
            ranges.push(Interval { start, end });
        }
        let mut done = Vec::new();
        for step in s.done {
            done.push(Step {
                at: clock(&step.at, window)?,
                what: step.what,
                evidence: step.evidence,
            });
        }
        sessions.push(Session {
            project: s.project,
            kind: s.kind,
            active_s: 0,
            ranges,
            summary: s.summary,
            done,
            outcomes: s
                .outcomes
                .into_iter()
                .map(|o| Outcome {
                    what: o.what,
                    status: o.status,
                    evidence: o.evidence,
                })
                .collect(),
            blockers: s.blockers,
            sources: s.sources,
            evidence: s.evidence,
            borderline: s.borderline.filter(|b| !b.trim().is_empty()),
        });
    }
    Ok((sessions, output.summary))
}

/// Result of the add-up check. `problem_s` ranks attempts: lower is better.
#[derive(Debug, Clone, PartialEq)]
pub struct AddUp {
    pub covered_s: i64,
    pub active_total_s: i64,
    pub problem_s: i64,
    pub errors: Vec<String>,
}

impl AddUp {
    pub fn passed(&self) -> bool {
        self.errors.is_empty()
    }
}

type Span = (i64, i64);

fn spans(intervals: &[Interval]) -> Vec<Span> {
    let mut out: Vec<Span> = intervals
        .iter()
        .map(|i| (i.start.timestamp(), i.end.timestamp()))
        .filter(|(a, b)| a < b)
        .collect();
    out.sort_unstable();
    let mut merged: Vec<Span> = Vec::new();
    for (a, b) in out {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

fn minus(whole: &[Span], holes: &[Span]) -> Vec<Span> {
    let mut out = Vec::new();
    for &(mut a, b) in whole {
        for &(c, d) in holes {
            if d <= a || c >= b {
                continue;
            }
            if c > a {
                out.push((a, c));
            }
            a = a.max(d);
            if a >= b {
                break;
            }
        }
        if a < b {
            out.push((a, b));
        }
    }
    out
}

fn total(spans: &[Span]) -> i64 {
    spans.iter().map(|(a, b)| b - a).sum()
}

fn clock_of(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .map(crate::facts::hm)
        .unwrap_or_default()
}

fn show(spans: &[Span]) -> String {
    let mut parts: Vec<String> = spans
        .iter()
        .take(12)
        .map(|(a, b)| format!("{}-{} ({} s)", clock_of(*a), clock_of(*b), b - a))
        .collect();
    if spans.len() > 12 {
        parts.push(format!("and {} more", spans.len() - 12));
    }
    parts.join(", ")
}

/// Ranges must not overlap, must not include inactive seconds, and must sum
/// exactly to the active total.
pub fn add_up(sessions: &[Session], active: &[Interval]) -> AddUp {
    let active = spans(active);
    let active_total_s = total(&active);
    let mut errors = Vec::new();
    let mut problem_s = 0;
    let mut all: Vec<(i64, i64, usize)> = sessions
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            s.ranges
                .iter()
                .map(move |r| (r.start.timestamp(), r.end.timestamp(), i))
        })
        .collect();
    all.sort_unstable();
    let mut reach: Option<(i64, usize)> = None;
    let mut overlaps = 0;
    for &(a, b, i) in &all {
        if let Some((end, j)) = reach
            && a < end
        {
            let o = end.min(b) - a;
            problem_s += o;
            overlaps += 1;
            if overlaps <= 12 {
                errors.push(format!(
                    "overlap {}-{} ({o} s) between {:?} and {:?}",
                    clock_of(a),
                    clock_of(end.min(b)),
                    sessions[j].project,
                    sessions[i].project
                ));
            }
        }
        if reach.is_none_or(|(end, _)| b > end) {
            reach = Some((b, i));
        }
    }
    if overlaps > 12 {
        errors.push(format!("and {} more overlaps", overlaps - 12));
    }
    for s in sessions {
        let inactive = minus(&spans(&s.ranges), &active);
        if !inactive.is_empty() {
            problem_s += total(&inactive);
            errors.push(format!(
                "{:?} includes inactive time {}",
                s.project,
                show(&inactive)
            ));
        }
    }
    let covered = spans(
        &sessions
            .iter()
            .flat_map(|s| s.ranges.clone())
            .collect::<Vec<_>>(),
    );
    let uncovered = minus(&active, &covered);
    if !uncovered.is_empty() {
        problem_s += total(&uncovered);
        errors.push(format!("active time in no session: {}", show(&uncovered)));
    }
    let covered_s = total(&minus(&covered, &minus(&covered, &active)));
    let claimed: i64 = sessions
        .iter()
        .flat_map(|s| spans(&s.ranges))
        .map(|(a, b)| b - a)
        .sum();
    if !errors.is_empty() || claimed != active_total_s {
        errors.push(format!(
            "sessions claim {claimed} s; ACTIVE TOTAL is {active_total_s} s"
        ));
    }
    AddUp {
        covered_s,
        active_total_s,
        problem_s,
        errors,
    }
}

fn to_intervals(spans: &[Span]) -> Vec<Interval> {
    spans
        .iter()
        .filter_map(|(a, b)| {
            Some(Interval {
                start: DateTime::from_timestamp(*a, 0)?,
                end: DateTime::from_timestamp(*b, 0)?,
            })
        })
        .collect()
}

/// Make sessions add up exactly: drop inactive seconds, give an overlapped
/// second to the earlier-listed session, and give each uncovered active
/// stretch to the session that touches it (the one before, else after, else
/// the nearest). Every change is returned as an audit record.
pub fn repair_add_up(sessions: &mut Vec<Session>, active: &[Interval]) -> Vec<CheckFailure> {
    let active = spans(active);
    // Sessions written without ranges report source records outside active
    // time. They count 0 s and never take seconds.
    let source_only: Vec<bool> = sessions.iter().map(|s| s.ranges.is_empty()).collect();
    let mut failures = Vec::new();
    let mut claimed: Vec<Span> = Vec::new();
    for s in sessions.iter_mut() {
        let own = spans(&s.ranges);
        let inactive = minus(&own, &active);
        let kept = minus(&own, &inactive);
        let free = minus(&kept, &claimed);
        let taken = minus(&kept, &free);
        if !inactive.is_empty() {
            failures.push(repair(
                &s.project,
                format!("removed inactive time {}", show(&inactive)),
            ));
        }
        if !taken.is_empty() {
            failures.push(repair(
                &s.project,
                format!(
                    "removed time already in an earlier session {}",
                    show(&taken)
                ),
            ));
        }
        claimed = spans(&to_intervals(&[claimed, free.clone()].concat()));
        s.ranges = to_intervals(&free);
    }
    let uncovered = minus(&active, &claimed);
    let timed = |s: &Vec<Session>| s.iter().any(|s| !s.ranges.is_empty());
    let fallback = (!uncovered.is_empty() && !timed(sessions)).then_some(sessions.len());
    if fallback.is_some() {
        sessions.push(Session {
            project: "Unassigned active time".into(),
            kind: SessionKind::Other,
            ranges: vec![],
            active_s: 0,
            summary: "Active time the interpreter did not assign to a session.".into(),
            done: vec![],
            outcomes: vec![],
            blockers: vec![],
            sources: vec![],
            evidence: vec![],
            borderline: Some("Assigned by the add-up repair, not by the interpreter.".into()),
        });
    }
    for (a, b) in uncovered {
        let owner = |t: i64| {
            sessions.iter().position(|s| {
                s.ranges
                    .iter()
                    .any(|r| r.start.timestamp() <= t && t < r.end.timestamp())
            })
        };
        let (index, how) = match (owner(a - 1), owner(b)) {
            (Some(i), _) => (i, "the session before it"),
            (None, Some(i)) => (i, "the session after it"),
            (None, None) => {
                let distance = |s: &Session| {
                    s.ranges
                        .iter()
                        .map(|r| {
                            (r.start.timestamp() - b)
                                .abs()
                                .min((a - r.end.timestamp()).abs())
                        })
                        .min()
                        .unwrap_or(i64::MAX)
                };
                let nearest = (0..sessions.len())
                    .filter(|i| !sessions[*i].ranges.is_empty())
                    .min_by_key(|i| distance(&sessions[*i]))
                    .or(fallback)
                    .expect("a timed session or the fallback");
                (nearest, "the nearest session")
            }
        };
        let s = &mut sessions[index];
        s.ranges.extend(to_intervals(&[(a, b)]));
        s.ranges = to_intervals(&spans(&s.ranges));
        failures.push(repair(
            &s.project,
            format!(
                "assigned uncovered active time {} to {how}",
                show(&[(a, b)])
            ),
        ));
    }
    let mut index = 0;
    sessions.retain(|s| {
        let keep = !s.ranges.is_empty() || source_only.get(index).copied().unwrap_or(false);
        index += 1;
        keep
    });
    for s in sessions.iter_mut() {
        s.active_s = total(&spans(&s.ranges));
    }
    failures
}

fn repair(project: &str, reason: String) -> CheckFailure {
    CheckFailure {
        item: format!("add-up: {project}"),
        reason: format!("repaired: {reason}"),
    }
}

/// Keep only citable evidence. A step, outcome or blocker left with none is
/// removed; a session left with none cites the fact-sheet stretches it covers,
/// or, with no active time, is removed.
pub fn check_citations(
    sessions: &mut Vec<Session>,
    citable: &BTreeSet<i64>,
    stretches: &[crate::facts::Stretch],
    window: Interval,
) -> Vec<CheckFailure> {
    let mut failures = Vec::new();
    let keep = |ids: &mut Vec<i64>, item: &str, failures: &mut Vec<CheckFailure>| {
        let before = ids.len();
        ids.retain(|id| citable.contains(id));
        ids.sort_unstable();
        ids.dedup();
        if ids.len() < before {
            failures.push(CheckFailure {
                item: item.into(),
                reason: "dropped evidence ids not in the fact sheet or a lookup result".into(),
            });
        }
        !ids.is_empty()
    };
    for s in sessions.iter_mut() {
        let name = s.project.clone();
        s.done.retain_mut(|step| {
            let ok = keep(&mut step.evidence, &format!("{name}: step"), &mut failures)
                && window.contains(step.at);
            if !ok {
                failures.push(CheckFailure {
                    item: format!("{name}: step"),
                    reason: format!("removed uncited step {:?}", step.what),
                });
            }
            ok
        });
        s.outcomes.retain_mut(|o| {
            let ok = keep(&mut o.evidence, &format!("{name}: outcome"), &mut failures);
            if !ok {
                failures.push(CheckFailure {
                    item: format!("{name}: outcome"),
                    reason: format!("removed uncited outcome {:?}", o.what),
                });
            }
            ok
        });
        s.blockers.retain_mut(|b| {
            let ok = keep(&mut b.evidence, &format!("{name}: blocker"), &mut failures);
            if !ok {
                failures.push(CheckFailure {
                    item: format!("{name}: blocker"),
                    reason: format!("removed uncited blocker {:?}", b.what),
                });
            }
            ok
        });
        if !keep(&mut s.evidence, &name, &mut failures) {
            s.evidence = stretches
                .iter()
                .filter(|st| {
                    s.ranges
                        .iter()
                        .any(|r| r.intersect(&st.interval).is_some_and(|i| !i.is_empty()))
                })
                .flat_map(|st| st.evidence.first().copied())
                .filter(|id| citable.contains(id))
                .take(20)
                .collect();
            s.evidence.sort_unstable();
            s.evidence.dedup();
        }
    }
    sessions.retain(|s| {
        let keep = !s.ranges.is_empty() || !s.evidence.is_empty();
        if !keep {
            failures.push(CheckFailure {
                item: s.project.clone(),
                reason: "removed a session with no active time and no citable source records"
                    .into(),
            });
        }
        keep
    });
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Interval {
        Interval {
            start: "2026-01-01T10:00:00Z".parse().unwrap(),
            end: "2026-01-01T14:00:00Z".parse().unwrap(),
        }
    }
    fn iv(a: &str, b: &str) -> Interval {
        Interval {
            start: clock(a, window()).unwrap(),
            end: clock(b, window()).unwrap(),
        }
    }
    fn session(project: &str, ranges: Vec<Interval>) -> Session {
        Session {
            project: project.into(),
            kind: SessionKind::Work,
            ranges,
            active_s: 0,
            summary: String::new(),
            done: vec![],
            outcomes: vec![],
            blockers: vec![],
            sources: vec![],
            evidence: vec![],
            borderline: None,
        }
    }

    #[test]
    fn sessions_must_add_up_exactly_and_repair_moves_every_second_once() {
        let active = vec![iv("10:00:00", "10:10:00"), iv("11:00:00", "11:05:00")];
        let exact = vec![
            session(
                "A",
                vec![iv("10:00:00", "10:04:00"), iv("11:00:00", "11:05:00")],
            ),
            session("B", vec![iv("10:04:00", "10:10:00")]),
        ];
        let check = add_up(&exact, &active);
        assert!(check.passed(), "{:?}", check.errors);
        assert_eq!(check.covered_s, 900);

        // Overlap at 10:03-10:05, inactive 10:10-10:12, uncovered 11:00-11:05.
        let mut wrong = vec![
            session("A", vec![iv("10:00:00", "10:05:00")]),
            session("B", vec![iv("10:03:00", "10:12:00")]),
        ];
        let check = add_up(&wrong, &active);
        assert!(!check.passed());
        assert_eq!(check.problem_s, 120 + 120 + 300, "{:?}", check.errors);
        let text = check.errors.join("\n");
        assert!(text.contains("overlap 10:03:00-10:05:00"), "{text}");
        assert!(text.contains("inactive time 10:10:00-10:12:00"), "{text}");
        assert!(
            text.contains("active time in no session: 11:00:00-11:05:00"),
            "{text}"
        );

        let repairs = repair_add_up(&mut wrong, &active);
        assert!(add_up(&wrong, &active).passed());
        assert_eq!(wrong[0].ranges, vec![iv("10:00:00", "10:05:00")]);
        // B lost the overlap and the inactive minutes, and took the uncovered
        // stretch as the nearest session.
        assert_eq!(
            wrong[1].ranges,
            vec![iv("10:05:00", "10:10:00"), iv("11:00:00", "11:05:00")]
        );
        assert_eq!(wrong.iter().map(|s| s.active_s).sum::<i64>(), 900);
        assert!(repairs.iter().all(|r| r.reason.starts_with("repaired: ")));
        assert_eq!(repairs.len(), 3, "{repairs:?}");

        // Nothing assigned at all still adds up, in one marked session.
        let mut none = vec![];
        repair_add_up(&mut none, &active);
        assert_eq!(none.len(), 1);
        assert!(add_up(&none, &active).passed());
    }
}
