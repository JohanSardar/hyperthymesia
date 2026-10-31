// Frozen pre-optimization scan-based compiler: compatibility oracle, not production code.
use crate::*;
use chrono::Duration;
use hx_schema::{Operation, kinds::CorePayload as Payload};
use std::collections::{BTreeMap, BTreeSet};

// A heartbeat proves liveness for at most one minute, never indefinite activity.
const GAP_SECONDS: i64 = 60;

fn at(o: &Observation) -> DateTime<Utc> {
    o.envelope
        .time
        .source_start
        .unwrap_or(o.envelope.time.observed_at)
}
fn key(o: &Observation) -> String {
    o.envelope
        .device
        .as_ref()
        .map(|d| format!("device:{d}"))
        .unwrap_or_else(|| format!("source:{}", o.envelope.connector_instance))
}
fn rank(
    o: &Observation,
) -> (
    Option<u64>,
    DateTime<Utc>,
    &str,
    bool,
    std::cmp::Reverse<i64>,
) {
    (
        o.envelope.sequence,
        o.envelope.time.observed_at,
        o.envelope.revision.as_str(),
        o.envelope.operation == Operation::Delete,
        std::cmp::Reverse(o.id),
    )
}
fn end(o: &Observation) -> DateTime<Utc> {
    let t = &o.envelope.time;
    if t.duration().is_none()
        && let Some(Ok(Payload::ShellCommand(p))) = o.envelope.core_payload()
        && let Some(ms) = p.duration_ms.and_then(|ms| i64::try_from(ms).ok())
        && let Some(end) = at(o).checked_add_signed(Duration::milliseconds(ms))
    {
        return end;
    }
    t.duration()
        .and_then(|d| i64::try_from(d.nanos).ok())
        .and_then(|n| at(o).checked_add_signed(Duration::nanoseconds(n)))
        .or(t.source_end)
        .unwrap_or_else(|| at(o))
}
fn elapsed(a: &Observation, b: &Observation) -> Duration {
    let mut time = a.envelope.time.clone();
    time.source_end = Some(at(b));
    time.mono_end = b.envelope.time.mono_start.clone();
    time.duration()
        .and_then(|d| i64::try_from(d.nanos).ok())
        .map(Duration::nanoseconds)
        .unwrap_or_else(|| at(b) - at(a))
}
fn clip(start: DateTime<Utc>, end: DateTime<Utc>, window: Interval) -> Option<Interval> {
    Interval { start, end }.intersect(&window)
}
fn overlaps(o: &Observation, window: Interval) -> bool {
    window.contains(at(o))
        || clip(at(o), end(o), window).is_some()
        || o.envelope
            .time
            .source_end
            .is_some_and(|end| clip(at(o), end, window).is_some())
        || aggregate_day(o).is_some_and(|day| day.intersect(&window).is_some())
        || matches!(o.envelope.core_payload(), Some(Ok(Payload::CalendarEvent(c))) if c.planned.intersect(&window).is_some())
}
fn aggregate_day(o: &Observation) -> Option<Interval> {
    let Some(Ok(Payload::UsageAggregate(p))) = o.envelope.core_payload() else {
        return None;
    };
    let local = chrono::NaiveDate::parse_from_str(&p.day, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)?
        .and_utc();
    let start = local.checked_sub_signed(Duration::minutes(i64::from(
        o.envelope.time.utc_offset_min.unwrap_or(0),
    )))?;
    Some(Interval {
        start,
        end: start.checked_add_signed(Duration::days(1))?,
    })
}
fn point(t: &mut Timeline, o: &Observation, label: String) {
    let stamp = if let Some(Ok(Payload::CalendarEvent(p))) = o.envelope.core_payload() {
        p.planned.start
    } else {
        at(o)
    };
    t.points.push(Point {
        at: stamp
            .max(t.window.start)
            .min(t.window.end - Duration::nanoseconds(1)),
        lane_key: key(o),
        kind: o.envelope.kind.to_string(),
        label,
        evidence: o.id,
    });
}

