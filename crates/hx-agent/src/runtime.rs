use crate::{
    config::{Config, Profile, support_dir},
    envelope::Producer,
    outbox::Outbox,
    policy::Policy,
    upload::Uploader,
};
use anyhow::{Context, Result};
use hx_macos_sys::{Focus, NativeEvent};
use hx_schema::{kinds::*, *};
use std::{
    io::{BufRead, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct RunOptions {
    pub dry_run: bool,
    pub duration_s: Option<u64>,
    pub paused: Option<Arc<AtomicBool>>,
    pub stop: Option<Arc<AtomicBool>>,
    pub menu_bar: bool,
}
pub(crate) struct Submission {
    pub envelope: Envelope,
    pub reply: Option<mpsc::SyncSender<bool>>,
    pub focus: Option<Focus>,
    pub secure: bool,
    pub blobs: Vec<(BlobRef, Vec<u8>)>,
}
struct Companion {
    event: CompanionEvent,
    reply: mpsc::SyncSender<bool>,
}
pub fn status() -> Result<serde_json::Value> {
    let cfg = Config::load()?;
    let p = hx_macos_sys::permissions();
    let power = hx_macos_sys::power();
    let latest_health = std::fs::read(support_dir().join("health.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    Ok(
        serde_json::json!({"health":latest_health,"permissions":p,"profile":Profile::select(power,&cfg),"secure_input":hx_macos_sys::secure_input(),"collectors":{"focus":"available","presence":"available","accessibility":if p.accessibility {"available"}else {"permission_denied"},"input":if p.input {"available"}else {"permission_denied"},"media":if p.screen {"not_started"}else {"permission_denied"}}}),
    )
}
fn io_worker(
    config: Config,
    rx: tokio::sync::mpsc::Receiver<Submission>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let dir=support_dir();std::fs::create_dir_all(&dir)?;use std::os::unix::fs::PermissionsExt;std::fs::set_permissions(&dir,std::fs::Permissions::from_mode(0o700))?;
        let policy=Policy::new(&config)?;
        let mut outbox=Outbox::open(&dir.join("outbox.sqlite"),config.event_budget_bytes,config.media_budget_bytes)?;
        let mut latest_health=serde_json::Map::new();
        let mut uploader=Uploader::new(&config)?;let mut rx=rx;let mut next=tokio::time::Instant::now()+uploader.delay();
        loop {tokio::select! {
            item=rx.recv()=>match item {Some(Submission {mut envelope,reply,focus,secure,blobs})=>{
                if envelope.kind.as_str()=="collector.health" {envelope.payload["queue_age_ms"]=serde_json::json!(outbox.queue_age_ms()?);
                    if let Some(name)=envelope.payload["collector"].as_str(){latest_health.insert(name.into(),envelope.payload.clone());std::fs::write(dir.join("health.json.tmp"),serde_json::to_vec(&latest_health)?)?;std::fs::rename(dir.join("health.json.tmp"),dir.join("health.json"))?;}
                }
                let result=crate::policy::persist(&mut outbox,&policy,focus.as_ref(),secure,&envelope,&blobs);if let Some(reply)=reply {let _=reply.send(matches!(&result,Ok(true)));}
                if let Err(e)=result {eprintln!("outbox: {e}");stop.store(true,Ordering::Relaxed);return Err(e);}},None=>break},
            ()=tokio::time::sleep_until(next)=>{if !config.token.is_empty() && let Err(e)=uploader.once(&mut outbox).await {eprintln!("upload retained for retry: {e}");}next=tokio::time::Instant::now()+uploader.delay();}
        }}
        Ok(())
    })
}
fn socket_listener(
    tx: mpsc::SyncSender<Companion>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    let dir = support_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join("agent.sock");
    if path.exists() {
        anyhow::ensure!(UnixStream::connect(&path).is_err(), "agent already running");
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = socket.set_write_timeout(Some(Duration::from_secs(1)));
                    let mut reader = std::io::BufReader::new(match socket.try_clone() {
                        Ok(s) => s,
                        Err(_) => continue,
                    });
                    loop {
                        let mut line = Vec::new();
                        let result = std::io::Read::take(
                            &mut reader,
                            (crate::native_messaging::MAX_FRAME + 1) as u64,
                        )
                        .read_until(b'\n', &mut line);
                        if !matches!(result,Ok(n) if n>0) {
                            break;
                        }
                        if line.len() > crate::native_messaging::MAX_FRAME
                            || line.last() != Some(&b'\n')
                        {
                            let _ =
                                socket.write_all(b"{\"ok\":false,\"error\":\"frame too large\"}\n");
                            break;
                        }
                        let ok = serde_json::from_slice(&line).ok().is_some_and(|event| {
                            let (reply, rx) = mpsc::sync_channel(1);
                            tx.try_send(Companion { event, reply }).is_ok()
                                && rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false)
                        });
                        if writeln!(socket, "{{\"ok\":{ok}}}").is_err() {
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(path);
    }))
}
fn focus_payload(focus: &Focus, allowed: bool) -> CorePayload {
    CorePayload::FocusSpan(FocusSpan {
        app_bundle_id: if allowed {
            focus.bundle_id.clone()
        } else {
            "excluded".into()
        },
        app_name: if allowed {
            focus.name.clone()
        } else {
            "Policy gap".into()
        },
        window_title: if allowed { focus.title.clone() } else { None },
        pid: if allowed { Some(focus.pid) } else { None },
        display: None,
    })
}
fn health(
    p: &mut Producer,
    collector: &str,
    status: CollectorStatus,
    detail: Option<String>,
    dropped: u64,
) -> Envelope {
    let last = p.last_for(collector);
    p.core(
        format!("health:{collector}:{}", ulid::Ulid::generate()),
        "0",
        Operation::Observe,
        CorePayload::CollectorHealth(CollectorHealth {
            collector: collector.into(),
            status,
            last_sequence: last.map(|v| v.0),
            last_observed_at: last.map(|v| v.1),
            queue_age_ms: Some(0),
            dropped,
            detail,
        }),
    )
}
fn close_span(span: &Envelope, p: &mut Producer) -> Envelope {
    let end = p.time();
    close_span_at(span, p, end)
}
fn close_span_at(span: &Envelope, p: &mut Producer, end: ObservationTime) -> Envelope {
    let mut closed = p.core(
        span.source_key.to_string(),
        "1",
        Operation::Upsert,
        span.core_payload().expect("core span").expect("valid span"),
    );
    closed.time = span.time.clone();
    let uncertain = end
        .source_start
        .zip(span.time.source_start)
        .is_some_and(|(end, start)| end < start)
        || end
            .mono_start
            .as_ref()
            .zip(span.time.mono_start.as_ref())
            .is_some_and(|(end, start)| end.nanos < start.nanos);
    closed.time.source_end = end
        .source_start
        .map(|end| span.time.source_start.map_or(end, |start| end.max(start)));
    closed.time.mono_end = end.mono_start.map(|mut end| {
        if let Some(start) = &span.time.mono_start {
            end.nanos = end.nanos.max(start.nanos);
        }
        end
    });
    if uncertain {
        closed.coverage = Some(Coverage {
            scope: format!("{}:notification_order_uncertain", closed.kind),
            completeness: Completeness::Unknown,
        });
    }
    closed.time.observed_at = end.observed_at;
    closed
}
pub fn run(options: RunOptions) -> Result<()> {
    let cfg = Config::load()?;
    let policy = Policy::new(&cfg)?;
    let mut producer = Producer::new(cfg.device_id.clone())?;
    let main_loop = hx_macos_sys::MainLoop::new().context("capture must run on the main thread")?;
    let stop = options
        .stop
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let paused = options
        .paused
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let mut menu = if options.menu_bar {
        hx_macos_sys::MenuBar::new(paused.clone(), stop.clone())
    } else {
        None
    };
    let (tx, rx) = tokio::sync::mpsc::channel(1024);
    let worker = if options.dry_run {
        drop(rx);
        None
    } else {
        let cfg = cfg.clone();
        let stop = stop.clone();
        Some(std::thread::spawn(move || io_worker(cfg, rx, stop)))
    };
    let (companion_tx, companion_rx) = mpsc::sync_channel(128);
    let socket = if options.dry_run {
        None
    } else {
        Some(socket_listener(companion_tx, stop.clone())?)
    };
    let capture_context = Arc::new(std::sync::Mutex::new(None::<Focus>));
    let media_stop = stop.clone();
    let media = if options.dry_run {
        None
    } else {
        let tx = tx.clone();
        let cfg = cfg.clone();
        let context = capture_context.clone();
        let paused = paused.clone();
        Some(std::thread::spawn(move || {
            crate::media::run(cfg, tx, context, paused, media_stop)
        }))
    };
    let emit = |envelope: Envelope, reply: Option<mpsc::SyncSender<bool>>| {
        if options.dry_run {
            let result = (|| -> Result<()> {
                let mut out = std::io::stdout().lock();
                serde_json::to_writer(&mut out, &envelope)?;
                writeln!(out)?;
                out.flush()?;
                Ok(())
            })();
            if let Some(r) = reply {
                let _ = r.send(result.is_ok());
            }
            result
        } else {
            match tx.try_send(Submission {
                envelope,
                reply,
                focus: capture_context.lock().expect("capture context").clone(),
                secure: hx_macos_sys::secure_input(),
                blobs: vec![],
            }) {
                Ok(()) => Ok(()),
                Err(_) => {
                    anyhow::bail!("capture handoff full; stopping to avoid silent loss")
                }
            }
        }
    };
    emit(producer.calibration(), None)?;
    let mut input = hx_macos_sys::InputTap::start();
    let start = Instant::now();
    let mut last_tick = Instant::now() - Duration::from_secs(1);
    let mut last_health = Instant::now() - Duration::from_secs(60);
    let mut last_text = Instant::now();
    let mut focus: Option<Focus> = None;
    let mut focus_span: Option<Envelope> = None;
    let mut presence_span: Option<Envelope> = None;
    let mut ax_observer: Option<hx_macos_sys::AxObserver> = None;
    let mut locked = hx_macos_sys::session_locked().unwrap_or(false);
    let mut sleeping = false;
    let mut display_off = hx_macos_sys::display_asleep();
    let mut last_state = None;
    let mut last_text_hash = String::new();
    let ax = crate::accessibility::Worker::start();
    let mut ax_dropped = 0u64;
    let mut buckets = crate::input::Buckets::new(hx_macos_sys::continuous_nanos());
    let mut last_permissions = hx_macos_sys::permissions();
    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed)
            && options
                .duration_s
                .is_none_or(|s| start.elapsed() < Duration::from_secs(s))
        {
            main_loop.pump(Duration::from_millis(50));
            if let Some(menu) = &mut menu {
                menu.update();
            }
            let mut changed = ax_observer.as_ref().is_some_and(|o| o.take_changed());
            while let Some(signal) = main_loop.pop() {
                let event = signal.event;
                let mut event_time = producer.time();
                let sample_now = event_time.mono_start.as_ref().expect("native stamp").nanos;
                event_time.source_start = Some(
                    event_time.observed_at
                        - chrono::Duration::nanoseconds(
                            sample_now.saturating_sub(signal.nanos).min(i64::MAX as u64) as i64,
                        ),
                );
                event_time.mono_start.as_mut().expect("native stamp").nanos = signal.nanos;
                match event {
                    NativeEvent::Activation => {
                        changed = true;
                        if let Some(mut activated) = signal.focus {
                            if !hx_macos_sys::skip_ax(&activated) {
                                activated.title = ax.title(activated.pid);
                            }
                            if let Some(span) = focus_span.take() {
                                emit(
                                    close_span_at(&span, &mut producer, event_time.clone()),
                                    None,
                                )?;
                            }
                            let allowed =
                                policy.allows(&activated.bundle_id, activated.title.as_deref());
                            let mut span = producer.core(
                                format!("focus:{}", ulid::Ulid::generate()),
                                "0",
                                Operation::Upsert,
                                focus_payload(&activated, allowed),
                            );
                            span.time = event_time.clone();
                            if !allowed {
                                span.coverage = Some(Coverage {
                                    scope: "excluded_focus:content_suppressed".into(),
                                    completeness: Completeness::Unknown,
                                });
                            }
                            emit(span.clone(), None)?;
                            focus_span = Some(span);
                            focus = Some(activated);
                            ax_observer = focus.as_ref().and_then(hx_macos_sys::AxObserver::new);
                            last_text = Instant::now() - Duration::from_secs(3600);
                            last_text_hash.clear();
                        }
                    }
                    NativeEvent::Lock => locked = true,
                    NativeEvent::Unlock => locked = false,
                    NativeEvent::Sleep => sleeping = true,
                    NativeEvent::Wake => {
                        sleeping = false;
                        buckets.reset(hx_macos_sys::continuous_nanos());
                        emit(producer.calibration(), None)?;
                        input = hx_macos_sys::InputTap::start();
                        ax_observer = focus.as_ref().and_then(hx_macos_sys::AxObserver::new);
                        changed = true;
                    }
                    NativeEvent::DisplaySleep => display_off = true,
                    NativeEvent::DisplayWake => display_off = false,
                    _ => {}
                }
                if matches!(
                    event,
                    NativeEvent::Lock
                        | NativeEvent::Unlock
                        | NativeEvent::Sleep
                        | NativeEvent::Wake
                        | NativeEvent::DisplaySleep
                        | NativeEvent::DisplayWake
                ) {
                    let state = if sleeping {
                        DeviceState::DeviceAsleep
                    } else if locked {
                        DeviceState::Locked
                    } else if display_off {
                        DeviceState::DisplayOff
                    } else if hx_macos_sys::idle_seconds() >= cfg.idle_threshold_s as f64 {
                        DeviceState::Idle
                    } else {
                        DeviceState::Active
                    };
                    if last_state != Some(state) {
                        if let Some(span) = presence_span.take() {
                            emit(
                                close_span_at(&span, &mut producer, event_time.clone()),
                                None,
                            )?;
                        }
                        let mut span = producer.core(
                            format!("presence:{}", ulid::Ulid::generate()),
                            "0",
                            Operation::Upsert,
                            CorePayload::PresenceSpan(PresenceSpan {
                                state,
                                idle_threshold_s: Some(cfg.idle_threshold_s),
                            }),
                        );
                        span.time = event_time;
                        emit(span.clone(), None)?;
                        presence_span = Some(span);
                        last_state = Some(state);
                    }
                }
            }
            let tick = last_tick.elapsed() >= Duration::from_secs(1);
            if tick || changed {
                let mut current = hx_macos_sys::frontmost();
                if let Some(f) = current.as_mut()
                    && !hx_macos_sys::skip_ax(f)
                {
                    f.title = ax.title(f.pid);
                }
                if current != focus {
                    if let Some(span) = focus_span.take() {
                        emit(close_span(&span, &mut producer), None)?;
                    }
                    if let Some(f) = &current {
                        let allowed = policy.allows(&f.bundle_id, f.title.as_deref());
                        let mut span = producer.core(
                            format!("focus:{}", ulid::Ulid::generate()),
                            "0",
                            Operation::Upsert,
                            focus_payload(f, allowed),
                        );
                        if !allowed {
                            span.coverage = Some(Coverage {
                                scope: "excluded_focus:content_suppressed".into(),
                                completeness: Completeness::Unknown,
                            });
                        }
                        emit(span.clone(), None)?;
                        focus_span = Some(span);
                    }
                    ax_observer = current.as_ref().and_then(hx_macos_sys::AxObserver::new);
                    if let Some(observer) = &ax_observer
                        && !observer.unsupported.is_empty()
                    {
                        emit(
                            health(
                                &mut producer,
                                "accessibility",
                                CollectorStatus::Running,
                                Some(format!(
                                    "AX notification registration errors {:?}; bounded polling remains active",
                                    observer.unsupported
                                )),
                                ax_dropped,
                            ),
                            None,
                        )?;
                    }
                    focus = current;
                    *capture_context.lock().expect("capture context") = focus.clone();
                    last_text = Instant::now() - Duration::from_secs(3600);
                    last_text_hash.clear();
                }
            }
            let content_allowed = !paused.load(Ordering::Relaxed)
                && !locked
                && !sleeping
                && !display_off
                && !hx_macos_sys::secure_input()
                && focus
                    .as_ref()
                    .is_some_and(|f| policy.allows(&f.bundle_id, f.title.as_deref()));
            *capture_context.lock().expect("capture context") =
                if content_allowed { focus.clone() } else { None };
            while let Ok(c) = companion_rx.try_recv() {
                if content_allowed && policy.allows_payload(&c.event.payload) {
                    match producer.companion(c.event) {
                        Ok(e) => emit(e, Some(c.reply))?,
                        Err(_) => {
                            let _ = c.reply.send(false);
                        }
                    }
                } else {
                    let _ = c.reply.send(false);
                }
            }
            let native_allowed = content_allowed
                && focus.as_ref().is_some_and(|f| {
                    !hx_macos_sys::unverifiable_private_state(f) && !hx_macos_sys::skip_ax(f)
                });
            let profile = Profile::select(hx_macos_sys::power(), &cfg);
            if native_allowed && last_text.elapsed() >= Duration::from_secs(profile.text_interval())
            {
                last_text = Instant::now();
                if let Some(f) = &focus
                    && ax
                        .request
                        .try_send(crate::accessibility::Request::Text(f.clone()))
                        .is_err()
                {
                    ax_dropped += 1;
                }
            }
            while let Ok(sample) = ax.samples.try_recv() {
                if !content_allowed || focus.as_ref() != Some(&sample.focus) {
                    continue;
                }
                let payload = match sample.observation {
                    crate::accessibility::Observation::Text(text) => {
                        let hash = sha256_hex(text.text.as_bytes());
                        if hash == last_text_hash {
                            continue;
                        }
                        last_text_hash = hash.clone();
                        CorePayload::TextSnapshot(TextSnapshot {
                            surface: sample.focus.bundle_id.clone(),
                            source: TextSource::Accessibility,
                            text: text.text,
                            text_sha256: hash,
                            completeness: if text.truncated {
                                Completeness::Truncated
                            } else {
                                Completeness::Partial
                            },
                            visible: None,
                        })
                    }
                    crate::accessibility::Observation::Click(hit, lag) => {
                        CorePayload::UiClick(UiClick {
                            app_bundle_id: sample.focus.bundle_id.clone(),
                            role: hit.role,
                            label: hit.label,
                            sample_lag_ms: lag,
                        })
                    }
                };
                let mut e = producer.core(
                    format!("ax:{}", ulid::Ulid::generate()),
                    "0",
                    Operation::Observe,
                    payload,
                );
                e.time.source_start = Some(sample.observed_at);
                e.time.observed_at = sample.observed_at;
                e.time.mono_start.as_mut().expect("native stamp").nanos = sample.nanos;
                emit(e, None)?;
            }
            if let Some(tap) = &input {
                while let Some(event) = tap.pop() {
                    buckets.push(event);
                    if matches!(event.kind, 1 | 3 | 25)
                        && content_allowed
                        && let Some(f) = &focus
                        && ax
                            .request
                            .try_send(crate::accessibility::Request::Click(f.clone(), event))
                            .is_err()
                    {
                        ax_dropped += 1;
                    }
                }
            }
            if tick {
                last_tick = Instant::now();
                let state = if sleeping {
                    DeviceState::DeviceAsleep
                } else if locked {
                    DeviceState::Locked
                } else if display_off {
                    DeviceState::DisplayOff
                } else if hx_macos_sys::idle_seconds() >= cfg.idle_threshold_s as f64 {
                    DeviceState::Idle
                } else {
                    DeviceState::Active
                };
                if last_state != Some(state) {
                    if let Some(span) = presence_span.take() {
                        emit(close_span(&span, &mut producer), None)?;
                    }
                    let span = producer.core(
                        format!("presence:{}", ulid::Ulid::generate()),
                        "0",
                        Operation::Upsert,
                        CorePayload::PresenceSpan(PresenceSpan {
                            state,
                            idle_threshold_s: Some(cfg.idle_threshold_s),
                        }),
                    );
                    emit(span.clone(), None)?;
                    presence_span = Some(span);
                    last_state = Some(state);
                }
                if input.is_some() {
                    let now = producer.time();
                    let now_nanos = now.mono_start.as_ref().expect("native clock").nanos;
                    for (start, bucket) in buckets.complete(now_nanos) {
                        let mut e = producer.core(
                            format!(
                                "input:{}:{start}",
                                now.mono_start.as_ref().expect("native clock").boot_id
                            ),
                            "0",
                            Operation::Observe,
                            CorePayload::InputBucket(bucket),
                        );
                        e.time.source_start = Some(
                            now.observed_at
                                - chrono::Duration::nanoseconds((now_nanos - start) as i64),
                        );
                        e.time.source_end = e
                            .time
                            .source_start
                            .map(|s| s + chrono::Duration::seconds(1));
                        let mut stamp = now.mono_start.clone().expect("native clock");
                        stamp.nanos = start;
                        e.time.mono_start = Some(stamp.clone());
                        stamp.nanos += 1_000_000_000;
                        e.time.mono_end = Some(stamp);
                        emit(e, None)?;
                    }
                }
            }
            let permissions = hx_macos_sys::permissions();
            let permission_change = permissions.accessibility != last_permissions.accessibility
                || permissions.input != last_permissions.input
                || permissions.screen != last_permissions.screen;
            if permission_change {
                input = hx_macos_sys::InputTap::start();
                last_permissions = permissions;
            }
            if last_health.elapsed() >= Duration::from_secs(60) || permission_change {
                last_health = Instant::now();
                for (collector, granted, implemented) in [
                    ("focus", true, true),
                    ("presence", true, true),
                    ("accessibility", permissions.accessibility, true),
                    ("input", permissions.input, input.is_some()),
                    ("media", permissions.screen, !options.dry_run),
                ] {
                    if collector == "media" && !options.dry_run {
                        continue;
                    }
                    let status = if !granted {
                        CollectorStatus::PermissionDenied
                    } else if !implemented {
                        CollectorStatus::Failed
                    } else if ["accessibility", "media"].contains(&collector) && !native_allowed {
                        CollectorStatus::Paused
                    } else {
                        CollectorStatus::Running
                    };
                    let detail = if !granted {
                        Some("PermissionDenied: use explicit onboard command".into())
                    } else if !implemented {
                        Some("collector unavailable; no coverage claimed".into())
                    } else if status == CollectorStatus::Paused {
                        Some(
                            "content suppressed by policy, pause, device state or Secure Input"
                                .into(),
                        )
                    } else {
                        None
                    };
                    emit(
                        health(
                            &mut producer,
                            collector,
                            status,
                            detail,
                            if ["focus", "presence"].contains(&collector) {
                                main_loop.dropped()
                            } else if collector == "accessibility" {
                                ax_dropped + ax.dropped.load(Ordering::Relaxed)
                            } else if collector == "input" {
                                input.as_ref().map_or(0, |t| t.dropped()) + buckets.dropped
                            } else {
                                0
                            },
                        ),
                        None,
                    )?;
                }
            }
        }
        if let Some(span) = focus_span.take() {
            emit(close_span(&span, &mut producer), None)?;
        }
        if let Some(span) = presence_span.take() {
            emit(close_span(&span, &mut producer), None)?;
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    drop(tx);
    drop(input);
    if let Some(t) = socket {
        let _ = t.join();
    }
    if let Some(t) = media {
        let _ = t.join();
    }
    if let Some(t) = worker {
        t.join()
            .map_err(|_| anyhow::anyhow!("I/O worker panicked"))??;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn open_span_revision_closes_in_native_clock_domain() {
        let mut p = Producer::new("test".into()).unwrap();
        let mut span = p.core(
            "focus:stable",
            "0",
            Operation::Upsert,
            CorePayload::PresenceSpan(PresenceSpan {
                state: DeviceState::Active,
                idle_threshold_s: Some(300),
            }),
        );
        span.time.mono_start.as_mut().unwrap().nanos = 100;
        let mut end = span.time.clone();
        end.mono_start.as_mut().unwrap().nanos = 250;
        end.source_start = end.source_start.map(|t| t + chrono::Duration::seconds(1));
        let closed = close_span_at(&span, &mut p, end);
        assert!(span.time.duration().is_none());
        assert_eq!(closed.source_key, span.source_key);
        assert_eq!(closed.revision.as_str(), "1");
        assert_eq!(closed.time.duration().unwrap().nanos, 150);
        closed.validate().unwrap();
    }
    #[test]
    fn late_notifications_are_marked_uncertain_instead_of_negative_spans() {
        let mut p = Producer::new("test".into()).unwrap();
        let span = p.core(
            "presence:stable",
            "0",
            Operation::Upsert,
            CorePayload::PresenceSpan(PresenceSpan {
                state: DeviceState::Idle,
                idle_threshold_s: Some(300),
            }),
        );
        let mut end = span.time.clone();
        end.source_start = end.source_start.map(|t| t - chrono::Duration::seconds(1));
        end.mono_start.as_mut().unwrap().nanos -= 1;
        let closed = close_span_at(&span, &mut p, end);
        closed.validate().unwrap();
        assert_eq!(closed.time.duration().unwrap().nanos, 0);
        assert_eq!(closed.coverage.unwrap().completeness, Completeness::Unknown);
    }
}
