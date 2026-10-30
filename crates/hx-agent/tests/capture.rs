use hx_agent::{
    config::Config,
    envelope::Producer,
    native_messaging::{MAX_FRAME, read_frame, reply},
    outbox::Outbox,
    policy::{Policy, persist},
    upload::Uploader,
};
use hx_schema::{kinds::*, *};
fn event() -> Envelope {
    Producer::new("test".into()).unwrap().core(
        ulid::Ulid::generate().to_string(),
        "0",
        Operation::Observe,
        CorePayload::PresenceSpan(PresenceSpan {
            state: DeviceState::Active,
            idle_threshold_s: Some(300),
        }),
    )
}
fn media(data: &[u8]) -> (Envelope, (BlobRef, Vec<u8>)) {
    let mut e = event();
    e.kind = Kind::new("media.still");
    e.payload = serde_json::to_value(MediaStill {
        display: "main".into(),
        trigger: StillTrigger::FocusChange,
        width: 100,
        height: 100,
        app_bundle_id: Some("test.app".into()),
        window_title: None,
    })
    .unwrap();
    let b = BlobRef {
        sha256: sha256_hex(data),
        media_type: "image/jpeg".into(),
        bytes: data.len() as u64,
    };
    e.blobs = vec![b.clone()];
    (e, (b, data.to_vec()))
}
fn db() -> (tempfile::TempDir, Outbox) {
    let dir = tempfile::tempdir().unwrap();
    let out = Outbox::open(&dir.path().join("outbox"), 100_000, 8).unwrap();
    (dir, out)
}
fn ack(batch: &UploadBatch) -> UploadAck {
    UploadAck {
        batch_id: batch.batch_id,
        accepted: batch.envelopes.len() as u32,
        duplicates: 0,
        rejected: vec![],
    }
}
#[test]
fn event_budget_rejects_without_removing_unacked() {
    let dir = tempfile::tempdir().unwrap();
    let e = event();
    let mut out = Outbox::open(
        &dir.path().join("db"),
        serde_json::to_vec(&e).unwrap().len() as u64,
        100,
    )
    .unwrap();
    out.enqueue(&e, &[]).unwrap();
    assert!(out.enqueue(&event(), &[]).is_err());
    assert_eq!(out.stats().unwrap().0, 1);
}
#[test]
fn oldest_media_eviction_is_durable_and_emits_loss() {
    let (_dir, mut out) = db();
    let (a, ba) = media(b"aaaaaa");
    let (b, bb) = media(b"bbbbbb");
    out.enqueue(&a, std::slice::from_ref(&ba)).unwrap();
    let losses = out.enqueue(&b, std::slice::from_ref(&bb)).unwrap();
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].sha256, ba.0.sha256);
    assert!(out.blob(&ba.0.sha256).unwrap().is_none());
    assert!(out.blob(&bb.0.sha256).unwrap().is_some());
    let batch = out.batch().unwrap().unwrap();
    assert!(batch.envelopes.iter().any(|e| {
        e.kind.as_str() == "collector.health"
            && e.payload["detail"]
                .as_str()
                .unwrap()
                .contains("media_evicted")
    }));
    assert_eq!(out.losses().unwrap().len(), 1);
}
#[test]
fn ack_must_match_and_blob_lives_until_event_ack() {
    let (_dir, mut out) = db();
    let (e, b) = media(b"evidence");
    out.enqueue(&e, std::slice::from_ref(&b)).unwrap();
    let batch = out.batch().unwrap().unwrap();
    let mut wrong = ack(&batch);
    wrong.batch_id = ulid::Ulid::generate();
    assert!(out.ack(&wrong).is_err());
    assert!(out.blob(&b.0.sha256).unwrap().is_some());
    let mut incomplete = ack(&batch);
    incomplete.accepted = 0;
    assert!(out.ack(&incomplete).is_err());
    out.ack(&ack(&batch)).unwrap();
    assert_eq!(out.stats().unwrap(), (0, 0, 0));
}
#[test]
fn pending_batch_and_observation_survive_reopen() {
    let (dir, mut out) = db();
    out.enqueue(&event(), &[]).unwrap();
    let first = out.batch().unwrap().unwrap();
    drop(out);
    let mut reopened = Outbox::open(&dir.path().join("outbox"), 100_000, 8).unwrap();
    let second = reopened.batch().unwrap().unwrap();
    assert_eq!(first, second);
    reopened.ack(&ack(&second)).unwrap();
    assert!(reopened.batch().unwrap().is_none());
}
#[test]
fn in_flight_media_cannot_be_evicted() {
    let (_dir, mut out) = db();
    let (a, ba) = media(b"aaaaaa");
    out.enqueue(&a, std::slice::from_ref(&ba)).unwrap();
    let batch = out.batch().unwrap().unwrap();
    let (b, bb) = media(b"bbbbbb");
    assert!(out.enqueue(&b, &[bb]).is_err());
    assert_eq!(out.batch().unwrap().unwrap(), batch);
    assert!(out.blob(&ba.0.sha256).unwrap().is_some());
}
#[test]
fn excluded_app_produces_no_text_still_or_blob_rows() {
    let (_dir, mut out) = db();
    let config = Config {
        excluded_bundle_ids: vec!["secret.app".into()],
        ..Default::default()
    };
    let policy = Policy::new(&config).unwrap();
    let focus = hx_macos_sys::Focus {
        bundle_id: "secret.app".into(),
        name: "secret".into(),
        pid: 1,
        title: Some("private".into()),
        electron: false,
    };
    let (still, blob) = media(b"secret");
    assert!(!persist(&mut out, &policy, Some(&focus), false, &still, &[blob]).unwrap());
    let mut text = event();
    text.kind = Kind::new("text.snapshot");
    text.payload = serde_json::json!({"text":"secret"});
    assert!(!persist(&mut out, &policy, Some(&focus), false, &text, &[]).unwrap());
    assert_eq!(out.stats().unwrap(), (0, 0, 0));
}
#[test]
fn title_unknown_and_secure_input_fail_closed() {
    let (_dir, mut out) = db();
    let policy = Policy::new(&Config {
        excluded_title_regexes: vec!["(?i)private".into()],
        ..Default::default()
    })
    .unwrap();
    assert!(!policy.allows("app", None));
    assert!(!policy.allows("app", Some("PRIVATE")));
    assert!(policy.allows("app", Some("work")));
    let (e, b) = media(b"hello");
    let focus = hx_macos_sys::Focus {
        bundle_id: "app".into(),
        name: "app".into(),
        pid: 1,
        title: Some("work".into()),
        electron: false,
    };
    assert!(!persist(&mut out, &policy, Some(&focus), true, &e, &[b]).unwrap());
    assert_eq!(out.stats().unwrap().0, 0);
}
fn companion() -> CompanionEvent {
    CompanionEvent {
        companion: "browser".into(),
        kind: Kind::new("web.visit"),
        source_key: SourceKey::new("tab:1"),
        revision: None,
        occurred_at: chrono::Utc::now(),
        ended_at: None,
        sensitivity: Some(Sensitivity::Normal),
        payload: serde_json::json!({"url":"https://example.org","incognito":false}),
    }
}
#[test]
fn companion_wrap_preserves_source_and_adds_native_provenance() {
    let mut p = Producer::new("mac".into()).unwrap();
    let c = companion();
    let e = p.companion(c.clone()).unwrap();
    assert_eq!(e.connector_instance.as_str(), "companion:browser:mac");
    assert_eq!(e.payload, c.payload);
    assert_eq!(e.time.source_start, Some(c.occurred_at));
    assert_eq!(
        e.time.mono_start.unwrap().domain,
        ClockDomain::MachContinuous
    );
    assert_eq!(e.sensitivity, Sensitivity::Restricted);
    assert!(e.sequence.is_some());
    assert!(e.device.is_some());
    assert!(e.coverage.is_some());
}
#[test]
fn nested_private_companion_payload_is_suppressed() {
    let policy = Policy::new(&Config::default()).unwrap();
    assert!(!policy.allows_payload(&serde_json::json!({"tab":{"incognito":true}})));
}
#[test]
fn native_messaging_framing_accepts_fragmented_reads_and_rejects_truncation() {
    let c = companion();
    let bytes = serde_json::to_vec(&c).unwrap();
    let mut wire = (bytes.len() as u32).to_le_bytes().to_vec();
    wire.extend(bytes);
    assert_eq!(read_frame(&mut wire.as_slice()).unwrap(), Some(c));
    assert!(read_frame(&mut &wire[..wire.len() - 1]).is_err());
    assert!(read_frame(&mut ((MAX_FRAME + 1) as u32).to_le_bytes().as_slice()).is_err());
    assert!(read_frame(&mut &b""[..]).unwrap().is_none());
    assert!(read_frame(&mut &b"\x01"[..]).is_err());
    let mut out = Vec::new();
    reply(&mut out, &serde_json::json!({"ok":true})).unwrap();
    assert_eq!(
        u32::from_le_bytes(out[..4].try_into().unwrap()) as usize,
        out.len() - 4
    );
}
#[test]
fn invalid_blob_rolls_back_observation() {
    let (_dir, mut out) = db();
    let (e, mut b) = media(b"blob");
    b.1 = b"tampered".to_vec();
    assert!(out.enqueue(&e, &[b]).is_err());
    assert_eq!(out.stats().unwrap(), (0, 0, 0));
}

