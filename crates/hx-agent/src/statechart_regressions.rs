use crate::{
    config::Config,
    envelope::Producer,
    gap::PolicyGap,
    outbox::Outbox,
    policy::{Policy, persist},
};
use hx_macos_sys::Focus;
use hx_schema::{kinds::*, *};
fn producer() -> Producer {
    Producer::new("synthetic".into()).unwrap()
}
fn span(p: &mut Producer, presence: bool) -> Envelope {
    p.core(
        if presence {
            "presence:synthetic"
        } else {
            "focus:synthetic"
        },
        "0",
        Operation::Upsert,
        if presence {
            CorePayload::PresenceSpan(PresenceSpan {
                state: DeviceState::Active,
                idle_threshold_s: Some(300),
            })
        } else {
            CorePayload::FocusSpan(FocusSpan {
                app_bundle_id: "synthetic.app".into(),
                app_name: "Synthetic".into(),
                window_title: None,
                pid: None,
                display: None,
            })
        },
    )
}
#[test]
fn private_browser_content_is_blocked_with_content_free_gap() {
    let policy = Policy::new(&Config::default()).unwrap();
    let focus = Focus {
        bundle_id: "com.apple.Safari".into(),
        name: "Synthetic".into(),
        pid: 1,
        title: Some("Synthetic secret".into()),
        electron: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let mut out = Outbox::open(&dir.path().join("db"), 100_000, 100).unwrap();
    let mut p = producer();
    let mut text = p.calibration();
    text.kind = Kind::new("text.snapshot");
    text.payload = serde_json::json!({"text":"synthetic secret"});
    assert!(!persist(&mut out, &policy, Some(&focus), false, &text, &[]).unwrap());
    let gap = PolicyGap::default()
        .observe(policy.excludes_native(&focus), p.time(), &mut p)
        .expect("unverifiable private state needs a gap");
    assert!(persist(&mut out, &policy, Some(&focus), false, &gap, &[]).unwrap());
    assert!(
        !serde_json::to_string(&gap.payload)
            .unwrap()
            .contains("secret")
    );
    assert_eq!(out.stats().unwrap().0, 1);
}
#[test]
fn browser_companion_policy_gap_passes_content_gate() {
    let policy = Policy::new(&Config::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut out = Outbox::open(&dir.path().join("db"), 100_000, 100).unwrap();
    let mut p = producer();
    let mut tracker = PolicyGap::default();
    for excluded in [true, true, false] {
        let mut gap = tracker.observe(excluded, p.time(), &mut p).unwrap();
        gap.connector_instance = ConnectorInstance::new("companion:browser:synthetic");
        assert!(persist(&mut out, &policy, None, true, &gap, &[]).unwrap());
    }
    // An arbitrary companion cannot bypass the gate by labeling content as a gap.
    let mut forged = p.calibration();
    forged.kind = Kind::new("policy.gap");
    forged.connector_instance = ConnectorInstance::new("companion:browser:synthetic");
    forged.payload = serde_json::json!({"reason":"policy_exclusion","open":true,"text":"secret"});
    assert!(!persist(&mut out, &policy, None, true, &forged, &[]).unwrap());
    assert_eq!(out.stats().unwrap().0, 3);
}
#[test]
fn crash_restart_reconciles_prior_open_spans_and_gap_even_after_ack() {
    for acknowledged in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let mut p = producer();
        let mut out = Outbox::open(&path, 100_000, 100).unwrap();
        let mut tracker = PolicyGap::default();
        let originals = [
            span(&mut p, false),
            span(&mut p, true),
            tracker.observe(true, p.time(), &mut p).unwrap(),
        ];
        for e in &originals {
            out.enqueue(e, &[]).unwrap();
        }
        let mut heartbeat = p.calibration();
        heartbeat.time.source_start = heartbeat
            .time
            .source_start
            .map(|t| t + chrono::Duration::seconds(5));
        heartbeat.time.observed_at = heartbeat.time.source_start.unwrap();
        heartbeat.time.mono_start.as_mut().unwrap().nanos += 5_000_000_000;
        let last = heartbeat.time.clone();
        out.enqueue(&heartbeat, &[]).unwrap();
        if acknowledged {
            let b = out.batch().unwrap().unwrap();
            out.ack(&UploadAck {
                batch_id: b.batch_id,
                accepted: b.envelopes.len() as u32,
                duplicates: 0,
                rejected: vec![],
            })
            .unwrap();
        }
        drop(out);
        let mut out = Outbox::open(&path, 100_000, 100).unwrap();
        let mut now = last.clone();
        now.source_start = now.source_start.map(|t| t + chrono::Duration::seconds(20));
        now.observed_at = now.source_start.unwrap();
        now.mono_start.as_mut().unwrap().nanos += 20_000_000_000;
        out.reconcile(now.clone()).unwrap();
        let batch = out.batch().unwrap().unwrap();
        for original in &originals {
            let closed = batch
                .envelopes
                .iter()
                .find(|e| e.source_key == original.source_key && e.revision.as_str() == "1")
                .expect("recovered closure");
            assert_eq!(closed.time.source_end, last.source_start);
            assert_eq!(closed.time.mono_end, last.mono_start);
            if closed.kind.as_str() == "policy.gap" {
                assert_eq!(closed.payload["open"], false);
            }
        }
        let gap = batch
            .envelopes
            .iter()
            .find(|e| {
                e.kind.as_str() == "policy.gap" && e.payload["reason"] == "capture_interrupted"
            })
            .expect("explicit interruption");
        assert_eq!(gap.time.source_start, last.source_start);
        assert_eq!(gap.time.source_end, now.source_start);
        let count = out.stats().unwrap().0;
        out.reconcile(now).unwrap();
        assert_eq!(
            out.stats().unwrap().0,
            count,
            "reconciliation must be idempotent"
        );
    }
}
#[test]
fn captured_observation_budget_failure_has_durable_loss() {
    for mode in ["event", "oversized", "pinned", "eviction_loss"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let mut p = producer();
        let mut out = Outbox::open(&path, 100_000, 8).unwrap();
        let mut e = p.calibration();
        e.kind = Kind::new("media.still");
        e.payload = serde_json::to_value(MediaStill {
            display: "synthetic".into(),
            trigger: StillTrigger::FocusChange,
            width: 1,
            height: 1,
            app_bundle_id: None,
            window_title: Some("synthetic-content-must-not-appear-in-loss".into()),
        })
        .unwrap();
        let blob = |data: &[u8]| {
            (
                BlobRef {
                    sha256: sha256_hex(data),
                    media_type: "image/jpeg".into(),
                    bytes: data.len() as u64,
                },
                data.to_vec(),
            )
        };
        if ["pinned", "eviction_loss"].contains(&mode) {
            let b = blob(b"shared");
            let mut seed = e.clone();
            seed.blobs.push(b.0.clone());
            out.enqueue(&seed, &[b]).unwrap();
            if mode == "pinned" {
                out.batch().unwrap();
            } else {
                let budget = out.stats().unwrap().1 as u64
                    + serde_json::to_vec(&seed).unwrap().len() as u64
                    + 100;
                drop(out);
                out = Outbox::open(&path, budget, 8).unwrap();
            }
        }
        if mode == "event" {
            drop(out);
            out = Outbox::open(&path, 0, 8).unwrap();
        }
        let b = blob(if mode == "oversized" {
            b"oversized"
        } else {
            b"newdata"
        });
        e.observation_id = ObservationId::generate();
        e.blobs = vec![b.0.clone()];
        assert!(
            !out.capture(&e, &[b]).unwrap(),
            "budget rejection should be handled without stopping capture"
        );
        drop(out);
        let mut out = Outbox::open(&path, 100_000, 8).unwrap();
        if mode == "pinned" {
            let old = out.batch().unwrap().unwrap();
            out.ack(&UploadAck {
                batch_id: old.batch_id,
                accepted: old.envelopes.len() as u32,
                duplicates: 0,
                rejected: vec![],
            })
            .unwrap();
        }
        let batch = out.batch().unwrap().unwrap();
        let loss = batch
            .envelopes
            .iter()
            .find(|e| {
                e.coverage
                    .as_ref()
                    .is_some_and(|c| c.scope == "capture_loss")
            })
            .expect("durable deliverable loss");
        assert_eq!(loss.payload["dropped"], 1);
        assert!(
            !serde_json::to_string(loss)
                .unwrap()
                .contains("synthetic-content-must-not-appear-in-loss")
        );
        let reason = match mode {
            "event" => "event_budget",
            "oversized" => "oversized_media",
            "pinned" => "pinned_media",
            _ => "eviction_loss_budget",
        };
        assert_eq!(loss.payload["reason"], reason);
        if mode == "eviction_loss" {
            assert!(
                out.blob(&sha256_hex(b"shared")).unwrap().is_some(),
                "failed eviction must roll back"
            );
        }
        assert!(
            loss.payload["detail"]
                .as_str()
                .unwrap()
                .contains(&e.observation_id.to_string())
        );
        assert!(
            out.capture(&p.calibration(), &[]).unwrap(),
            "metadata continues after media rejection"
        );
    }
}

#[test]
fn repeated_budget_losses_are_bounded_and_do_not_mutate_pending_uploads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut out = Outbox::open(&path, 0, 0).unwrap();
    let mut p = producer();
    assert!(!out.capture(&p.calibration(), &[]).unwrap());
    let frozen = out.batch().unwrap().unwrap();
    for _ in 0..150 {
        assert!(!out.capture(&p.calibration(), &[]).unwrap());
    }
    assert_eq!(out.batch().unwrap().unwrap(), frozen);
    assert_eq!(
        out.stats().unwrap().0,
        2,
        "only the frozen loss and coalesced loss remain"
    );
    assert!(out.stats().unwrap().1 < 4096);
    drop(out);
    let mut out = Outbox::open(&path, 0, 0).unwrap();
    assert_eq!(out.batch().unwrap().unwrap(), frozen);
    out.ack(&UploadAck {
        batch_id: frozen.batch_id,
        accepted: 1,
        duplicates: 0,
        rejected: vec![],
    })
    .unwrap();
    let batch = out.batch().unwrap().unwrap();
    assert_eq!(batch.envelopes.len(), 1);
    assert_eq!(batch.envelopes[0].payload["dropped"], 150);
}
#[test]
fn interrupted_downtime_is_unknown_in_the_real_compiler_across_reboot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut out = Outbox::open(&path, 100_000, 100).unwrap();
    let mut p = producer();
    let focus = span(&mut p, false);
    let start = focus.time.source_start.unwrap();
    out.enqueue(&focus, &[]).unwrap();
    let mut paused = p.core(
        "synthetic-paused",
        "0",
        Operation::Observe,
        CorePayload::CollectorHealth(CollectorHealth {
            collector: "text".into(),
            status: CollectorStatus::Paused,
            last_sequence: None,
            last_observed_at: None,
            queue_age_ms: None,
            dropped: 0,
            detail: None,
        }),
    );
    paused.time = focus.time.clone();
    out.enqueue(&paused, &[]).unwrap();
    let mut now = focus.time.clone();
    now.source_start = Some(start + chrono::Duration::seconds(20));
    now.observed_at = now.source_start.unwrap();
    now.mono_start.as_mut().unwrap().boot_id = BootId::new("synthetic-next-boot");
    now.mono_start.as_mut().unwrap().nanos = 1;
    out.reconcile(now.clone()).unwrap();
    let batch = out.batch().unwrap().unwrap();
    let gap = batch
        .envelopes
        .iter()
        .find(|e| e.payload["reason"] == "capture_interrupted")
        .unwrap();
    assert!(gap.time.mono_start.is_none() && gap.time.mono_end.is_none());
    gap.validate().unwrap();
    let observations: Vec<_> = batch
        .envelopes
        .into_iter()
        .enumerate()
        .map(|(id, envelope)| hx_compile::Observation {
            id: id as i64,
            received_at: envelope.time.observed_at,
            envelope,
        })
        .collect();
    let timeline = hx_compile::compile(
        &observations,
        Interval::new(start, now.source_start.unwrap()).unwrap(),
    );
    assert!(
        timeline
            .coverage
            .iter()
            .any(|c| c.state == hx_compile::CoverageState::Unknown
                && c.interval == Interval::new(start, now.source_start.unwrap()).unwrap())
    );
    assert!(
        timeline
            .coverage
            .iter()
            .all(|c| c.state == hx_compile::CoverageState::Unknown)
    );
}
#[test]
fn orderly_closed_intervals_do_not_create_restart_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Outbox::open(&dir.path().join("db"), 100_000, 100).unwrap();
    let mut p = producer();
    let open = span(&mut p, false);
    out.enqueue(&open, &[]).unwrap();
    let mut closed = open;
    closed.observation_id = ObservationId::generate();
    closed.revision = Revision::new("1");
    closed.time.source_end = closed.time.source_start;
    closed.time.mono_end = closed.time.mono_start.clone();
    out.enqueue(&closed, &[]).unwrap();
    let count = out.stats().unwrap().0;
    out.reconcile(p.time()).unwrap();
    assert_eq!(out.stats().unwrap().0, count);
}

