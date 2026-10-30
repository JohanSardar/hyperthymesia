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
