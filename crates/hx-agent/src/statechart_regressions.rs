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
fn browser_and_electron_content_is_allowed_but_privacy_gates_remain() {
    for (bundle_id, electron) in [
        ("com.apple.Safari", false),
        ("org.mozilla.firefox", false),
        ("com.google.Chrome.beta", false),
        ("com.todesktop.synthetic", true),
        ("com.anthropic.claudefordesktop", true),
        ("com.tinyspeck.slackmacgap", true),
        ("com.microsoft.VSCode", true),
        ("com.todesktop.230313mzl4w4u92", true),
    ] {
        let policy = Policy::new(&Config::default()).unwrap();
        let focus = Focus {
            bundle_id: bundle_id.into(),
            name: "Synthetic".into(),
            pid: 1,
            title: Some("Synthetic window".into()),
            electron,
        };
        assert!(!policy.excludes_native(&focus));
        let mut p = producer();
        assert!(
            PolicyGap::default()
                .observe(false, p.time(), &mut p)
                .is_none()
        );
        for kind in ["text.snapshot", "ui.click", "media.still", "media.chunk"] {
            let dir = tempfile::tempdir().unwrap();
            let mut out = Outbox::open(&dir.path().join("db"), 100_000, 100).unwrap();
            let mut event = p.calibration();
            event.kind = Kind::new(kind);
            event.payload = serde_json::json!({"text":"synthetic content"});
            assert!(!persist(&mut out, &policy, Some(&focus), true, &event, &[]).unwrap());
            assert!(!persist(&mut out, &policy, None, false, &event, &[]).unwrap());
            for config in [
                Config {
                    excluded_bundle_ids: vec![bundle_id.into()],
                    ..Config::default()
                },
                Config {
                    excluded_title_regexes: vec!["Synthetic".into()],
                    ..Config::default()
                },
            ] {
                let excluded = Policy::new(&config).unwrap();
                assert!(excluded.excludes_native(&focus));
                assert!(!persist(&mut out, &excluded, Some(&focus), false, &event, &[]).unwrap());
            }
            assert!(persist(&mut out, &policy, Some(&focus), false, &event, &[]).unwrap());
            assert_eq!(out.stats().unwrap().0, 1);
        }
    }
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

// NoRecordStuck: a complete per-record refusal must not pin the immutable batch.
#[test]
fn no_record_stuck_after_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut p = producer();
    let mut out = Outbox::open(&path, 100_000, 100).unwrap();
    let mut refused = p.calibration();
    let bytes = b"synthetic media".to_vec();
    let blob = BlobRef {
        sha256: sha256_hex(&bytes),
        media_type: "video/mp4".into(),
        bytes: bytes.len() as u64,
    };
    refused.blobs.push(blob.clone());
    out.enqueue(&refused, &[(blob.clone(), bytes.clone())])
        .unwrap();
    out.enqueue(&p.calibration(), &[]).unwrap();
    let batch = out.batch().unwrap().unwrap();
    let ack = UploadAck {
        batch_id: batch.batch_id,
        accepted: 1,
        duplicates: 0,
        rejected: vec![Rejected {
            observation_id: refused.observation_id,
            reason: "synthetic refusal".into(),
        }],
    };
    let mut duplicate = ack.clone();
    duplicate.accepted = 0;
    duplicate.rejected.push(duplicate.rejected[0].clone());
    assert!(out.ack(&duplicate).is_err());
    assert_eq!(out.batch().unwrap().unwrap(), batch);
    out.ack(&ack)
        .expect("one refused record must not block delivery");
    drop(out);
    let mut out = Outbox::open(&path, 100_000, 100).unwrap();
    assert!(out.batch().unwrap().is_none());
    assert_eq!(out.dead_letter_count().unwrap(), 1);
    assert_eq!(out.blob(&blob.sha256).unwrap().unwrap().1, bytes);
    let db = rusqlite::Connection::open(&path).unwrap();
    let (json, reason): (Vec<u8>, String) = db
        .query_row("SELECT json,reason FROM dead_letters", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Envelope>(&json)
            .unwrap()
            .observation_id,
        refused.observation_id
    );
    assert_eq!(reason, "synthetic refusal");
    out.enqueue(&p.calibration(), &[]).unwrap();
    let later = out.batch().unwrap().unwrap();
    assert_ne!(later.batch_id, batch.batch_id);
    // Unknown/duplicate record IDs and incomplete acknowledgements preserve data.
    for invalid in [
        UploadAck {
            batch_id: later.batch_id,
            accepted: 0,
            duplicates: 0,
            rejected: vec![],
        },
        UploadAck {
            batch_id: later.batch_id,
            accepted: 0,
            duplicates: 0,
            rejected: ack.rejected,
        },
    ] {
        assert!(out.ack(&invalid).is_err());
        assert_eq!(out.batch().unwrap().unwrap(), later);
    }
}

