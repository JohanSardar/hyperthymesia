//! Synthetic-only entrypoint: no capture, companion sockets, or login supervision.
use crate::{
    config::{Config, support_dir},
    envelope::Producer,
    runtime::{Submission, io_worker_at},
};
use anyhow::{Context, Result, ensure};
use clap::Parser;
use hx_schema::{kinds::*, *};
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Options {
    #[arg(long, default_value_os_t = support_dir())]
    support_dir: PathBuf,
    /// A JPEG containing synthetic data only. Never reads the screen.
    #[arg(long)]
    seed_jpeg: Option<PathBuf>,
    #[arg(long)]
    duration_s: Option<u64>,
    /// Exercise the real outbox and uploader without AppKit (automated tests).
    #[arg(long)]
    headless: bool,
}

pub fn main() -> Result<()> {
    let options = Options::parse();
    let cfg = Config::load_at(&options.support_dir)?;
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let main_loop = if options.headless {
        None
    } else {
        Some(hx_macos_sys::MainLoop::new().context("harness must run on main thread")?)
    };
    let mut menu = if options.headless {
        None
    } else {
        hx_macos_sys::MenuBar::new(paused, stop.clone(), quit)
    };
    if let Some(menu) = &mut menu {
        menu.disable_permissions();
    }
    let mut onboarding = crate::first_run::Onboarding::new(options.support_dir.clone());
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let dir = options.support_dir;
    let worker_cfg = cfg.clone();
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || io_worker_at(worker_cfg, rx, worker_stop, dir));
    let result = (|| -> Result<()> {
        if let Some(path) = options.seed_jpeg {
            let data = std::fs::read(path)?;
            let (width, height) = image::ImageReader::with_format(
                std::io::Cursor::new(&data),
                image::ImageFormat::Jpeg,
            )
            .into_dimensions()?;
            let blob = BlobRef {
                sha256: sha256_hex(&data),
                media_type: "image/jpeg".into(),
                bytes: data.len() as u64,
            };
            let mut producer = Producer::new(cfg.device_id)?.namespace("macos-media");
            let mut envelope = producer.core(
                format!("synthetic:{}", ulid::Ulid::generate()),
                "0",
                Operation::Observe,
                CorePayload::MediaStill(MediaStill {
                    display: "synthetic".into(),
                    trigger: StillTrigger::FocusChange,
                    width,
                    height,
                    app_bundle_id: Some("com.hyperthymesia.agent.e2e".into()),
                    window_title: Some("Synthetic fixture".into()),
                }),
            );
            envelope.blobs.push(blob.clone());
            let (reply, ack) = mpsc::sync_channel(1);
            tx.blocking_send(Submission {
                envelope,
                reply: Some(reply),
                focus: Some(hx_macos_sys::Focus {
                    bundle_id: "com.hyperthymesia.agent.e2e".into(),
                    name: "Synthetic".into(),
                    pid: 0,
                    title: Some("Synthetic fixture".into()),
                    electron: false,
                }),
                secure: false,
                blobs: vec![(blob, data)],
            })?;
            ensure!(
                ack.recv_timeout(Duration::from_secs(10))?,
                "synthetic JPEG was not queued"
            );
            println!("Synthetic JPEG queued.");
            std::io::stdout().flush()?;
        }
        let start = Instant::now();
        while !stop.load(Ordering::Relaxed)
            && options
                .duration_s
                .is_none_or(|s| start.elapsed() < Duration::from_secs(s))
        {
            ensure!(!worker.is_finished(), "harness I/O worker stopped");
            if let Some(main_loop) = &main_loop {
                main_loop.pump(Duration::from_millis(50));
            } else {
                std::thread::sleep(Duration::from_millis(50));
            }
            if let Some(menu) = &mut menu {
                onboarding.update(menu, None);
            }
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    drop(tx);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("harness worker panicked"))??;
    result
}
