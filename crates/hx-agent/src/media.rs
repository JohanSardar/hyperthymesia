use crate::{
    config::{Config, Profile, support_dir},
    envelope::Producer,
    policy::Policy,
    runtime::Submission,
};
use hx_macos_sys::{Focus, ScreenStream, VideoWriter};
use hx_schema::{kinds::*, *};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
struct Chunk {
    writer: VideoWriter,
    path: std::path::PathBuf,
    start: ObservationTime,
    started: Instant,
    width: u32,
    height: u32,
    focus: Focus,
}
struct Session {
    stream: ScreenStream,
    focus: Focus,
}
fn run_inner(
    config: Config,
    tx: tokio::sync::mpsc::Sender<Submission>,
    context: Arc<Mutex<Option<Focus>>>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let policy = Policy::new(&config)?;
    let mut producer = Producer::new(config.device_id.clone())?.namespace("macos-media");
    tx.blocking_send(Submission {
        envelope: producer.calibration(),
        reply: None,
        focus: None,
        secure: false,
        blobs: vec![],
    })?;
    let staging = support_dir().join("media-staging");
    if staging.exists()
        && std::fs::read_dir(&staging)?
            .any(|entry| entry.is_ok_and(|e| e.path().extension().is_some_and(|ext| ext == "mp4")))
    {
        send_health(&mut producer,&tx,CollectorStatus::Failed,"incomplete staging files from an interrupted capture are retained; recovery requires validation before upload".into(),0)?;
    }
    let mut session: Option<Session> = None;
    let mut chunk: Option<Chunk> = None;
    let mut last_still = Instant::now() - Duration::from_secs(3600);
    let mut hour = Instant::now();
    let mut spent = 0u64;
    let mut last_hash = String::new();
    let mut last_health = Instant::now() - Duration::from_secs(60);
    let mut retry = Instant::now() - Duration::from_secs(60);
    while !stop.load(Ordering::Relaxed) {
        let mut focus = context.lock().expect("capture context").clone();
        if let Some(f) = focus.as_mut() {
            f.title = hx_macos_sys::window_title(f.pid);
        }
        let allowed = !paused.load(Ordering::Relaxed)
            && !hx_macos_sys::secure_input()
            && hx_macos_sys::permissions().screen
            && focus.as_ref().is_some_and(|f| {
                policy.allows(&f.bundle_id, f.title.as_deref()) && !browser(f) && f.title.is_some()
            });
        if last_health.elapsed() >= Duration::from_secs(60) {
            last_health = Instant::now();
            let granted = hx_macos_sys::permissions().screen;
            let status = if !granted {
                CollectorStatus::PermissionDenied
            } else if !allowed {
                CollectorStatus::Paused
            } else if session.is_some() {
                CollectorStatus::Running
            } else {
                CollectorStatus::Failed
            };
            send_health(&mut producer,&tx,status,if !granted {"PermissionDenied: use explicit onboard command"}else if !allowed {"content suppressed: policy, unavailable focused-window identity, private-window state, pause or Secure Input"}else {"foreground-window capture only; full-display coverage unavailable"}.into(),session.as_ref().map_or(0,|s|s.stream.dropped()))?;
        }
        let changed = session
            .as_ref()
            .is_some_and(|s| focus.as_ref() != Some(&s.focus));
        if !allowed || changed {
            session = None;
            if let Some(c) = chunk.take() {
                finish(c, &mut producer, &tx)?;
            }
        }
        if allowed && session.is_none() && retry.elapsed() >= Duration::from_secs(10) {
            retry = Instant::now();
            if let Some(f) = &focus {
                match ScreenStream::start(f) {
                    Ok(stream) => {
                        session = Some(Session {
                            stream,
                            focus: f.clone(),
                        });
                        last_still = Instant::now() - Duration::from_secs(3600);
                        last_hash.clear();
                    }
                    Err(error) => {
                        send_health(&mut producer, &tx, CollectorStatus::Failed, error, 0)?;
                    }
                }
            }
        }
        if hour.elapsed() >= Duration::from_secs(3600) {
            hour = Instant::now();
            spent = 0;
        }
        if let Some(s) = &session {
            while let Some(frame) = s.stream.pop() {
                // Recheck native focus immediately before encoding or writing any frame.
                let mut now = hx_macos_sys::frontmost();
                if let Some(f) = now.as_mut() {
                    f.title = hx_macos_sys::window_title(f.pid);
                }
                if now.as_ref() != Some(&s.focus)
                    || paused.load(Ordering::Relaxed)
                    || hx_macos_sys::secure_input()
                {
                    continue;
                }
                if chunk
                    .as_ref()
                    .is_some_and(|c| c.started.elapsed() >= Duration::from_secs(60))
                    && let Some(c) = chunk.take()
                {
                    finish(c, &mut producer, &tx)?;
                }
                if chunk.is_none() {
                    let dir = support_dir().join("media-staging");
                    std::fs::create_dir_all(&dir)?;
                    let path = dir.join(format!("{}.mp4", ulid::Ulid::generate()));
                    let writer = VideoWriter::new(&path, s.stream.width, s.stream.height)
                        .map_err(anyhow::Error::msg)?;
                    chunk = Some(Chunk {
                        writer,
                        path,
                        start: producer.time(),
                        started: Instant::now(),
                        width: s.stream.width,
                        height: s.stream.height,
                        focus: s.focus.clone(),
                    });
                }
                if let Some(c) = chunk.as_mut()
                    && !c.writer.append(&frame).map_err(anyhow::Error::msg)?
                {
                    send_health(
                        &mut producer,
                        &tx,
                        CollectorStatus::Running,
                        "encoder backpressure: frame skipped".into(),
                        1,
                    )?;
                }
                let profile = Profile::select(hx_macos_sys::power(), &config);
                let cadence = config.still_cadence_s.max(match profile {
                    Profile::Full => 1,
                    Profile::Balanced => 30,
                    Profile::Saver => 120,
                });
                if last_still.elapsed() >= Duration::from_secs(cadence) {
                    last_still = Instant::now();
                    match frame.jpeg() {
                        Ok(bytes) => {
                            let hash = sha256_hex(&bytes);
                            if hash == last_hash {
                                continue;
                            }
                            if spent.saturating_add(bytes.len() as u64)
                                > config.still_budget_bytes_per_hour
                            {
                                send_health(
                                    &mut producer,
                                    &tx,
                                    CollectorStatus::Running,
                                    "still skipped: hourly byte budget".into(),
                                    1,
                                )?;
                                continue;
                            }
                            spent += bytes.len() as u64;
                            let trigger = if last_hash.is_empty() {
                                StillTrigger::FocusChange
                            } else {
                                StillTrigger::ContentChange
                            };
                            last_hash = hash.clone();
                            let mut e = producer.core(
                                format!("still:{}", ulid::Ulid::generate()),
                                "0",
                                Operation::Observe,
                                CorePayload::MediaStill(MediaStill {
                                    display: "main:foreground_window_only".into(),
                                    trigger,
                                    width: s.stream.width,
                                    height: s.stream.height,
                                    app_bundle_id: Some(s.focus.bundle_id.clone()),
                                    window_title: s.focus.title.clone(),
                                }),
                            );
                            e.time.mono_start.as_mut().expect("native stamp").nanos = frame.nanos;
                            let blob = BlobRef {
                                sha256: hash,
                                media_type: "image/jpeg".into(),
                                bytes: bytes.len() as u64,
                            };
                            e.blobs.push(blob.clone());
                            tx.blocking_send(Submission {
                                envelope: e,
                                reply: None,
                                focus: Some(s.focus.clone()),
                                secure: false,
                                blobs: vec![(blob, bytes)],
                            })?;
                        }
                        Err(error) => {
                            send_health(&mut producer, &tx, CollectorStatus::Failed, error, 1)?
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(session);
    if let Some(c) = chunk {
        finish(c, &mut producer, &tx)?;
    }
    Ok(())
}
fn browser(f: &Focus) -> bool {
    hx_macos_sys::skip_ax(f)
        || ["com.apple.Safari", "org.mozilla.firefox"]
            .iter()
            .any(|b| f.bundle_id.starts_with(b))
}
fn finish(
    c: Chunk,
    p: &mut Producer,
    tx: &tokio::sync::mpsc::Sender<Submission>,
) -> anyhow::Result<()> {
    let frames = c.writer.frames;
    let result = c.writer.finish();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&c.path);
        send_health(p, tx, CollectorStatus::Failed, e, 1)?;
        return Ok(());
    }
    let bytes = std::fs::read(&c.path)?;
    let mut e = p.core(
        format!("chunk:{}", ulid::Ulid::generate()),
        "0",
        Operation::Observe,
        CorePayload::MediaChunk(MediaChunk {
            display: "main:foreground_window_only".into(),
            codec: "hevc".into(),
            width: c.width,
            height: c.height,
            frames,
        }),
    );
    let end = e.time.clone();
    e.time = c.start;
    e.time.source_end = end.source_start;
    e.time.mono_end = end.mono_start;
    let blob = BlobRef {
        sha256: sha256_hex(&bytes),
        media_type: "video/mp4".into(),
        bytes: bytes.len() as u64,
    };
    e.blobs.push(blob.clone());
    let (reply, ack) = std::sync::mpsc::sync_channel(1);
    tx.blocking_send(Submission {
        envelope: e,
        reply: Some(reply),
        focus: Some(c.focus),
        secure: false,
        blobs: vec![(blob, bytes)],
    })?;
    anyhow::ensure!(
        ack.recv_timeout(Duration::from_secs(45))?,
        "media declined by persistence policy"
    );
    std::fs::remove_file(&c.path)?;
    Ok(())
}
fn send_health(
    p: &mut Producer,
    tx: &tokio::sync::mpsc::Sender<Submission>,
    status: CollectorStatus,
    detail: String,
    dropped: u64,
) -> anyhow::Result<()> {
    let last = p.last_for("media");
    let e = p.core(
        format!("media-health:{}", ulid::Ulid::generate()),
        "0",
        Operation::Observe,
        CorePayload::CollectorHealth(CollectorHealth {
            collector: "media".into(),
            status,
            last_sequence: last.map(|v| v.0),
            last_observed_at: last.map(|v| v.1),
            queue_age_ms: None,
            dropped,
            detail: Some(detail),
        }),
    );
    tx.blocking_send(Submission {
        envelope: e,
        reply: None,
        focus: None,
        secure: false,
        blobs: vec![],
    })?;
    Ok(())
}

pub(crate) fn run(
    config: Config,
    tx: tokio::sync::mpsc::Sender<Submission>,
    context: Arc<Mutex<Option<Focus>>>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let result = run_inner(config.clone(), tx.clone(), context, paused, stop);
    if let Err(error) = &result {
        let mut producer = Producer::new(config.device_id)?.namespace("macos-media");
        send_health(
            &mut producer,
            &tx,
            CollectorStatus::Failed,
            error.to_string(),
            1,
        )?;
    }
    result
}
