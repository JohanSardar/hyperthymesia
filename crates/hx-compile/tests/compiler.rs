use chrono::{DateTime, Duration, Utc};
use hx_compile::*;
use hx_schema::{Interval, Operation};
use proptest::prelude::*;
use serde::Deserialize;
use std::collections::BTreeSet;

fn fixture(name: &str) -> (Vec<Observation>, Truth) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/traces")
        .join(name);
    (
        serde_json::from_str(&std::fs::read_to_string(dir.join("observations.json")).unwrap())
            .unwrap(),
        serde_json::from_str(&std::fs::read_to_string(dir.join("truth.json")).unwrap()).unwrap(),
    )
}
#[derive(Deserialize)]
struct Expected {
    kind: String,
    label: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}
#[derive(Deserialize)]
struct ExpectedCoverage {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    state: CoverageState,
}
#[derive(Deserialize)]
struct ExpectedAggregate {
    label: String,
    seconds_estimated: u64,
}
#[derive(Deserialize)]
struct Truth {
    window: Interval,
    intervals: Vec<Expected>,
    coverage: Vec<ExpectedCoverage>,
    aggregates: Vec<ExpectedAggregate>,
}

#[test]
fn scripted_ground_truth() {
    for name in [
        "normal-switching",
        "six-second-interleave",
        "laptop-sleep",
        "wall-clock-jump",
        "collector-gap",
        "phone-aggregates",
        "late-upsert",
    ] {
        let (observations, truth) = fixture(name);
        let timeline = compile(&observations, truth.window);
        let actual: Vec<_> = timeline
            .lanes
            .iter()
            .flat_map(|lane| lane.intervals.iter().map(move |s| (&lane.id.kind, s)))
            .collect();
        assert_eq!(actual.len(), truth.intervals.len(), "{name}: {timeline:#?}");
        for expected in truth.intervals {
            assert!(
                actual.iter().any(|(kind, s)| **kind == expected.kind
                    && s.label == expected.label
                    && (s.interval.start - expected.start).abs() <= Duration::seconds(1)
                    && (s.interval.end - expected.end).abs() <= Duration::seconds(1)),
                "{name}: missing {}",
                expected.label
            );
        }
        for expected in truth.coverage {
            assert!(
                timeline.coverage.iter().any(|s| s.state == expected.state
                    && s.interval.start == expected.start
                    && s.interval.end == expected.end),
                "{name}"
            );
        }
        assert_eq!(timeline.aggregates.len(), truth.aggregates.len());
        for expected in truth.aggregates {
            assert!(
                timeline.aggregates.iter().any(|a| a.label == expected.label
                    && a.seconds_estimated == expected.seconds_estimated)
            );
        }
        assert_evidence(&timeline, &observations);
    }
}
fn assert_evidence(t: &Timeline, observations: &[Observation]) {
    let refs: BTreeSet<_> = t
        .points
        .iter()
        .map(|p| p.evidence)
        .chain(
            t.lanes
                .iter()
                .flat_map(|l| l.intervals.iter().flat_map(|s| s.evidence.iter().copied())),
        )
        .chain(t.aggregates.iter().flat_map(|a| a.evidence.iter().copied()))
        .collect();
    for o in observations {
        assert!(refs.contains(&o.id), "missing {}", o.id);
    }
}
#[test]
fn trace_snapshot() {
    let (observations, truth) = fixture("six-second-interleave");
    insta::assert_json_snapshot!(compile(&observations, truth.window));
}
fn generated(switches: &[u16]) -> (Vec<Observation>, Interval) {
    let (base, truth) = fixture("normal-switching");
    let observations = switches
        .iter()
        .enumerate()
        .map(|(i, second)| {
            let mut o = base[0].clone();
            o.id = i as i64 + 1;
            o.envelope.source_key = hx_schema::SourceKey::new(format!("event:{i}"));
            o.envelope.time.source_start =
                Some(truth.window.start + Duration::seconds(i64::from(*second)));
            o.envelope.time.source_end =
                Some(truth.window.start + Duration::seconds(i64::from(*second) + 30));
            o
        })
        .collect();
    (observations, truth.window)
}
proptest! {
    #[test]
    fn replay_order_evidence_and_duration(switches in prop::collection::vec(0u16..570, 1..40), order in prop::collection::vec(any::<u64>(), 40)) {
        let (observations, window) = generated(&switches);
        let expected = compile(&observations, window);
        let mut replay = observations.clone();
        replay.sort_by_key(|o| order[(o.id as usize-1)%order.len()]);
        replay.extend(observations.clone());
        prop_assert_eq!(&expected, &compile(&replay, window));
        assert_evidence(&expected, &observations);
        for lane in &expected.lanes {
            for span in &lane.intervals { prop_assert!(span.interval.start >= window.start && span.interval.end <= window.end); }
            if lane.id.kind == "focus" {
                for pair in lane.intervals.windows(2) { prop_assert!(pair[0].interval.end <= pair[1].interval.start); }
            }
        }
        for (_, duration) in exposure_by_label(&expected, "focus") { prop_assert!(duration <= window.len()); }
    }
    #[test]
    fn replay_upserts(order in prop::collection::vec(any::<u64>(), 3), copies in 1usize..10) {
        let (mut observations, truth) = fixture("late-upsert");
        let mut deletion = observations[0].clone(); deletion.id = 3; deletion.envelope.operation = Operation::Delete;
        deletion.envelope.sequence = Some(3); deletion.envelope.revision = hx_schema::Revision::new("deleted");
        observations.push(deletion);
        let expected = compile(&observations, truth.window);
        prop_assert!(expected.lanes.is_empty());
        let mut replay = (0..copies).flat_map(|_| observations.clone()).collect::<Vec<_>>();
        replay.sort_by_key(|o| order[o.id as usize-1]);
        prop_assert_eq!(expected, compile(&replay, truth.window));
    }
}

