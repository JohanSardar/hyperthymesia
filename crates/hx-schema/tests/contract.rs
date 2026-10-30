use chrono::{DateTime, Duration, TimeZone, Utc};
use hx_schema::kinds::{CorePayload, FocusSpan, InputBucket};
use hx_schema::*;
use proptest::prelude::*;

fn t(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_790_000_000 + secs, 0).unwrap()
}

fn focus(app: &str) -> CorePayload {
    CorePayload::FocusSpan(FocusSpan {
        app_bundle_id: app.into(),
        app_name: app.into(),
        window_title: Some("w".into()),
        pid: Some(1),
        display: None,
    })
}

fn env_with(time: ObservationTime) -> Envelope {
    Envelope::core(
        ConnectorInstance::new("test:dev"),
        SourceKey::new("k1"),
        Revision::append_only(),
        Operation::Observe,
        &focus("com.apple.Safari"),
        time,
    )
}

#[test]
fn roundtrip_keeps_dedup_key_and_payload() {
    let e = env_with(ObservationTime::point(t(0)));
    e.validate().unwrap();
    let back: Envelope = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back, e);
    assert_eq!(back.dedup_key(), e.dedup_key());
    assert_eq!(
        back.core_payload().unwrap().unwrap(),
        focus("com.apple.Safari")
    );
}

#[test]
fn unknown_sensitivity_defaults_to_restricted() {
    let mut v = serde_json::to_value(env_with(ObservationTime::point(t(0)))).unwrap();
    v.as_object_mut().unwrap().remove("sensitivity");
    let e: Envelope = serde_json::from_value(v).unwrap();
    assert_eq!(e.sensitivity, Sensitivity::Restricted);
}

#[test]
fn boundary_rejects_bad_envelopes() {
    let mut e = env_with(ObservationTime::point(t(0)));
    e.kind = Kind::new("Focus");
    assert!(matches!(e.validate(), Err(EnvelopeError::Kind(_))));

    let mut e = env_with(ObservationTime::point(t(0)));
    e.envelope_version = 2;
    assert_eq!(e.validate(), Err(EnvelopeError::Version(2)));

    let mut time = ObservationTime::point(t(10));
    time.source_end = Some(t(5));
    assert_eq!(
        env_with(time).validate(),
        Err(EnvelopeError::NegativeInterval)
    );

    let mut e = env_with(ObservationTime::point(t(0)));
    e.blobs.push(BlobRef {
        sha256: "XYZ".into(),
        media_type: "image/heic".into(),
        bytes: 1,
    });
    assert!(matches!(e.validate(), Err(EnvelopeError::BlobHash(_))));
}

#[test]
fn non_core_kinds_stay_opaque() {
    let mut e = env_with(ObservationTime::point(t(0)));
    e.kind = Kind::new("spotify.track_played");
    e.validate().unwrap();
    assert!(e.core_payload().is_none());
}

#[test]
fn input_bucket_payload_has_no_key_identity_fields() {
    let v = CorePayload::InputBucket(InputBucket {
        bucket_ms: 1000,
        keys: 3,
        ..Default::default()
    })
    .payload_value();
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["bucket_ms", "clicks", "keys", "mouse_px", "scroll_events"]
    );
}

fn mono(boot: &str, nanos: u64) -> MonoStamp {
    MonoStamp {
        domain: ClockDomain::MachContinuous,
        boot_id: BootId::new(boot),
        nanos,
    }
}

#[test]
fn duration_prefers_monotonic_even_when_wall_clock_steps() {
    let mut time = ObservationTime::point(t(0));
    time.source_end = Some(t(3600)); // wall clock jumped an hour
    time.mono_start = Some(mono("b1", 1_000));
    time.mono_end = Some(mono("b1", 1_000 + 5_000_000_000));
    let d = time.duration().unwrap();
    assert_eq!(d.basis, DurationBasis::Monotonic);
    assert_eq!(d.nanos, 5_000_000_000);
}

#[test]
fn duration_never_subtracts_clocks_from_different_boots() {
    let mut time = ObservationTime::point(t(0));
    time.source_end = Some(t(7));
    time.resolution_ms = 1000;
    time.mono_start = Some(mono("b1", 9_000_000_000));
    time.mono_end = Some(mono("b2", 10));
    let d = time.duration().unwrap();
    assert_eq!(d.basis, DurationBasis::WallClock);
    assert_eq!(d.nanos, 7_000_000_000);
    assert_eq!(d.resolution_ms, 1000);
}

#[test]
fn civil_day_split_handles_offsets() {
    // 22:00 to 02:00 local at UTC-7
    let iv = Interval::new(t(0), t(4 * 3600)).unwrap();
    let off = -7 * 60;
    let local_start = (t(0) + Duration::minutes(off.into())).time();
    let parts = iv.split_civil_days(off);
    let total: Duration = parts.iter().map(|p| p.len()).sum();
    assert_eq!(total, iv.len());
    assert!(parts.windows(2).all(|w| w[0].end == w[1].start));
    let crosses = local_start.signed_duration_since(chrono::NaiveTime::MIN) + Duration::hours(4)
        > Duration::hours(24);
    assert_eq!(parts.len(), if crosses { 2 } else { 1 });
}

proptest! {
    #[test]
    fn union_len_is_bounded_and_order_independent(
        raw in proptest::collection::vec((0i64..10_000, 0i64..600), 0..40),
        seed in any::<u64>(),
    ) {
        let ivs: Vec<Interval> = raw.iter().map(|(s, l)| Interval::new(t(*s), t(s + l)).unwrap()).collect();
        let u = union_len(&ivs);
        let sum: Duration = ivs.iter().map(|i| i.len()).sum();
        prop_assert!(u <= sum);
        prop_assert!(u >= ivs.iter().map(|i| i.len()).max().unwrap_or_default());
        let mut shuffled = ivs.clone();
        let n = shuffled.len().max(1);
        shuffled.rotate_left((seed as usize) % n);
        shuffled.reverse();
        prop_assert_eq!(union_len(&shuffled), u);
        let doubled: Vec<Interval> = ivs.iter().chain(ivs.iter()).copied().collect();
        prop_assert_eq!(union_len(&doubled), u);
    }

    #[test]
    fn civil_split_partitions_the_interval(start in 0i64..200_000, len in 0i64..400_000, off in -720i16..840) {
        let iv = Interval::new(t(start), t(start + len)).unwrap();
        let parts = iv.split_civil_days(off);
        let total: Duration = parts.iter().map(|p| p.len()).sum();
        prop_assert_eq!(total, iv.len());
        prop_assert!(parts.iter().all(|p| p.len() <= Duration::hours(24)));
        prop_assert!(parts.windows(2).all(|w| w[0].end == w[1].start));
    }

    #[test]
    fn content_digest_revision_is_stable(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        prop_assert_eq!(Revision::digest(&bytes), Revision::digest(&bytes));
    }
}
