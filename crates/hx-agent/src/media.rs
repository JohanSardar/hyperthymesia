use crate::{
    config::{Config, Profile},
    envelope::Producer,
    policy::Policy,
    runtime::Submission,
};
use hx_macos_sys::{Focus, Frame, ScreenStream};
use hx_schema::{kinds::*, *};
use hx_visual::VisionCliEngine;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
pub(crate) type LiveHealth = Arc<Mutex<Option<serde_json::Value>>>;
#[derive(Clone)]
pub(crate) struct CaptureSwitch {
    pub paused: Arc<AtomicBool>,
    pub recording: Arc<AtomicBool>,
}
impl CaptureSwitch {
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed) || !self.recording.load(Ordering::Relaxed)
    }
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
    fn take_sample(&mut self, now: Instant, cadence_s: u64) -> Option<T> {
        self.sample(now, cadence_s)?;
        self.latest.take()
    }
}
fn run_inner(
    config: Config,
    tx: tokio::sync::mpsc::Sender<Submission>,
    context: Arc<Mutex<Option<Focus>>>,
    control: CaptureSwitch,
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
    let mut session: Option<Session> = None;
    let engine = VisionCliEngine::default();
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
            && !control.is_paused()
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
                "Screenshots are paused.".into()
            } else {
                failure.clone().unwrap_or_else(|| {
                    "Reading foreground screen text; images are never saved.".into()
                })
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
        if let Some(s) = &mut session {
            if let Some(error) = s.stream.failure() {
                anyhow::bail!("Screen capture stopped: {error}");
            }
            let mut newest = s.stream.pop();
            while let Some(frame) = s.stream.pop() {
                newest = Some(frame);
            }
            if let Some(frame) = newest {
                // Recheck native focus immediately before recognizing any frame.
                let mut now = hx_macos_sys::frontmost();
                if let Some(f) = now.as_mut() {
                    f.title = hx_macos_sys::window_title(f);
                }
                if now.as_ref() != Some(&s.focus)
                    || control.is_paused()
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
                // Replace the previous sample without encoding or recognizing it.
                // Only the latest frame waits in memory for the next cadence tick.
                s.stills.latest = Some(frame);
            }
            // SCK may stop sending complete frames after the last visual change.
            // Retain the latest frame in memory until its cadence tick, even if
            // SCK goes idle. Pixels never enter the outbox or a file.
            let mut now = hx_macos_sys::frontmost();
            if let Some(f) = now.as_mut() {
                f.title = hx_macos_sys::window_title(f);
            }
            if now.as_ref() != Some(&s.focus)
                || control.is_paused()
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
            if let Some(frame) = s.stills.take_sample(Instant::now(), cadence) {
                // Encoding, hashing and OCR happen only for the selected frame.
                let nanos = frame.nanos;
                let bytes = frame.jpeg().map_err(anyhow::Error::msg)?;
                drop(frame);
                let hash = sha256_hex(&bytes);
                if hash == last_hash {
                    continue;
                }
                let mut payload = crate::screen_text::recognize(&s.focus, bytes, &engine, || {
                    hx_macos_sys::window_text(&s.focus, 256, Duration::from_millis(150), 65536)
                })?;
                // Recognition may outlive a focus, pause, or policy change.
                let mut now = hx_macos_sys::frontmost();
                if let Some(f) = now.as_mut() {
                    f.title = hx_macos_sys::window_title(f);
                }
                if now.as_ref() != Some(&s.focus)
                    || control.is_paused()
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
                    continue;
                }
                payload["visual_sha256"] = serde_json::json!(hash);
                last_hash = hash;
                let mut e = producer.core(
                    format!("screen-text:{}", ulid::Ulid::generate()),
                    "0",
                    Operation::Observe,
                    CorePayload::TextSnapshot(serde_json::from_value(payload.clone())?),
                );
                e.payload = payload;
                e.time = frame_time(e.time, nanos);
                tx.blocking_send(Submission {
                    envelope: e,
                    reply: None,
                    focus: Some(s.focus.clone()),
                    secure: false,
                    blobs: vec![],
                })?;
                s.stills.latest = None;
                failure = None;
                send_health(
                    &mut producer,
                    &tx,
                    &live,
                    CollectorStatus::Running,
                    "Screen text captured; images are never saved.".into(),
                    0,
                )?;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(session);
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
    let last = p.last_for("text");
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
    control: CaptureSwitch,
    stop: Arc<AtomicBool>,
    live: LiveHealth,
) -> anyhow::Result<()> {
    // One failed capture or OCR operation must not end screenshots for
    // the rest of the session: report it, back off, and start a fresh capture.
    let mut backoff = RESTART_MIN;
    let mut failure = None;
    loop {
        let started = Instant::now();
        let result = run_inner(
            config.clone(),
            tx.clone(),
            context.clone(),
            control.clone(),
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
            let is_paused = control.is_paused();
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
                        "Screenshots are paused.".into()
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
#[cfg(test)]
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
    fn periodic_still_uses_retained_change_and_skips_static_window() {
        // One changed frame each second: select exactly the frame at the tick,
        // with no selections (and hence no encoding/OCR) between ticks.
        for cadence in [30, 120] {
            let start = Instant::now();
            let mut frames = StillFrames::new();
            let mut selected = Vec::new();
            for second in 0..=240 {
                frames.latest = Some(second);
                if let Some(frame) =
                    frames.take_sample(start + Duration::from_secs(second), cadence)
                {
                    selected.push(frame);
                    assert!(frames.latest.is_none(), "release the selected frame");
                }
            }
            let expected: Vec<_> = (0..=240).step_by(cadence as usize).collect();
            assert_eq!(selected, expected);
            println!(
                "cadence {cadence}s: 241 changed frames, {} selected for encoding/OCR: {selected:?}",
                selected.len()
            );
        }
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
}