fn event(
    id: i64,
    second: i64,
    end: Option<i64>,
    kind: &str,
    payload: serde_json::Value,
) -> Observation {
    let (base, truth) = fixture("normal-switching");
    let mut o = base[0].clone();
    o.id = id;
    o.envelope.source_key = hx_schema::SourceKey::new(format!("event:{id}"));
    o.envelope.kind = hx_schema::Kind::new(kind);
    o.envelope.payload = payload;
    o.envelope.time =
        hx_schema::ObservationTime::point(truth.window.start + Duration::seconds(second));
    o.envelope.time.source_end = end.map(|e| truth.window.start + Duration::seconds(e));
    o
}
fn focus(id: i64, start: i64, end: Option<i64>) -> Observation {
    event(
        id,
        start,
        end,
        "focus.span",
        serde_json::json!({"app_bundle_id":"synthetic.editor","app_name":"Editor"}),
    )
}
fn health(id: i64, start: i64, collector: &str, status: &str) -> Observation {
    event(
        id,
        start,
        None,
        "collector.health",
        serde_json::json!({"collector":collector,"status":status,"dropped":0}),
    )
}
fn window() -> Interval {
    fixture("normal-switching").1.window
}
#[test]
fn open_spans_end_at_their_collectors_last_evidence() {
    let observations = vec![
        focus(1, 0, None),
        health(2, 30, "focus", "running"),
        health(3, 120, "media", "running"),
    ];
    let timeline = compile(&observations, window());
    let lane = timeline
        .lanes
        .iter()
        .find(|l| l.id.kind == "focus")
        .unwrap();
    assert_eq!(lane.intervals[0].interval.len(), Duration::seconds(30));
    assert!(lane.intervals[0].uncertain_edges);
    assert_evidence(&timeline, &observations);
}
#[test]
fn pause_resume_and_policy_gaps_remove_content_exposure() {
    let observations = vec![
        focus(1, 0, Some(180)),
        health(2, 30, "focus", "paused"),
        health(3, 90, "focus", "running"),
        event(
            4,
            110,
            Some(120),
            "policy.gap",
            serde_json::json!({"reason":"synthetic exclusion"}),
        ),
    ];
    let timeline = compile(&observations, window());
    let lane = timeline
        .lanes
        .iter()
        .find(|l| l.id.kind == "focus")
        .unwrap();
    let lengths: Vec<_> = lane
        .intervals
        .iter()
        .map(|s| s.interval.len().num_seconds())
        .collect();
    assert_eq!(lengths, [30, 20, 60]);
    assert!(
        timeline
            .coverage
            .iter()
            .any(|s| s.state == CoverageState::Suppressed)
    );
    assert_evidence(&timeline, &observations);
}
#[test]
fn interleaved_threads_group_independently_with_no_timeout_tail() {
    let agent = |id, start, thread: &str| {
        event(
            id,
            start,
            None,
            "agent.message",
            serde_json::json!({"provider":"synthetic","thread_id":thread,"role":"assistant","text":"synthetic output"}),
        )
    };
    let observations = vec![
        agent(1, 0, "a"),
        agent(2, 10, "b"),
        agent(3, 20, "a"),
        agent(4, 30, "b"),
        agent(5, 500, "a"),
    ];
    let timeline = compile(&observations, window());
    let lane = timeline
        .lanes
        .iter()
        .find(|l| l.id.kind == "agent")
        .unwrap();
    assert_eq!(lane.intervals.len(), 2);
    assert_eq!(lane.intervals[0].interval.len(), Duration::seconds(20));
    assert_eq!(lane.intervals[1].interval.len(), Duration::seconds(20));
    assert_eq!(timeline.points.len(), 5);
    assert_evidence(&timeline, &observations);
}
#[test]
fn shell_duration_and_planned_calendar_overlap_the_window() {
    let planned = Interval {
        start: window().start - Duration::seconds(10),
        end: window().start + Duration::seconds(30),
    };
    let observations = vec![
        event(
            1,
            -10,
            None,
            "shell.command",
            serde_json::json!({"command":"synthetic command","cwd":"/synthetic","duration_ms":30000}),
        ),
        event(
            2,
            -100,
            None,
            "calendar.event",
            serde_json::json!({"title":"Synthetic plan","planned":planned}),
        ),
    ];
    let timeline = compile(&observations, window());
    assert_eq!(timeline.lanes.len(), 2);
    let calendar = timeline
        .lanes
        .iter()
        .find(|l| l.id.kind == "calendar")
        .unwrap();
    assert_eq!(calendar.intervals[0].attrs["planned"], true);
    let shell = timeline
        .lanes
        .iter()
        .find(|l| l.id.kind == "shell")
        .unwrap();
    assert_eq!(shell.intervals[0].interval.len(), Duration::seconds(20));
    assert_evidence(&timeline, &observations);
}
#[test]
fn points_keep_all_core_and_extension_evidence() {
    let observations = vec![
        event(
            1,
            0,
            None,
            "ui.click",
            serde_json::json!({"app_bundle_id":"synthetic","sample_lag_ms":1}),
        ),
        event(
            2,
            1,
            None,
            "text.snapshot",
            serde_json::json!({"surface":"synthetic","source":"editor","text":"synthetic","text_sha256":"synthetic","completeness":"complete"}),
        ),
        event(
            3,
            2,
            None,
            "code.commit",
            serde_json::json!({"repo":"synthetic","sha":"synthetic","message":"synthetic commit"}),
        ),
        event(
            4,
            3,
            None,
            "media.still",
            serde_json::json!({"display":"synthetic","trigger":"periodic","width":1,"height":1}),
        ),
        event(
            5,
            4,
            None,
            "message.event",
            serde_json::json!({"service":"synthetic","conversation":"synthetic","direction":"received"}),
        ),
        event(
            6,
            5,
            None,
            "extension.opaque",
            serde_json::json!({"opaque":true}),
        ),
    ];
    let timeline = compile(&observations, window());
    assert_eq!(timeline.points.len(), observations.len());
    assert_evidence(&timeline, &observations);
}
#[test]
fn union_exposure_does_not_double_count_devices_or_repeated_labels() {
    let a = focus(1, 0, Some(120));
    let mut b = focus(2, 60, Some(180));
    b.envelope.device = Some(hx_schema::DeviceId::new("synthetic-other"));
    let timeline = compile(&[a, b], window());
    assert_eq!(
        exposure_by_label(&timeline, "focus"),
        vec![("Editor".into(), Duration::seconds(180))]
    );
}
#[test]
fn unrelated_boots_use_utc_duration() {
    let (mut observations, truth) = fixture("wall-clock-jump");
    observations[0]
        .envelope
        .time
        .mono_end
        .as_mut()
        .unwrap()
        .boot_id = hx_schema::BootId::new("other-boot");
    let timeline = compile(&observations, truth.window);
    assert_eq!(
        timeline.lanes[0].intervals[0].interval.len(),
        Duration::seconds(360)
    );
    assert_eq!(
        timeline.lanes[0].intervals[0].attrs["duration"]["basis"],
        "wall_clock"
    );
}
#[test]
fn dedup_uses_envelope_identity_and_canonical_evidence_row() {
    let a = focus(1, 0, Some(30));
    let mut replay = a.clone();
    replay.id = 999;
    assert_eq!(
        compile(std::slice::from_ref(&a), window()),
        compile(&[replay, a], window())
    );
}
#[test]
fn aggregates_revisions_remain_cards_and_do_not_inflate_current_totals() {
    let (mut observations, truth) = fixture("phone-aggregates");
    observations[0].envelope.operation = Operation::Upsert;
    let mut revised = observations[0].clone();
    revised.id = 2;
    revised.envelope.revision = hx_schema::Revision::new("1");
    revised.envelope.sequence = Some(2);
    revised.envelope.payload["seconds_estimated"] = 1200.into();
    observations.push(revised);
    let timeline = compile(&observations, truth.window);
    assert!(timeline.points.is_empty());
    assert!(timeline.lanes.is_empty());
    assert_eq!(timeline.aggregates.len(), 1);
    assert_eq!(timeline.aggregates[0].seconds_estimated, 1200);
    assert_evidence(&timeline, &observations);
}