// Exercise the SDK's public sync boundary, including its on-disk checkpoint.
#[test]
fn no_record_stuck_in_connector_after_a_refusal_or_http_failure() {
    use std::io::{BufRead, Read, Write};
    for status in [200, 401, 429, 503] {
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("content-length")
                    {
                        len = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let batch: UploadBatch =
                    serde_json::from_slice(&zstd::decode_all(body.as_slice()).unwrap()).unwrap();
                let body = serde_json::to_vec(&UploadAck {
                    batch_id: batch.batch_id,
                    accepted: (batch.envelopes.len() - 1) as u32,
                    duplicates: 0,
                    rejected: vec![Rejected {
                        observation_id: batch.envelopes[0].observation_id,
                        reason: "synthetic refusal".into(),
                    }],
                })
                .unwrap();
                write!(socket,"HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        let cli = || hx_connector_sdk::Cli {
            command: hx_connector_sdk::Command::Sync {
                dry_run: false,
                limit: 20,
                since: None,
            },
            server: Some(url.clone()),
            token: Some("synthetic".into()),
            instance: Some("synthetic".into()),
            source: Some("synthetic".into()),
            state_dir: Some(dir.path().into()),
            root: vec![],
            depth: 1,
            apple_token: None,
        };
        let sync = || {
            // A process spawned by a parallel test can briefly hold a copy of the lock's
            // file descriptor, so a run right after the last one may find it still taken.
            for _ in 0..100 {
                let result =
                    hx_connector_sdk::run_cli(cli(), "synthetic", "synthetic", &[], |_, cursor| {
                        let mut result = hx_connector_sdk::Scan::new(serde_json::json!(
                            cursor.as_u64().unwrap_or(0) + 1
                        ));
                        result.envelopes = vec![producer().calibration(), producer().calibration()];
                        Ok(result)
                    });
                match result {
                    Err(error) if error.to_string().contains("already syncing") => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    other => return other,
                }
            }
            panic!("connector lock stayed held for two seconds");
        };
        let result = sync();
        if status == 200 {
            assert!(
                result.is_ok(),
                "per-record refusals must advance the cursor: {result:?}"
            );
            sync().unwrap();
            sync().unwrap();
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("synthetic.json")).unwrap())
                    .unwrap();
            assert_eq!(state["cursor"], 3);
            let db = rusqlite::Connection::open(dir.path().join("synthetic.dead-letter.sqlite"))
                .unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM dead_letters WHERE reason='synthetic refusal'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                3
            );
            let health: serde_json::Value = serde_json::from_slice(
                &std::fs::read(dir.path().join("synthetic.health.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(health["dropped"], 3);
        } else {
            assert!(result.is_err());
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("synthetic.json")).unwrap())
                    .unwrap();
            assert!(state["cursor"].is_null());
            assert!(state["pending"].is_object());
            assert!(!dir.path().join("synthetic.dead-letter.sqlite").exists());
        }
        server.join().unwrap();
    }
}

// ToldWhenScreenFails: the menu consumes the same health shape as persistence.
// The real AppKit flow is reserved for the owner's login session.
#[test]
fn told_when_screen_fails_and_clears_on_recovery_or_pause() {
    use crate::runtime::screen_warning;
    let mut health = serde_json::json!({"media":{
        "status":"failed", "detail":"writer start: Cannot create file"
    }});
    let warning = screen_warning(Some(&health), false).unwrap();
    assert!(warning.contains("Cannot save screen recordings"));
    assert!(warning.contains("permissions"));
    assert!(screen_warning(Some(&health), true).is_none());
    // Starting a stream is not recovery; only a successfully written frame is.
    assert!(screen_warning(Some(&health), false).is_some());
    health["media"]["status"] = serde_json::json!("running");
    health["media"]["observed_at"] = serde_json::json!(chrono::Utc::now());
    assert!(screen_warning(Some(&health), false).is_none());
    health["media"]["observed_at"] =
        serde_json::json!(chrono::Utc::now() - chrono::Duration::seconds(56));
    assert!(
        screen_warning(Some(&health), false)
            .unwrap()
            .contains("not responding")
    );
    health["media"]["status"] = serde_json::json!("paused");
    assert!(screen_warning(Some(&health), false).is_none());
    health["media"]["status"] = serde_json::json!("permission_denied");
    assert!(
        screen_warning(Some(&health), false)
            .unwrap()
            .contains("Screen Recording permission")
    );
}

#[test]
fn recording_comes_back_after_failed_writer_or_missing_staging_folder() {
    let dir = tempfile::tempdir().unwrap();
    let staging = dir.path().join("media-staging");
    let failure = crate::media::staged_writer(&staging, |path| -> anyhow::Result<()> {
        std::fs::write(path, [])?;
        anyhow::bail!("writer start: Cannot create file");
    });
    assert!(
        failure
            .unwrap_err()
            .to_string()
            .contains("Cannot create file")
    );
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);
    std::fs::remove_dir(&staging).unwrap();
    let (path, ()) = crate::media::staged_writer(&staging, |path| {
        std::fs::write(path, b"synthetic video")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"synthetic video");
    // An existing unwritable directory fails visibly, then recovers after repair.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(
        crate::media::staged_writer(&staging, |path| Ok(std::fs::write(path, b"synthetic")?))
            .is_err()
    );
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        crate::media::staged_writer(&staging, |path| Ok(std::fs::write(path, b"synthetic")?))
            .is_ok()
    );
}

#[test]
fn recording_comes_back_on_crash_but_stays_quit_after_success() {
    // Validate the exact fallback plist with Apple's parser, without registering it.
    let dir = tempfile::tempdir().unwrap();
    let plist = crate::startup::launch_agent(
        &dir.path()
            .join("Made-up & App.app/Contents/MacOS/Hyperthymesia"),
    )
    .unwrap();
    let path = dir.path().join("agent.plist");
    std::fs::write(&path, plist).unwrap();
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["RunAtLoad"], true);
    assert_eq!(value["KeepAlive"]["SuccessfulExit"], false);
    assert_eq!(value["ProgramArguments"][1], "--managed");
    assert!(
        value["ProgramArguments"][0]
            .as_str()
            .unwrap()
            .contains("Made-up & App.app")
    );
}
