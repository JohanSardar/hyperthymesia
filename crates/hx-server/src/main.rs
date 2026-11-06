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
    /// Snapshot both databases and published blobs while the server stays online.
    Backup {
        #[arg(long)]
        to: PathBuf,
    },
    /// Verify a backup and restore it into an absent or empty state directory.
    Restore {
        #[arg(long)]
        from: PathBuf,
    },
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
    /// Print a single-use pairing link, valid for 15 minutes.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Pair {
        #[command(subcommand)]
        command: Option<PairCommand>,
        #[arg(long, default_value = "http://127.0.0.1:7377")]
        url: String,
        #[arg(long, required = true)]
        name: Option<String>,
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
enum PairCommand {
    /// List code hash IDs and metadata, never the pairing secrets.
    List,
    /// Revoke a full code ID from pair list. Does not revoke an issued token.
    Revoke { id: String },
}
#[derive(Subcommand)]
enum TokenCommand {
    /// List token IDs, names, scopes, prefixes and revocation state (no secrets).
    List,
    /// Revoke the full token ID printed by token list. Effective next request.
    Revoke { id: String },
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
    // Neither command opens a writable source pool. Restore must work before
    // core.sqlite exists; backup reads WAL snapshots without taking the writer.
    match &cli.command {
        Command::Backup { to } => return hx_store::backup::backup(&dir, to).await,
        Command::Restore { from } => return hx_store::backup::restore(from, &dir).await,
        _ => {}
    }
    let store = Store::open(&dir).await?;
    match cli.command {
        Command::Doctor => unreachable!("doctor runs without a database"),
        Command::Migrate => unreachable!("migrate creates a new state"),
        Command::Backup { .. } | Command::Restore { .. } => {
            unreachable!("backup and restore run without opening the store")
        }
        Command::Token {
            command:
                TokenCommand::Create {
                    name,
                    prefix,
                    scopes,
                },
        } => println!("{}", store.create_token(&name, prefix, scopes).await?),
        Command::Token {
            command: TokenCommand::List,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.list_tokens().await?)?
            );
        }
        Command::Token {
            command: TokenCommand::Revoke { id },
        } => {
            if !store.revoke_token(&id).await? {
                bail!("token ID not found");
            }
            println!("Token revoked.");
        }
        Command::Pair {
            command: Some(PairCommand::List),
            ..
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.list_pair_codes().await?)?
            );
        }
        Command::Pair {
            command: Some(PairCommand::Revoke { id }),
            ..
        } => {
            if !store.revoke_pair_code(&id).await? {
                bail!("code ID not found");
            }
            println!("Pairing code revoked.");
        }
        Command::Pair {
            command: None,
            url,
            name,
        } => {
            let server =
                url::Url::parse(&url).map_err(|_| anyhow::anyhow!("invalid server URL"))?;
            let loopback = server.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            if server.host().is_none()
                || !server.username().is_empty()
                || server.password().is_some()
                || server.query().is_some()
                || server.fragment().is_some()
                || !(server.scheme() == "https" || (server.scheme() == "http" && loopback))
            {
                bail!(
                    "pairing requires a server URL with HTTPS (or loopback HTTP), without credentials, query or fragment"
                );
            }
            let code = store
                .create_pair_code(&name.context("--name is required")?)
                .await?;
            let mut link = url::Url::parse("hyperthymesia://pair")?;
            link.query_pairs_mut()
                .append_pair("server", server.as_str().trim_end_matches('/'))
                .append_pair("code", &code);
            println!("{link}");
        }
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
            let ocr_worker = if cfg!(target_os = "macos") {
                let store = store.clone();
                Some(tokio::spawn(async move {
                    if hx_server::ocr::worker(store).await.is_err() {
                        tracing::error!("OCR worker unavailable");
                    }
                }))
            } else {
                tracing::warn!(
                    "OCR requires a macOS server; pending stills are retained for backfill"
                );
                None
            };
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
            let stop_ocr = ocr_worker.as_ref().map(|worker| worker.abort_handle());
            let result = axum::serve(listener, hx_server::router(store, policy))
                .with_graceful_shutdown(async move {
                    shutdown.await;
                    // Cancel provider work before waiting for HTTP connections
                    // to drain. Dropping fx's output future kills its child.
                    stop_worker.abort();
                    if let Some(worker) = stop_ocr {
                        worker.abort();
                    }
                })
                .await;
            worker.abort();
            let _ = worker.await;
            if let Some(worker) = ocr_worker {
                worker.abort();
                let _ = worker.await;
            }
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
