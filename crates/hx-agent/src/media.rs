use crate::{
    config::{Config, Profile, support_dir},
    envelope::Producer,
    policy::Policy,
    runtime::Submission,
};
use hx_macos_sys::{Focus, Frame, ScreenStream, VideoWriter};
use hx_schema::{kinds::*, *};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
pub(crate) type LiveHealth = Arc<Mutex<Option<serde_json::Value>>>;
struct Chunk {
    writer: VideoWriter,
    path: std::path::PathBuf,
    start: ObservationTime,
    last_frame: ObservationTime,
    started: Instant,
    width: u32,
    height: u32,
    focus: Focus,
}
struct Session {
    stream: ScreenStream,
    focus: Focus,
    stills: StillFrames<Frame>,
}
/// The interval owns the latest approved sample independently of SCK arrivals.
struct StillFrames<T> {
    latest: Option<T>,
    last_attempt: Option<Instant>,
}
impl<T> StillFrames<T> {
    fn new() -> Self {
        Self {
            latest: None,
            last_attempt: None,
        }
    }
    fn sample(&mut self, now: Instant, cadence_s: u64) -> Option<&T> {
        if self.latest.is_none()
            || self.last_attempt.is_some_and(|last| {
                now.duration_since(last) < Duration::from_secs(cadence_s.max(1))
            })
        {
            return None;
        }
        self.last_attempt = Some(now);
        self.latest.as_ref()
    }
}
fn run_inner(
    config: Config,
    tx: tokio::sync::mpsc::Sender<Submission>,
    context: Arc<Mutex<Option<Focus>>>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    mut failure: Option<String>,
    live: LiveHealth,
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
    if staging.exists() {
        for entry in std::fs::read_dir(&staging)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|ext| ext == "mp4")
                && entry.metadata()?.len() == 0
            {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    if staging.exists()
        && std::fs::read_dir(&staging)?
            .any(|entry| entry.is_ok_and(|e| e.path().extension().is_some_and(|ext| ext == "mp4")))
    {
        send_health(&mut producer,&tx, &live, CollectorStatus::Failed,"incomplete staging files from an interrupted capture are retained; recovery requires validation before upload".into(),0)?;
    }
    let mut session: Option<Session> = None;
    let mut chunk: Option<Chunk> = None;
    let mut hour = Instant::now();
    let mut spent = 0u64;
    let mut last_hash = String::new();
    let mut last_health = Instant::now() - Duration::from_secs(45);
    let mut last_screen = None;
    let mut retry = Instant::now() - Duration::from_secs(60);
    while !stop.load(Ordering::Relaxed) {
        let mut focus = context.lock().expect("capture context").clone();
        if let Some(f) = focus.as_mut() {
            f.title = hx_macos_sys::window_title(f);
        }
        let screen = hx_macos_sys::permissions().screen;
        let active = capture_active(
            hx_macos_sys::idle_seconds(),
            config.idle_threshold_s,
            hx_macos_sys::session_locked().unwrap_or(true),
            hx_macos_sys::display_asleep(),
        );
        let allowed = active
            && !paused.load(Ordering::Relaxed)
            && !hx_macos_sys::secure_input()
            && screen
            && focus.as_ref().is_some_and(|f| {
                policy.allows(&f.bundle_id, f.title.as_deref()) && f.title.is_some()
            });
        if health_due(last_health.elapsed(), last_screen, screen) {
            last_health = Instant::now();
            last_screen = Some(screen);
            let mut clock = producer.core(
                format!("clock-health:{}", ulid::Ulid::generate()),
                "0",
                Operation::Observe,
                CorePayload::CollectorHealth(CollectorHealth {
                    collector: "clock".into(),
                    status: CollectorStatus::Running,
                    last_sequence: producer.last_for("clock").map(|v| v.0),
                    last_observed_at: producer.last_for("clock").map(|v| v.1),
                    queue_age_ms: None,
                    dropped: 0,
                    detail: None,
                }),
            );
            clock.coverage = Some(Coverage {
                scope: "clock".into(),
                completeness: Completeness::Partial,
            });
            tx.blocking_send(Submission {
                envelope: clock,
                reply: None,
                focus: None,
                secure: false,
                blobs: vec![],
            })?;
            let granted = screen;
            let status = if !granted {
                CollectorStatus::PermissionDenied
            } else if !allowed {
                CollectorStatus::Paused
            } else if failure.is_none() {
                CollectorStatus::Running
            } else {
                CollectorStatus::Failed
            };
            let detail = if !granted {
                "Screen Recording permission is off. Enable it in System Settings > Privacy & Security > Screen Recording.".into()
            } else if !allowed {
                "Screen recording is paused.".into()
            } else {
                failure
                    .clone()
                    .unwrap_or_else(|| "Recording the foreground window.".into())
            };
            send_health(
                &mut producer,
                &tx,
                &live,
                status,
                detail,
                session.as_ref().map_or(0, |s| s.stream.dropped()),
            )?;
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
                            stills: StillFrames::new(),
                        });
                        last_hash.clear();
                    }
                    Err(error) => {
                        failure = Some(error.clone());
                        send_health(&mut producer, &tx, &live, CollectorStatus::Failed, error, 0)?;
                    }
                }
            }
        }
        if hour.elapsed() >= Duration::from_secs(3600) {
            hour = Instant::now();
            spent = 0;
        }
        if let Some(s) = &mut session {
            if let Some(error) = s.stream.failure() {
                anyhow::bail!("Screen capture stopped: {error}");
            }
            while let Some(frame) = s.stream.pop() {
                // Recheck native focus immediately before encoding or writing any frame.
                let mut now = hx_macos_sys::frontmost();
                if let Some(f) = now.as_mut() {
                    f.title = hx_macos_sys::window_title(f);
                }
                if now.as_ref() != Some(&s.focus)
                    || paused.load(Ordering::Relaxed)
                    || hx_macos_sys::secure_input()
                    || !capture_active(
                        hx_macos_sys::idle_seconds(),
                        config.idle_threshold_s,
                        hx_macos_sys::session_locked().unwrap_or(true),
                        hx_macos_sys::display_asleep(),
                    )
                    || !policy.allows(&s.focus.bundle_id, s.focus.title.as_deref())
                    || !hx_macos_sys::permissions().screen
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
                    let (path, writer) = staged_writer(&dir, |path| {
                        VideoWriter::new(path, s.stream.width, s.stream.height)
                            .map_err(anyhow::Error::msg)
                    })?;
                    chunk = Some(Chunk {
                        writer,
                        path,
                        start: frame_time(producer.time(), frame.nanos),
                        last_frame: frame_time(producer.time(), frame.nanos),
                        started: Instant::now(),
                        width: s.stream.width,
                        height: s.stream.height,
                        focus: s.focus.clone(),
                    });
                }
                if let Some(c) = chunk.as_mut() {
                    if c.writer.append(&frame).map_err(anyhow::Error::msg)? {
                        c.last_frame = frame_time(producer.time(), frame.nanos);
                        if failure.take().is_some() {
                            send_health(
                                &mut producer,
                                &tx,
                                &live,
                                CollectorStatus::Running,
                                "Screen recording recovered.".into(),
                                0,
                            )?;
                        }
                    } else {
                        send_health(
                            &mut producer,
                            &tx,
                            &live,
                            if failure.is_none() {
                                CollectorStatus::Running
                            } else {
                                CollectorStatus::Failed
                            },
                            failure.clone().unwrap_or_else(|| {
                                "The video encoder is not accepting frames.".into()
                            }),
                            1,
                        )?;
                    }
                }
                s.stills.latest = Some(frame);
            }
            // SCK may stop sending complete frames after the last visual change.
            // Keep the newest approved sample so the interval can observe a change
            // that happened between ticks. The hash below suppresses static frames.
            let mut now = hx_macos_sys::frontmost();
            if let Some(f) = now.as_mut() {
                f.title = hx_macos_sys::window_title(f);
            }
            if now.as_ref() != Some(&s.focus)
                || paused.load(Ordering::Relaxed)
                || hx_macos_sys::secure_input()
                || !hx_macos_sys::permissions().screen
                || !capture_active(
                    hx_macos_sys::idle_seconds(),
                    config.idle_threshold_s,
                    hx_macos_sys::session_locked().unwrap_or(true),
                    hx_macos_sys::display_asleep(),
                )
                || !policy.allows(&s.focus.bundle_id, s.focus.title.as_deref())
            {
                s.stills.latest = None;
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let profile = Profile::select(hx_macos_sys::power(), &config);
            let cadence = config.still_cadence_s.max(match profile {
                Profile::Full => 1,
                Profile::Balanced => 30,
                Profile::Saver => 120,
            });
            if let Some(frame) = s.stills.sample(Instant::now(), cadence) {
                match frame.jpeg() {
                    Ok(bytes) => {
                        let hash = sha256_hex(&bytes);
                        let Some(trigger) = still_trigger(&last_hash, &hash) else {
                            continue;
                        };
                        if spent.saturating_add(bytes.len() as u64)
                            > config.still_budget_bytes_per_hour
                        {
                            send_health(
                                &mut producer,
                                &tx,
                                &live,
                                CollectorStatus::Running,
                                "still skipped: hourly byte budget".into(),
                                1,
                            )?;
                            continue;
                        }
                        spent += bytes.len() as u64;
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
                        e.time = frame_time(e.time, frame.nanos);
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
                        failure = Some(error.clone());
                        send_health(&mut producer, &tx, &live, CollectorStatus::Failed, error, 1)?
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
/// Recreate a removed staging directory on every attempt and remove partial
/// output even when AVAssetWriter fails during construction (Cannot create file).
pub(crate) fn staged_writer<T>(
    dir: &std::path::Path,
    create: impl FnOnce(&std::path::Path) -> anyhow::Result<T>,
) -> anyhow::Result<(std::path::PathBuf, T)> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("Cannot create the screen recording folder: {e}"))?;
    let path = dir.join(format!("{}.mp4", ulid::Ulid::generate()));
    match create(&path) {
        Ok(writer) => Ok((path, writer)),
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            Err(anyhow::anyhow!("Cannot save screen recording: {error}"))
        }
    }
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
        return Err(anyhow::anyhow!("Could not finish screen recording: {e}"));
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
    e.time = chunk_interval(c.start, c.last_frame, e.time);
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
    live: &LiveHealth,
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
    // Update the menu before disk/network persistence: an upload can be blocked
    // for several HTTP timeouts, but the person still needs the failure now.
    let mut payload = e.payload.clone();
    payload["observed_at"] = serde_json::json!(e.time.observed_at);
    *live.lock().expect("screen health") = Some(serde_json::json!({"media":payload}));
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
    live: LiveHealth,
) -> anyhow::Result<()> {
    // One failed write (a full disk, a codec error) must not end screen capture for
    // the rest of the session: report it, back off, and start a fresh capture.
    let mut backoff = RESTART_MIN;
    let mut failure = None;
    loop {
        let started = Instant::now();
        let result = run_inner(
            config.clone(),
            tx.clone(),
            context.clone(),
            paused.clone(),
            stop.clone(),
            failure.clone(),
            live.clone(),
        );
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        failure = Some(error.to_string());
        if tx.is_closed() {
            return Err(error);
        }
        let mut producer = Producer::new(config.device_id.clone())?.namespace("macos-media");
        send_health(
            &mut producer,
            &tx,
            &live,
            CollectorStatus::Failed,
            format!("{error}; restarting capture"),
            1,
        )?;
        if started.elapsed() >= RESTART_HEALTHY {
            backoff = RESTART_MIN;
        }
        let resume = Instant::now() + backoff;
        let mut was_paused = false;
        while Instant::now() < resume {
            let is_paused = paused.load(Ordering::Relaxed);
            if is_paused != was_paused {
                send_health(
                    &mut producer,
                    &tx,
                    &live,
                    if is_paused {
                        CollectorStatus::Paused
                    } else {
                        CollectorStatus::Failed
                    },
                    if is_paused {
                        "Screen recording is paused.".into()
                    } else {
                        format!("{error}; restarting capture")
                    },
                    0,
                )?;
                was_paused = is_paused;
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        backoff = next_backoff(backoff);
    }
}

const RESTART_MIN: Duration = Duration::from_secs(15);
const RESTART_MAX: Duration = Duration::from_secs(300);
/// A capture that ran this long before failing restarts at the minimum delay.
const RESTART_HEALTHY: Duration = Duration::from_secs(600);

fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(RESTART_MAX)
}

fn capture_active(idle_s: f64, threshold_s: u32, locked: bool, asleep: bool) -> bool {
    idle_s.is_finite() && idle_s < threshold_s as f64 && !locked && !asleep
}
fn still_trigger(previous_hash: &str, hash: &str) -> Option<StillTrigger> {
    if previous_hash == hash {
        None
    } else if previous_hash.is_empty() {
        Some(StillTrigger::FocusChange)
    } else {
        Some(StillTrigger::Periodic)
    }
}
fn health_due(elapsed: Duration, last_screen: Option<bool>, screen: bool) -> bool {
    elapsed >= Duration::from_secs(45) || last_screen != Some(screen)
}
fn chunk_interval(
    start: ObservationTime,
    last_frame: ObservationTime,
    finish: ObservationTime,
) -> ObservationTime {
    let mut time = start;
    // Wall times derive from separate clock reads, so a one-frame chunk can
    // land nanoseconds before its start. Never emit a negative interval.
    time.source_end = last_frame.source_start.max(time.source_start);
    time.mono_end = match (&time.mono_start, last_frame.mono_start) {
        (Some(s), Some(mut e)) if e.boot_id == s.boot_id && e.domain == s.domain => {
            e.nanos = e.nanos.max(s.nanos);
            Some(e)
        }
        (_, e) => e,
    };
    time.observed_at = finish.observed_at;
    time
}
fn frame_time(mut now: ObservationTime, nanos: u64) -> ObservationTime {
    let stamp = now.mono_start.as_mut().expect("native stamp");
    now.source_start = Some(
        now.observed_at
            - chrono::Duration::nanoseconds(
                stamp.nanos.saturating_sub(nanos).min(i64::MAX as u64) as i64
            ),
    );
    stamp.nanos = nanos;
    now
}

#[cfg(test)]
mod statechart_regressions {
    use super::*;
    #[test]
    fn capture_restarts_back_off_to_a_ceiling() {
        let mut delay = RESTART_MIN;
        for _ in 0..10 {
            delay = next_backoff(delay);
        }
        assert_eq!(delay, RESTART_MAX);
        assert_eq!(next_backoff(RESTART_MIN), RESTART_MIN * 2);
    }
    #[test]
    fn chunk_interval_never_ends_before_it_starts() {
        let at = chrono::Utc::now();
        let stamp = |nanos| hx_schema::MonoStamp {
            domain: hx_schema::ClockDomain::MachContinuous,
            boot_id: hx_schema::BootId::new("synthetic-boot"),
            nanos,
        };
        let mut start = ObservationTime::point(at);
        start.mono_start = Some(stamp(1_000));
        // The same single frame, read again a few nanoseconds of wall-clock jitter earlier.
        let mut last = ObservationTime::point(at - chrono::Duration::nanoseconds(3));
        last.mono_start = Some(stamp(999));
        let time = chunk_interval(start, last, ObservationTime::point(at));
        assert_eq!(time.validate(), Ok(()));
        assert_eq!(time.source_end, Some(at));
        assert_eq!(time.mono_end.map(|s| s.nanos), Some(1_000));
    }
    #[test]
    fn periodic_still_uses_retained_change_and_skips_static_window() {
        let cadence = Config::default().still_cadence_s;
        assert_eq!(cadence, 60);
        assert_eq!(
            still_trigger("", "initial"),
            Some(StillTrigger::FocusChange)
        );
        let start = Instant::now();
        let mut frames = StillFrames::new();
        assert_eq!(frames.sample(start, cadence), None);
        frames.latest = Some("initial");
        assert_eq!(frames.sample(start, cadence), Some(&"initial"));
        // A new sample arrives at 10s, then SCK sends no complete samples.
        frames.latest = Some("changed at 10s");
        assert_eq!(
            frames.sample(start + Duration::from_secs(10), cadence),
            None
        );
        let retained = frames
            .sample(start + Duration::from_secs(60), cadence)
            .unwrap();
        assert_eq!(
            still_trigger("initial", retained),
            Some(StillTrigger::Periodic)
        );
        assert_eq!(
            frames.sample(start + Duration::from_secs(61), cadence),
            None
        );
        let retained = frames
            .sample(start + Duration::from_secs(420), cadence)
            .unwrap();
        assert_eq!(still_trigger("changed at 10s", retained), None);
        // Clearing a suppressed session's retained sample prevents reuse.
        frames.latest = None;
        assert_eq!(
            frames.sample(start + Duration::from_secs(480), cadence),
            None
        );
    }
    #[test]
    fn idle_locked_and_sleeping_devices_suppress_media() {
        assert!(capture_active(10., 300, false, false));
        assert!(!capture_active(300., 300, false, false));
        assert!(!capture_active(0., 300, true, false));
        assert!(!capture_active(0., 300, false, true));
        assert!(!capture_active(f64::NAN, 300, false, false));
    }
    #[test]
    fn screen_permission_loss_is_reported_before_heartbeat() {
        assert!(health_due(Duration::from_secs(1), Some(true), false));
        assert!(!health_due(Duration::from_secs(1), Some(false), false));
        assert!(health_due(Duration::from_secs(46), Some(false), false));
    }
    #[test]
    fn video_finish_timestamp_stops_at_last_accepted_frame() {
        let p = Producer::new("synthetic".into()).unwrap();
        let start = p.time();
        let mut last = start.clone();
        last.source_start = last.source_start.map(|t| t + chrono::Duration::seconds(4));
        last.mono_start.as_mut().unwrap().nanos += 4_000_000_000;
        let boundary = last.source_start.unwrap() + chrono::Duration::seconds(1);
        let mut finish = last.clone();
        finish.source_start = Some(boundary + chrono::Duration::seconds(5));
        finish.mono_start.as_mut().unwrap().nanos += 6_000_000_000;
        let interval = chunk_interval(start, last.clone(), finish);
        assert_eq!(interval.source_end, last.source_start);
        assert_eq!(interval.mono_end, last.mono_start);
        assert!(interval.source_end.unwrap() < boundary);
    }
}
