use chrono::{Duration, TimeZone, Utc};
use hx_compile::{
    AttentionConfig, LaneInterval, Observation, compile, compile_with_attention_config,
};
use hx_schema::{
    ConnectorInstance, DeviceId, Envelope, Interval, Kind, ObservationTime, Operation, Revision,
    SourceKey,
};
use serde_json::json;

fn time(s: i64) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + Duration::seconds(s)
}
fn window() -> Interval {
    Interval {
        start: time(0),
        end: time(270),
    }
}
fn event(id: i64, second: i64, kind: &str, payload: serde_json::Value) -> Observation {
    let mut e = Envelope::core(
        ConnectorInstance::new("synthetic:agent"),
        SourceKey::new(format!("event:{id}")),
        Revision::new("0"),
        Operation::Observe,
        &hx_schema::kinds::CorePayload::InputBucket(Default::default()),
        ObservationTime::point(time(second)),
    );
    e.device = Some(DeviceId::new("synthetic-device"));
    e.kind = Kind::new(kind);
    e.payload = payload;
    Observation {
        id,
        received_at: time(second),
        envelope: e,
    }
}
fn input(id: i64, second: i64, keys: u32) -> Observation {
    event(
        id,
        second,
        "input.bucket",
        json!({"bucket_ms":1000,"keys":keys,"clicks":0,"mouse_px":0,"scroll_events":0}),
    )
}
fn health(id: i64, second: i64, collector: &str, status: &str) -> Observation {
    event(
        id,
        second,
        "collector.health",
        json!({"collector":collector,"status":status,"dropped":0}),
    )
}
fn fixture(text: bool) -> Vec<Observation> {
    let mut events = vec![
        event(
            1,
            0,
            "focus.span",
            json!({"app_bundle_id":"synthetic.reader","app_name":"Reader","window_title":"Stuck page"}),
        ),
        event(
            2,
            0,
            "media.still",
            json!({"display":"1","trigger":"focus_change","width":100,"height":100}),
        ),
        input(3, 0, 1),
        input(4, 240, 1),
    ];
    events[1].envelope.connector_instance = ConnectorInstance::new("synthetic:media");
    events[1].envelope.blobs.push(hx_schema::BlobRef {
        sha256: "a".repeat(64),
        media_type: "image/jpeg".into(),
        bytes: 1,
    });
    if text {
        events.push(event(5, 0, "text.snapshot", json!({"surface":"synthetic.reader","source":"accessibility","text":"Synthetic page. ".repeat(30),"text_sha256":"b".repeat(64),"completeness":"complete","visible":true})));
    }
    // Zero buckets must not reset engagement, even when present every second.
    for s in 1..240 {
        events.push(input(1000 + s, s, 0));
    }
    for s in (0..270).step_by(30) {
        for (i, collector) in ["input", "focus", "media"].into_iter().enumerate() {
            let mut h = health(2000 + s * 3 + i as i64, s, collector, "running");
            if collector == "media" {
                h.envelope.connector_instance = ConnectorInstance::new("synthetic:media");
            }
            events.push(h);
        }
    }
    events
}
fn attention(events: &[Observation]) -> Vec<LaneInterval> {
    compile(events, window())
        .lanes
        .into_iter()
        .find(|l| l.id.kind == "attention")
        .unwrap()
        .intervals
}
fn state_at(spans: &[LaneInterval], second: i64) -> &str {
    &spans
        .iter()
        .find(|s| s.interval.contains(time(second)))
        .unwrap()
        .label
}
#[test]
fn stuck_page_is_engaged_then_reading_then_away_until_input() {
    let events = fixture(true);
    let spans = attention(&events);
    for (s, label) in [
        (0, "engaged"),
        (29, "engaged"),
        (30, "reading"),
        (119, "reading"),
        (120, "probably_away"),
        (239, "probably_away"),
        (240, "engaged"),
    ] {
        assert_eq!(state_at(&spans, s), label, "at {s}");
    }
    assert_eq!(spans.first().unwrap().interval.start, time(0));
    assert_eq!(spans.last().unwrap().interval.end, time(270));
    for pair in spans.windows(2) {
        assert_eq!(pair[0].interval.end, pair[1].interval.start);
    }
    for s in &spans {
        assert_eq!(s.attrs["basis"], "inferred");
        assert!(!s.evidence.is_empty());
        assert!(
            s.evidence
                .iter()
                .all(|id| events.iter().any(|e| e.id == *id))
        );
    }
    assert_eq!(state_at(&attention(&fixture(false)), 30), "probably_away");
    let mut replay = events.clone();
    replay.reverse();
    replay.extend(events.clone());
    assert_eq!(compile(&events, window()), compile(&replay, window()));
    let custom = compile_with_attention_config(
        &events,
        window(),
        AttentionConfig {
            input_gap_s: 10,
            reading_max_s: 40,
            ..Default::default()
        },
    );
    let custom = &custom
        .lanes
        .iter()
        .find(|l| l.id.kind == "attention")
        .unwrap()
        .intervals;
    assert_eq!(state_at(custom, 10), "reading");
    assert_eq!(state_at(custom, 40), "probably_away");
}
#[test]
fn unhealthy_missing_and_policy_capture_are_unknown() {
    for status in ["failed", "paused", "permission_denied", "stopped"] {
        let mut events = fixture(false);
        events.retain(|o| {
            !(o.envelope.payload["collector"] == "input" && o.envelope.time.observed_at >= time(90))
        });
        events.push(health(9000, 90, "input", status));
        let spans = attention(&events);
        assert_eq!(state_at(&spans, 90), "unknown");
        assert_eq!(state_at(&spans, 250), "unknown");
    }
    let mut events = fixture(false);
    events.retain(|o| {
        !(o.envelope.payload["collector"] == "input" && o.envelope.time.observed_at > time(0))
    });
    assert_eq!(state_at(&attention(&events), 60), "unknown");
    events = fixture(false);
    let mut gap = event(
        9000,
        70,
        "policy.gap",
        json!({"reason":"capture_interrupted"}),
    );
    gap.envelope.time.source_end = Some(time(210));
    events.push(gap);
    let spans = attention(&events);
    assert_eq!(state_at(&spans, 70), "unknown");
    assert_eq!(state_at(&spans, 209), "unknown");
    assert!(
        spans
            .iter()
            .filter(|s| s.interval.contains(time(100)))
            .all(|s| s.evidence.contains(&9000))
    );
    events.retain(|o| o.envelope.kind.as_str() != "collector.health");
    assert!(attention(&events).iter().all(|s| s.label == "unknown"));
}
#[test]
fn audio_and_changing_hashes_support_watching_and_lock_overrides_activity() {
    let mut events = fixture(false);
    events[0].envelope.payload["window_title"] = json!("Video — Audio playing");
    assert_eq!(state_at(&attention(&events), 100), "watching");
    events[0].envelope.payload["window_title"] = json!("Static page");
    let mut frame = events[1].clone();
    frame.id = 9000;
    frame.envelope.source_key = SourceKey::new("changed-frame");
    frame.envelope.time = ObservationTime::point(time(90));
    frame.envelope.blobs[0].sha256 = "c".repeat(64);
    events.push(frame);
    let spans = attention(&events);
    assert_eq!(state_at(&spans, 90), "watching");
    assert_eq!(state_at(&spans, 120), "probably_away");
    assert!(
        spans
            .iter()
            .find(|s| s.interval.contains(time(100)))
            .unwrap()
            .evidence
            .contains(&2)
    );
    events.push(event(9001, 130, "presence.span", json!({"state":"locked"})));
    assert_eq!(state_at(&attention(&events), 130), "locked_or_asleep");
    assert_eq!(state_at(&attention(&events), 250), "locked_or_asleep");
}