#[test]
fn an_open_focus_does_not_resume_after_an_unobserved_gap() {
    let observations = vec![
        focus(1, 0, None),
        health(2, 10, "focus", "running"),
        health(3, 180, "focus", "running"),
        focus(4, 240, Some(300)),
    ];
    let timeline = compile(&observations, window());
    let lane = timeline
        .lanes
        .iter()
        .find(|l| l.id.kind == "focus")
        .unwrap();
    assert_eq!(lane.intervals.len(), 2);
    assert_eq!(
        lane.intervals[0].interval.end,
        window().start + Duration::seconds(70)
    );
    assert_eq!(
        lane.intervals[1].interval.start,
        window().start + Duration::seconds(240)
    );
}
#[test]
fn aggregate_reported_later_is_selected_by_its_civil_day() {
    let (mut observations, truth) = fixture("phone-aggregates");
    observations[0].envelope.time.source_start = Some(truth.window.end + Duration::days(1));
    let timeline = compile(&observations, truth.window);
    assert_eq!(timeline.aggregates.len(), 1);
    assert!(timeline.points.is_empty());
}
#[test]
fn clipped_clock_jump_evidence_is_not_lost() {
    let (observations, mut truth) = fixture("wall-clock-jump");
    truth.window.start += Duration::seconds(300);
    let timeline = compile(&observations, truth.window);
    assert_evidence(&timeline, &observations);
    assert!(timeline.lanes.is_empty());
    assert!(timeline.points.iter().all(|p| truth.window.contains(p.at)));
}