#[test]
fn restart_reconciliation_preserves_pending_batches_and_migrates_old_outboxes() {
    for migrate in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let mut p = producer();
        let mut out = Outbox::open(&path, 100_000, 100).unwrap();
        let open = span(&mut p, false);
        out.enqueue(&open, &[]).unwrap();
        let frozen = out.batch().unwrap().unwrap();
        drop(out);
        if migrate {
            // Recreate the pre-checkpoint schema using synthetic data only.
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("DROP TABLE open_intervals; DROP TABLE observed; DROP TABLE checkpoint_version; DROP TABLE reserved_events;").unwrap();
        }
        let mut out = Outbox::open(&path, 0, 100).unwrap();
        let mut now = open.time.clone();
        now.source_start = now.source_start.map(|t| t + chrono::Duration::seconds(20));
        now.observed_at = now.source_start.unwrap();
        now.mono_start.as_mut().unwrap().nanos += 20_000_000_000;
        out.reconcile(now).unwrap();
        assert_eq!(out.batch().unwrap().unwrap(), frozen);
        out.ack(&UploadAck {
            batch_id: frozen.batch_id,
            accepted: 1,
            duplicates: 0,
            rejected: vec![],
        })
        .unwrap();
        let recovered = out.batch().unwrap().unwrap();
        assert_eq!(recovered.envelopes.len(), 2);
        assert!(
            recovered
                .envelopes
                .iter()
                .any(|e| e.source_key == open.source_key && e.revision.as_str() == "1")
        );
        assert!(
            recovered
                .envelopes
                .iter()
                .any(|e| e.payload["reason"] == "capture_interrupted")
        );
    }
}

#[test]
fn restart_does_not_reinterpret_companion_span_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Outbox::open(&dir.path().join("db"), 100_000, 100).unwrap();
    let mut p = producer();
    let mut companion = span(&mut p, false);
    companion.connector_instance = ConnectorInstance::new("companion:browser:synthetic");
    companion.revision = Revision::new("opaque-companion-revision");
    out.enqueue(&companion, &[]).unwrap();
    out.reconcile(p.time()).unwrap();
    assert_eq!(out.stats().unwrap().0, 1);
}
