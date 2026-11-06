use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use hx_schema::Interval;
use hx_server::processing::{self, ProviderConfig, Runner};
use hx_store::Store;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Local state folder; required for every command except doctor and version.
    #[arg(long, env = "HX_STATE_DIR", global = true)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Migrate,
    /// Check provider readiness without connecting to the database or calling a model.
    Doctor,
    Serve {
        #[arg(long, default_value = "127.0.0.1:7377")]
        listen: SocketAddr,
    },
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    Compile {
        #[arg(long)]
        from: DateTime<Utc>,
        #[arg(long)]
        to: DateTime<Utc>,
    },
    Interpret {
        #[arg(long)]
        from: DateTime<Utc>,
        #[arg(long)]
        to: DateTime<Utc>,
    },
    Gc,
}
#[derive(Subcommand)]
enum TokenCommand {
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, required = true)]
        prefix: Vec<String>,
        #[arg(long, value_delimiter = ',', required = true)]
        scopes: Vec<String>,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    if matches!(cli.command, Command::Doctor) {
        if matches!(ProviderConfig::from_env(), ProviderConfig::Stub) {
            bail!("HX_PROVIDER selects the stub; set HX_PROVIDER=fx for real interpretation");
        }
        hx_server::fx::doctor(&hx_server::fx::Config::from_env()).await?;
        println!("ok: provider configured, fx version and login checks passed (no model call)");
        return Ok(());
    }
    let dir = cli
        .state_dir
        .context("--state-dir or HX_STATE_DIR is required")?;
    if matches!(cli.command, Command::Migrate) {
        let store = Store::create(&dir).await?;
        store.pool.close().await;
        return Ok(());
    }
    let store = Store::open(&dir).await?;
    match cli.command {
        Command::Doctor => unreachable!("doctor runs without a database"),
        Command::Migrate => unreachable!("migrate creates a new state"),
        Command::Token {
            command:
                TokenCommand::Create {
                    name,
                    prefix,
                    scopes,
                },
        } => println!("{}", store.create_token(&name, prefix, scopes).await?),
        Command::Gc => println!("Expired {} blobs", store.gc().await?),
        Command::Compile { from, to, .. } => {
            for w in windows(from, to)? {
                processing::compile_window(&store, w).await?;
            }
        }
        Command::Interpret { from, to } => {
            for w in windows(from, to)? {
                processing::interpret_window(
                    &store,
                    w,
                    &processing::coordinator_provider(ProviderConfig::from_env()),
                )
                .await?;
            }
        }
        Command::Serve { listen, .. } => {
            if !listen.ip().is_loopback() {
                bail!("listen must be loopback; use a local Tailscale proxy");
            }
            let policy = hx_server::disclosure::DisclosurePolicy::from_env()?;
            let shutdown = shutdown_signal()?;
            let listener = tokio::net::TcpListener::bind(listen).await?;
            let provider = ProviderConfig::from_env();
            if matches!(provider, ProviderConfig::Stub) {
                eprintln!(
                    "WARNING: HX_PROVIDER selects the stub; no real interpretation will run. Set HX_PROVIDER=fx and run hx-server doctor."
                );
            }
            // See docs/gc-safety.md: new records don't protect old hashes, and the
            // first run's deletion of old chunks is the owner's decision.
            eprintln!(
                "WARNING: automatic blob GC is off; disk usage will grow. See docs/gc-safety.md."
            );
            let runner = Runner {
                store: store.clone(),
                provider: processing::coordinator_provider(provider),
            };
            let ocr_mode = hx_server::ocr::mode().await?;
            let ocr_store = store.clone();
            let ocr_worker = tokio::spawn(async move {
                if hx_server::ocr::worker_in_mode(ocr_store, ocr_mode)
                    .await
                    .is_err()
                {
                    tracing::error!("OCR worker unavailable");
                }
            });
            let worker = tokio::spawn(async move {
                let mut next_schedule = tokio::time::Instant::now();
                loop {
                    if tokio::time::Instant::now() >= next_schedule {
                        next_schedule =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(60);
                        if let Err(e) = processing::schedule(&runner.store).await {
                            tracing::error!("scheduler failed: {e:#}");
                        }
                    }
                    match hx_jobs::run_one(&runner.store, &runner).await {
                        Ok(true) => continue,
                        Err(e) => tracing::error!("processing failed: {e:#}"),
                        Ok(false) => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            });
            let stop_worker = worker.abort_handle();
            let stop_ocr = ocr_worker.abort_handle();
            let result = axum::serve(
                listener,
                hx_server::router(store, policy).layer(axum::Extension(ocr_mode)),
            )
            .with_graceful_shutdown(async move {
                shutdown.await;
                // Cancel provider work before waiting for HTTP connections
                // to drain. Dropping fx's output future kills its child.
                stop_worker.abort();
                stop_ocr.abort();
            })
            .await;
            worker.abort();
            let _ = worker.await;
            ocr_worker.abort();
            let _ = ocr_worker.await;
            result?;
        }
    }
    Ok(())
}
fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    // Register SIGTERM before starting workers; service managers don't send Ctrl-C.
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    })
}
fn windows(from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Interval>> {
    if to <= from || to - from > chrono::Duration::days(366) {
        bail!("invalid range");
    }
    let mut result = vec![];
    let mut w = hx_store::window_at(from);
    while w.start < to {
        result.push(w);
        w = hx_store::window_at(w.end);
    }
    Ok(result)
}