#[test]
fn browser_transitions_keep_short_switches_and_source_lanes_are_distinct() {
    let observations = vec![
        event(
            1,
            0,
            Some(30),
            "web.visit",
            serde_json::json!({"url":"https://synthetic.invalid/a"}),
        ),
        event(
            2,
            30,
            Some(36),
            "web.visit",
            serde_json::json!({"url":"https://synthetic.invalid/b"}),
        ),
        event(
            3,
            36,
            Some(60),
            "web.visit",
            serde_json::json!({"url":"https://synthetic.invalid/a"}),
        ),
    ];
    let timeline = compile(&observations, window());
    assert_eq!(timeline.lanes[0].id.kind, "browser");
    assert_eq!(timeline.lanes[0].intervals.len(), 3);
    assert_eq!(
        timeline.lanes[0].intervals[1].interval.len(),
        Duration::seconds(6)
    );
    let a = focus(4, 0, Some(60));
    let mut b = focus(5, 0, Some(60));
    b.envelope.connector_instance = hx_schema::ConnectorInstance::new("synthetic:second-source");
    let timeline = compile(&[a, b], window());
    assert_eq!(timeline.lanes.len(), 2);
    assert_ne!(timeline.lanes[0].id, timeline.lanes[1].id);
}
#[test]
fn open_transitions_use_comparable_monotonic_stamps() {
    let (mut observations, _) = fixture("wall-clock-jump");
    let mut a = observations.remove(0);
    a.envelope.time.source_end = None;
    a.envelope.time.mono_end = None;
    let mut b = focus(2, 360, Some(390));
    b.envelope.time.mono_start = Some(hx_schema::MonoStamp {
        domain: hx_schema::ClockDomain::MachContinuous,
        boot_id: hx_schema::BootId::new("synthetic-boot"),
        nanos: 60_000_000_000,
    });
    let timeline = compile(&[a, b], window());
    assert_eq!(
        timeline.lanes[0].intervals[0].interval.len(),
        Duration::seconds(60)
    );
    assert!(timeline.lanes[0].intervals[0].uncertain_edges);
}

#[test]
fn agent_activity_bounds_prefer_monotonic_elapsed_time() {
    let message = |id, start| {
        event(
            id,
            start,
            None,
            "agent.message",
            serde_json::json!({"provider":"synthetic","thread_id":"synthetic-thread","role":"assistant","text":"synthetic"}),
        )
    };
    let mut a = message(1, 0);
    let mut b = message(2, 360);
    let stamp = |nanos| hx_schema::MonoStamp {
        domain: hx_schema::ClockDomain::MachContinuous,
        boot_id: hx_schema::BootId::new("synthetic-boot"),
        nanos,
    };
    a.envelope.time.mono_start = Some(stamp(0));
    b.envelope.time.mono_start = Some(stamp(30_000_000_000));
    let timeline = compile(&[a, b], window());
    assert_eq!(
        timeline.lanes[0].intervals[0].interval.len(),
        Duration::seconds(30)
    );
}