#[derive(Clone, Default)]
struct Mock {
    seen: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    blobs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
async fn ingest(
    axum::extract::State(state): axum::extract::State<Mock>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    assert_eq!(headers["authorization"], "Bearer test-token");
    assert_eq!(headers["content-encoding"], "zstd");
    let batch: UploadBatch =
        serde_json::from_slice(&zstd::decode_all(body.as_ref()).unwrap()).unwrap();
    let mut accepted = 0;
    let mut duplicates = 0;
    {
        let mut seen = state.seen.lock().unwrap();
        for e in &batch.envelopes {
            if seen.insert(e.observation_id.to_string()) {
                accepted += 1
            } else {
                duplicates += 1
            }
        }
    }
    // Simulate committing successfully and losing the HTTP acknowledgment.
    if state
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        == 0
    {
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    axum::Json(UploadAck {
        batch_id: batch.batch_id,
        accepted,
        duplicates,
        rejected: vec![],
    })
    .into_response()
}
async fn blob(
    axum::extract::State(state): axum::extract::State<Mock>,
    axum::extract::Path(sha): axum::extract::Path<String>,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    assert_eq!(sha, sha256_hex(&body));
    state
        .blobs
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    axum::http::StatusCode::CREATED
}
#[tokio::test]
async fn uploader_retry_after_restart_deduplicates_without_loss() {
    let (dir, mut out) = db();
    let (e, b) = media(b"payload");
    out.enqueue(&e, &[b]).unwrap();
    let state = Mock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/v1/events", axum::routing::post(ingest))
        .route("/v1/blobs/{sha}", axum::routing::put(blob))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config = Config {
        server_url: format!("http://{server}"),
        token: "test-token".into(),
        ..Default::default()
    };
    let mut uploader = Uploader::new(&config).unwrap();
    assert!(uploader.once(&mut out).await.is_err());
    assert_eq!(out.stats().unwrap().0, 1);
    drop(out);
    let mut out = Outbox::open(&dir.path().join("outbox"), 100_000, 8).unwrap();
    let mut uploader = Uploader::new(&config).unwrap();
    uploader.once(&mut out).await.unwrap();
    assert_eq!(out.stats().unwrap(), (0, 0, 0));
    assert_eq!(state.seen.lock().unwrap().len(), 1);
    assert_eq!(state.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(state.blobs.load(std::sync::atomic::Ordering::SeqCst), 2);
    task.abort();
}

#[test]
fn upload_process_child() {
    let Ok(path) = std::env::var("HX_KILL_TEST_DB") else {
        return;
    };
    let url = std::env::var("HX_KILL_TEST_URL").unwrap();
    let mut out = Outbox::open(std::path::Path::new(&path), 100_000, 8).unwrap();
    let mut uploader = Uploader::new(&Config {
        server_url: url,
        token: "test-token".into(),
        ..Default::default()
    })
    .unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(uploader.once(&mut out))
        .unwrap();
}
#[derive(Clone)]
struct KillMock {
    state: Mock,
    committed: std::sync::Arc<tokio::sync::Notify>,
}
async fn kill_ingest(
    axum::extract::State(state): axum::extract::State<KillMock>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let batch: UploadBatch =
        serde_json::from_slice(&zstd::decode_all(body.as_ref()).unwrap()).unwrap();
    let mut accepted = 0;
    let mut duplicates = 0;
    {
        let mut seen = state.state.seen.lock().unwrap();
        for e in &batch.envelopes {
            if seen.insert(e.observation_id.to_string()) {
                accepted += 1
            } else {
                duplicates += 1
            }
        }
    }
    if state
        .state
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        == 0
    {
        state.committed.notify_one();
        std::future::pending::<()>().await;
    }
    axum::Json(UploadAck {
        batch_id: batch.batch_id,
        accepted,
        duplicates,
        rejected: vec![],
    })
    .into_response()
}
#[tokio::test]
async fn killed_mid_upload_replays_same_batch_without_loss() {
    let (dir, mut out) = db();
    out.enqueue(&event(), &[]).unwrap();
    let original = out.batch().unwrap().unwrap();
    drop(out);
    let state = KillMock {
        state: Mock::default(),
        committed: std::sync::Arc::new(tokio::sync::Notify::new()),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/v1/events", axum::routing::post(kill_ingest))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "upload_process_child", "--nocapture"])
        .env("HX_KILL_TEST_DB", dir.path().join("outbox"))
        .env("HX_KILL_TEST_URL", format!("http://{address}"))
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        state.committed.notified(),
    )
    .await
    .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let mut out = Outbox::open(&dir.path().join("outbox"), 100_000, 8).unwrap();
    assert_eq!(out.batch().unwrap().unwrap(), original);
    let mut uploader = Uploader::new(&Config {
        server_url: format!("http://{address}"),
        token: "test-token".into(),
        ..Default::default()
    })
    .unwrap();
    uploader.once(&mut out).await.unwrap();
    assert_eq!(out.stats().unwrap().0, 0);
    assert_eq!(state.state.seen.lock().unwrap().len(), 1);
    assert_eq!(
        state.state.calls.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    server.abort();
}

#[test]
fn producer_sequences_continue_after_ack_and_restart() {
    let (dir, mut out) = db();
    let mut e = event();
    e.sequence = Some(50);
    out.enqueue(&e, &[]).unwrap();
    let batch = out.batch().unwrap().unwrap();
    assert_eq!(batch.envelopes[0].sequence, Some(50));
    out.ack(&ack(&batch)).unwrap();
    drop(out);
    let mut out = Outbox::open(&dir.path().join("outbox"), 100_000, 8).unwrap();
    let mut next = event();
    next.sequence = Some(1);
    out.enqueue(&next, &[]).unwrap();
    assert_eq!(
        out.batch().unwrap().unwrap().envelopes[0].sequence,
        Some(51)
    );
}
#[test]
fn browser_without_private_window_signal_is_not_persisted() {
    let (_dir, mut out) = db();
    let p = Policy::new(&Config::default()).unwrap();
    let focus = hx_macos_sys::Focus {
        bundle_id: "com.google.Chrome".into(),
        name: "Chrome".into(),
        pid: 1,
        title: None,
        electron: false,
    };
    let mut c = companion();
    c.payload = serde_json::json!({"url":"https://example.org"});
    let e = Producer::new("test".into()).unwrap().companion(c).unwrap();
    assert!(!persist(&mut out, &p, Some(&focus), false, &e, &[]).unwrap());
    assert_eq!(out.stats().unwrap().0, 0);
}