#[test]
fn sparse_silence_and_zero_buckets_have_identical_attention() {
    let events = fixture(true);
    let mut sparse = events.clone();
    sparse
        .retain(|o| o.envelope.kind.as_str() != "input.bucket" || o.envelope.payload["keys"] != 0);
    assert_eq!(attention(&events), attention(&sparse));
    sparse.retain(|o| o.envelope.kind.as_str() != "input.bucket");
    sparse[0].envelope.payload["window_title"] = json!("Audio playing");
    assert_eq!(state_at(&attention(&sparse), 0), "watching");
    assert_eq!(state_at(&attention(&sparse), 200), "watching");
}

#[test]
fn unchanged_frames_are_static_and_chunks_need_changed_hashes() {
    let mut events = fixture(false);
    let mut frame = events[1].clone();
    frame.id = 9000;
    frame.envelope.source_key = SourceKey::new("repeated-frame");
    frame.envelope.time = ObservationTime::point(time(90));
    events.push(frame.clone());
    assert_eq!(state_at(&attention(&events), 90), "probably_away");
    frame.id = 9001;
    frame.envelope.source_key = SourceKey::new("chunk-first");
    frame.envelope.kind = Kind::new("media.chunk");
    frame.envelope.payload =
        json!({"display":"1","codec":"h264","width":100,"height":100,"frames":20});
    frame.envelope.time = ObservationTime::point(time(100));
    events.push(frame.clone());
    frame.id = 9002;
    frame.envelope.source_key = SourceKey::new("chunk-changed");
    frame.envelope.time = ObservationTime::point(time(110));
    frame.envelope.blobs[0].sha256 = "d".repeat(64);
    events.push(frame);
    assert_eq!(state_at(&attention(&events), 110), "watching");
    assert_eq!(state_at(&attention(&events), 140), "probably_away");
}

#[test]
fn missing_media_or_lost_input_is_unknown_and_sleep_is_explicit() {
    let mut events = fixture(false);
    events.retain(|o| o.envelope.payload["collector"] != "media");
    assert_eq!(state_at(&attention(&events), 60), "unknown");
    let mut events = fixture(false);
    events.retain(|o| {
        !(o.envelope.payload["collector"] == "input" && o.envelope.time.observed_at >= time(90))
    });
    let mut dropped = health(9000, 90, "input", "running");
    dropped.envelope.payload["dropped"] = json!(1);
    events.push(dropped);
    assert_eq!(state_at(&attention(&events), 90), "unknown");
    for state in ["locked", "device_asleep", "display_off"] {
        let mut events = fixture(false);
        events.push(event(9001, 130, "presence.span", json!({"state":state})));
        assert_eq!(state_at(&attention(&events), 130), "locked_or_asleep");
    }
}

#[test]
fn lock_without_input_health_is_known_but_focus_without_health_is_unknown() {
    let lock = event(1, 0, "presence.span", json!({"state":"locked"}));
    assert!(
        attention(&[lock])
            .iter()
            .all(|s| s.label == "locked_or_asleep")
    );
    let focus = event(
        1,
        0,
        "focus.span",
        json!({"app_bundle_id":"synthetic.app","app_name":"Synthetic"}),
    );
    assert!(attention(&[focus]).iter().all(|s| s.label == "unknown"));
}