pub fn compile(observations: &[Observation], window: Interval) -> Timeline {
    let mut t = Timeline {
        window,
        compiler_version: COMPILER_VERSION.into(),
        lanes: vec![],
        points: vec![],
        coverage: vec![],
        aggregates: vec![],
    };
    if window.start >= window.end {
        return t;
    }
    let mut unique = BTreeMap::new();
    for o in observations {
        unique
            .entry(o.envelope.dedup_key())
            .and_modify(|old: &mut &Observation| {
                if (o.id, o.received_at) < (old.id, old.received_at) {
                    *old = o;
                }
            })
            .or_insert(o);
    }
    let mut latest = BTreeMap::new();
    for o in unique.values().copied() {
        let identity = (&o.envelope.connector_instance, &o.envelope.source_key);
        latest
            .entry(identity)
            .and_modify(|old: &mut &Observation| {
                if rank(o) > rank(old) {
                    *old = o;
                }
            })
            .or_insert(o);
    }
    let active: Vec<_> = latest
        .values()
        .copied()
        .filter(|o| o.envelope.operation != Operation::Delete)
        .collect();
    coverage(&active, &mut t);
    // Connector identity stays in the internal grouping even for device lanes.
    let mut groups: BTreeMap<(LaneId, String), Vec<&Observation>> = BTreeMap::new();
    for o in &active {
        let parsed = o.envelope.core_payload().and_then(Result::ok);
        let (kind, label) = match &parsed {
            Some(Payload::FocusSpan(p)) => (
                "focus",
                p.window_title
                    .as_ref()
                    .map(|title| format!("{} — {title}", p.app_name))
                    .unwrap_or(p.app_name.clone()),
            ),
            Some(Payload::PresenceSpan(p)) => (
                "presence",
                serde_json::to_value(p.state)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
            ),
            Some(Payload::WebVisit(p)) => ("browser", p.title.clone().unwrap_or(p.url.clone())),
            Some(Payload::AgentMessage(p)) => (
                "agent",
                p.thread_title.clone().unwrap_or(p.thread_id.clone()),
            ),
            Some(Payload::ShellCommand(p)) => ("shell", p.command.clone()),
            Some(Payload::CalendarEvent(p)) => ("calendar", p.title.clone()),
            Some(Payload::MessageEvent(p)) => ("messages", p.conversation.clone()),
            Some(Payload::UsageAggregate(p)) => {
                if overlaps(o, window) {
                    t.aggregates.push(AggregateCard {
                        day: p.day.clone(),
                        label: format!("{} — {}", p.device_label, p.app),
                        seconds_estimated: p.seconds_estimated,
                        evidence: vec![o.id],
                    });
                }
                continue;
            }
            _ => ("", o.envelope.kind.to_string()),
        };
        if overlaps(o, window)
            && kind != "focus"
            && kind != "presence"
            && kind != "browser"
            && kind != "shell"
        {
            point(&mut t, o, label);
        }
        if !kind.is_empty() {
            let lane_key = if let Some(Payload::AgentMessage(p)) = &parsed {
                format!(
                    "source:{}:provider:{}",
                    o.envelope.connector_instance, p.provider
                )
            } else {
                key(o)
            };
            groups
                .entry((
                    LaneId {
                        key: lane_key,
                        kind: kind.into(),
                    },
                    o.envelope.connector_instance.to_string(),
                ))
                .or_default()
                .push(o);
        }
    }
    let mut multiplicity = BTreeMap::new();
    for (id, _) in groups.keys() {
        *multiplicity.entry(id.clone()).or_insert(0usize) += 1;
    }
    for ((mut id, collector), mut events) in groups {
        events.sort_by_key(|o| (at(o), o.envelope.sequence, o.id));
        let mut spans: Vec<LaneInterval> = Vec::new();
        for (i, o) in events.iter().enumerate() {
            let parsed = o.envelope.core_payload().and_then(Result::ok).unwrap();
            let mut start = at(o);
            let mut finish = end(o);
            let mut uncertain = o.envelope.time.duration().is_none();
            let mut attrs = o.envelope.payload.clone();
            let label = match &parsed {
                Payload::FocusSpan(p) => p
                    .window_title
                    .as_ref()
                    .map(|s| format!("{} — {s}", p.app_name))
                    .unwrap_or(p.app_name.clone()),
                Payload::PresenceSpan(p) => serde_json::to_value(p.state)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
                Payload::WebVisit(p) => p.title.clone().unwrap_or(p.url.clone()),
                Payload::AgentMessage(p) => p.thread_title.clone().unwrap_or(p.thread_id.clone()),
                Payload::ShellCommand(p) => p.command.clone(),
                Payload::CalendarEvent(p) => {
                    start = p.planned.start;
                    finish = p.planned.end;
                    uncertain = false;
                    attrs["planned"] = true.into();
                    p.title.clone()
                }
                Payload::MessageEvent(p) => p.conversation.clone(),
                _ => unreachable!(),
            };
            if let Payload::ShellCommand(p) = &parsed
                && o.envelope.time.duration().is_none()
                && let Some(ms) = p.duration_ms.and_then(|ms| i64::try_from(ms).ok())
                && let Some(e) = start.checked_add_signed(Duration::milliseconds(ms))
            {
                finish = e;
                uncertain = false;
            }
            if matches!(id.kind.as_str(), "focus" | "presence" | "browser") {
                if uncertain {
                    let horizon = active
                        .iter()
                        .filter(|a| {
                            a.envelope.connector_instance == o.envelope.connector_instance
                                && a.envelope.device == o.envelope.device
                                && collector_name(a) == collector_name(o)
                        })
                        .map(|a| end(a).max(at(a)))
                        .max()
                        .unwrap_or(start);
                    finish = events.get(i + 1).map(|n| at(n)).unwrap_or(horizon);
                    if let Some(next) = events.get(i + 1) {
                        let mut clock = o.envelope.time.clone();
                        clock.mono_end = next.envelope.time.mono_start.clone();
                        if let Some(d) = clock
                            .duration()
                            .filter(|d| d.basis == hx_schema::DurationBasis::Monotonic)
                            && let Ok(n) = i64::try_from(d.nanos)
                            && let Some(e) = start.checked_add_signed(Duration::nanoseconds(n))
                        {
                            finish = e.min(finish);
                        }
                    }
                }
                if let Some(next) = events.get(i + 1) {
                    finish = finish.min(at(next));
                }
                if id.kind == "focus" || id.kind == "browser" {
                    for sleep in &active {
                        if key(sleep) == id.key
                            && at(sleep) >= start
                            && at(sleep) < finish
                            && matches!(sleep.envelope.core_payload(), Some(Ok(Payload::PresenceSpan(p))) if matches!(p.state, hx_schema::kinds::DeviceState::DeviceAsleep | hx_schema::kinds::DeviceState::Locked | hx_schema::kinds::DeviceState::DisplayOff))
                        {
                            finish = at(sleep);
                            uncertain = true;
                        }
                    }
                }
            }
            if let Some(d) = o.envelope.time.duration() {
                attrs["duration"] = serde_json::to_value(d).unwrap();
            }
            if id.kind == "agent" {
                continue;
            }
            if let Some(interval) = clip(start, finish, window) {
                let mut pieces = vec![interval];
                let open =
                    o.envelope.time.source_end.is_none() && o.envelope.time.mono_end.is_none();
                if matches!(id.kind.as_str(), "focus" | "presence" | "browser") {
                    for gap in &t.coverage {
                        if gap.lane_key == key(o)
                            && gap.state != CoverageState::Observed
                            && gap
                                .reason
                                .as_deref()
                                .is_some_and(|r| r.starts_with(&coverage_prefix(o)))
                        {
                            let mut remaining = Vec::new();
                            for piece in pieces {
                                if let Some(overlap) = piece.intersect(&gap.interval) {
                                    uncertain = true;
                                    if piece.start < overlap.start {
                                        remaining.push(Interval {
                                            start: piece.start,
                                            end: overlap.start,
                                        });
                                    }
                                    if !open && overlap.end < piece.end {
                                        remaining.push(Interval {
                                            start: overlap.end,
                                            end: piece.end,
                                        });
                                    }
                                } else {
                                    remaining.push(piece);
                                }
                            }
                            pieces = remaining;
                        }
                    }
                }
                for interval in pieces {
                    spans.push(LaneInterval {
                        interval,
                        label: label.clone(),
                        attrs: attrs.clone(),
                        evidence: vec![o.id],
                        uncertain_edges: uncertain,
                    });
                }
            }
        }
        if id.kind == "agent" {
            let mut threads: BTreeMap<&str, Vec<&Observation>> = BTreeMap::new();
            for o in &events {
                threads
                    .entry(o.envelope.payload["thread_id"].as_str().unwrap())
                    .or_default()
                    .push(o);
            }
            for thread in threads.values() {
                let mut session: Option<LaneInterval> = None;
                let mut first = thread[0];
                let mut previous = thread[0];
                for o in thread {
                    let finish = end(o);
                    if let Some(last) = session.as_mut()
                        && elapsed(previous, o) >= Duration::zero()
                        && elapsed(previous, o) <= Duration::seconds(GAP_SECONDS)
                    {
                        let mapped_end = at(first)
                            .checked_add_signed(elapsed(first, o))
                            .and_then(|start| start.checked_add_signed(finish - at(o)))
                            .unwrap_or(finish);
                        last.interval.end = mapped_end.max(last.interval.end);
                        previous = o;
                        last.evidence.push(o.id);
                        last.uncertain_edges = true;
                        continue;
                    }
                    if let Some(last) = session.take() {
                        spans.push(last);
                    }
                    first = o;
                    previous = o;
                    session = Some(LaneInterval {
                        interval: Interval {
                            start: at(o),
                            end: finish,
                        },
                        label: o.envelope.payload["thread_title"]
                            .as_str()
                            .unwrap_or(o.envelope.payload["thread_id"].as_str().unwrap())
                            .into(),
                        attrs: o.envelope.payload.clone(),
                        evidence: vec![o.id],
                        uncertain_edges: o.envelope.time.source_end.is_none(),
                    });
                }
                if let Some(last) = session {
                    spans.push(last);
                }
            }
            spans = spans
                .into_iter()
                .filter_map(|mut s| {
                    s.interval = s.interval.intersect(&window)?;
                    Some(s)
                })
                .collect();
        }
        if !spans.is_empty() {
            if multiplicity[&id] > 1 {
                id.key = format!("{}:source:{collector}", id.key);
            }
            t.lanes.push(Lane {
                id,
                intervals: spans,
            });
        }
    }
    let referenced: BTreeSet<_> = t
        .lanes
        .iter()
        .flat_map(|l| l.intervals.iter().flat_map(|s| s.evidence.iter().copied()))
        .chain(t.points.iter().map(|p| p.evidence))
        .chain(t.aggregates.iter().flat_map(|a| a.evidence.iter().copied()))
        .collect();
    for o in unique.values() {
        if overlaps(o, window) && !referenced.contains(&o.id) {
            if o.envelope.kind.as_str() == "usage.aggregate" {
                let winner = latest[&(&o.envelope.connector_instance, &o.envelope.source_key)];
                if let Some(card) = t
                    .aggregates
                    .iter_mut()
                    .find(|a| a.evidence.contains(&winner.id))
                {
                    card.evidence.push(o.id);
                } else if let Some(Ok(Payload::UsageAggregate(p))) = o.envelope.core_payload() {
                    t.aggregates.push(AggregateCard {
                        day: p.day,
                        label: "superseded or deleted aggregate (not current usage)".into(),
                        seconds_estimated: 0,
                        evidence: vec![o.id],
                    });
                }
                continue;
            }
            point(
                &mut t,
                o,
                if o.envelope.operation == Operation::Delete {
                    "deleted source object".into()
                } else {
                    o.envelope.kind.to_string()
                },
            );
        }
    }
    for lane in &mut t.lanes {
        lane.intervals
            .sort_by_key(|s| (s.interval, s.evidence.clone()));
    }
    t.lanes.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.intervals[0].interval.cmp(&b.intervals[0].interval))
    });
    t.points
        .sort_by_key(|p| (p.at, p.lane_key.clone(), p.evidence));
    t.aggregates
        .sort_by_key(|a| (a.day.clone(), a.label.clone(), a.evidence.clone()));
    t
}

fn collector_name(o: &Observation) -> String {
    if let Some(Ok(Payload::CollectorHealth(p))) = o.envelope.core_payload() {
        p.collector.trim_end_matches(".span").into()
    } else {
        o.envelope
            .kind
            .as_str()
            .split('.')
            .next()
            .unwrap_or("unknown")
            .into()
    }
}
fn coverage_prefix(o: &Observation) -> String {
    format!("{}[{}]:", o.envelope.connector_instance, collector_name(o))
}
fn horizon(o: &Observation) -> DateTime<Utc> {
    end(o)
        .max(at(o))
        .checked_add_signed(Duration::seconds(GAP_SECONDS))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}
fn coverage(active: &[&Observation], t: &mut Timeline) {
    let mut collectors: BTreeMap<(String, String), Vec<&Observation>> = BTreeMap::new();
    for o in active {
        if o.envelope.device.is_some() && o.envelope.kind.as_str() != "usage.aggregate" {
            collectors
                .entry((key(o), coverage_prefix(o)))
                .or_default()
                .push(o);
        }
    }
    for ((lane_key, collector), mut events) in collectors {
        // Policy metadata applies to every collector in the same installation.
        let installation = events[0].envelope.connector_instance.clone();
        events.extend(active.iter().copied().filter(|o| {
            key(o) == lane_key
                && o.envelope.connector_instance == installation
                && o.envelope.kind.as_str() == "policy.gap"
        }));
        events.sort_by_key(|o| (at(o), o.id));
        let mut boundaries = BTreeSet::from([t.window.start, t.window.end]);
        for o in &events {
            boundaries.insert(at(o).max(t.window.start).min(t.window.end));
            boundaries.insert(end(o).max(t.window.start).min(t.window.end));
            boundaries.insert(horizon(o).max(t.window.start).min(t.window.end));
        }
        let bounds: Vec<_> = boundaries.into_iter().collect();
        let mut cursor = 0;
        let mut alive_until = DateTime::<Utc>::MIN_UTC;
        let mut policy_until = DateTime::<Utc>::MIN_UTC;
        let mut status = hx_schema::kinds::CollectorStatus::Running;
        for pair in bounds.windows(2) {
            let start = pair[0];
            while let Some(o) = events.get(cursor).filter(|o| at(o) <= start) {
                if o.envelope.kind.as_str() == "policy.gap" {
                    policy_until = policy_until.max(end(o));
                } else {
                    alive_until = alive_until.max(horizon(o));
                }
                if let Some(Ok(Payload::CollectorHealth(p))) = o.envelope.core_payload() {
                    status = p.status;
                }
                cursor += 1;
            }
            let paused = status == hx_schema::kinds::CollectorStatus::Paused;
            let failed = status != hx_schema::kinds::CollectorStatus::Running && !paused;
            let policy = policy_until > start;
            let alive = alive_until > start;
            let state = if paused || policy {
                CoverageState::Suppressed
            } else if failed || !alive {
                CoverageState::Unknown
            } else {
                CoverageState::Observed
            };
            let reason = Some(format!(
                "{collector}{}",
                match state {
                    CoverageState::Observed => "evidence",
                    CoverageState::Unknown => "collector gap",
                    CoverageState::Suppressed => "paused or policy gap",
                }
            ));
            if let Some(last) = t.coverage.last_mut()
                && last.lane_key == lane_key
                && last.reason == reason
                && last.state == state
                && last.interval.end == start
            {
                last.interval.end = pair[1];
            } else {
                t.coverage.push(CoverageSpan {
                    lane_key: lane_key.clone(),
                    interval: Interval {
                        start,
                        end: pair[1],
                    },
                    state,
                    reason,
                });
            }
        }
    }
}
