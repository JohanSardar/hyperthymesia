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
    /// Check state privacy/schema and provider readiness without calling a model.
    Doctor {
        /// Check only state/schema/privacy; provider login may still be missing.
        #[arg(long)]
        state_only: bool,
        /// Installer preflight: accept missing migrations, never changed/newer ones.
        #[arg(long, requires = "state_only")]
        allow_older_schema: bool,
    },
    /// Read schema compatibility and the number of live interpret leases.
    Status {
        #[arg(long)]
        require_idle: bool,
    },
    /// Hold the service worker idle throughout an installer lifecycle.
    #[command(hide = true)]
    UpgradeGuard {
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<std::ffi::OsString>,
    },
    /// Resume interrupted installer rollback before a managed service starts.
    #[command(hide = true)]
    UpgradeRecover {
        #[arg(long)]
        lifecycle: PathBuf,
        /// Do not perform recovery; only let a guarded candidate start.
        #[arg(long)]
        maintenance_only: bool,
    },
    /// Atomically replace an installer symlink without platform-specific mv flags.
    #[command(hide = true)]
    InstallLink {
        #[arg(long)]
        link: PathBuf,
        #[arg(long)]
        target: PathBuf,
    },
    /// Atomically publish the install-prefix marker without following its target.
    #[command(hide = true)]
    InstallMarker {
        #[arg(long)]
        staged: PathBuf,
        #[arg(long)]
        target: PathBuf,
    },
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
    /// Disabled: deleting old media is not yet safe for referenced evidence.
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
    if let Command::InstallLink { link, target } = &cli.command {
        if let Ok(metadata) = std::fs::symlink_metadata(link) {
            anyhow::ensure!(
                metadata.file_type().is_symlink(),
                "installer link is not a symlink"
            );
        }
        let parent = link.parent().context("installer link parent")?;
        let stage = tempfile::Builder::new()
            .prefix(".switch.")
            .tempdir_in(parent)?;
        let next = stage.path().join("current");
        std::os::unix::fs::symlink(target, &next)?;
        std::fs::rename(&next, link)?;
        std::fs::File::open(parent)?.sync_all()?;
        return Ok(());
    }
    if let Command::InstallMarker { staged, target } = &cli.command {
        let parent = target.parent().context("installer marker parent")?;
        anyhow::ensure!(
            staged.parent() == Some(parent)
                && target
                    .file_name()
                    .is_some_and(|name| name == ".install-prefix")
                && staged
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".install-prefix.")),
            "invalid installer marker path"
        );
        anyhow::ensure!(
            std::fs::symlink_metadata(staged)?.file_type().is_file(),
            "installer marker staging path must be a regular file"
        );
        match std::fs::symlink_metadata(target) {
            Ok(metadata) => anyhow::ensure!(
                metadata.file_type().is_file(),
                "installer marker target must be a regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::fs::File::open(staged)?.sync_all()?;
        std::fs::rename(staged, target)?;
        std::fs::File::open(parent)?.sync_all()?;
        return Ok(());
    }
    if let Command::Doctor {
        state_only,
        allow_older_schema,
    } = &cli.command
    {
        if let Some(dir) = &cli.state_dir {
            if *allow_older_schema {
                Store::check_upgrade_state(dir).await?;
            } else {
                Store::check_state(dir).await?;
            }
            println!("ok: private state, compatible schema and SQLite integrity");
        } else if *state_only {
            bail!("--state-dir or HX_STATE_DIR is required for --state-only");
        }
        if *state_only {
            return Ok(());
        }
        if matches!(ProviderConfig::from_env(), ProviderConfig::Stub) {
            bail!("HX_PROVIDER selects the stub; set HX_PROVIDER=fx for real interpretation");
        }
        hx_server::fx::doctor(&hx_server::fx::Config::from_env()).await?;
        println!("ok: provider configured, fx version and login checks passed (no model call)");
        return Ok(());
    }
    if matches!(cli.command, Command::Gc) {
        bail!(
            "gc is disabled: old media can still be referenced by evidence. No files were deleted; see docs/gc-safety.md."
        );
    }
    let dir = cli
        .state_dir
        .context("--state-dir or HX_STATE_DIR is required")?;
    if matches!(cli.command, Command::Migrate) {
        let store = Store::migrate(&dir).await?;
        store.pool.close().await;
        return Ok(());
    }
    if let Command::Status { require_idle } = &cli.command {
        let leases = Store::status(&dir).await?;
        println!(
            "{}",
            serde_json::json!({"running_interpret_leases":leases, "retained": retained_copies(&dir)})
        );
        if *require_idle && leases != 0 {
            bail!("upgrade refused: running interpret lease");
        }
        return Ok(());
    }
    if let Command::UpgradeRecover {
        lifecycle,
        maintenance_only,
    } = &cli.command
    {
        // The hard link keeps the original guard inode reachable even if
        // rollback has already moved the state directory aside. Never create
        // empty replacement state just to acquire a recovery lock.
        let lock = std::fs::OpenOptions::new().read(true).write(true).open(
            lifecycle
                .parent()
                .context("recovery parent")?
                .join("guard.lock"),
        )?;
        match lock.try_lock() {
            Ok(()) => {
                if *maintenance_only {
                    bail!(
                        "upgrade recovery is pending; rerun the installer after stopping the server"
                    );
                }
                let status = tokio::process::Command::new("/bin/sh")
                    .arg(lifecycle)
                    .arg("--recover")
                    .stdin(lock.try_clone()?)
                    .status()
                    .await?;
                anyhow::ensure!(status.success(), "interrupted upgrade recovery failed");
            }
            // Health must start while the installer verifies the candidate.
            // Admission also checks the durable pending record, not just this
            // lock: losing the installer must not admit rollback-bound writes.
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }
    if let Command::UpgradeGuard { command } = &cli.command {
        let lock = worker_lock(&dir)?;
        // Idle ticks and in-flight uploads briefly hold a shared lock too.
        // Wait for those to drain, but never wait out a paid interpret lease.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if Store::status(&dir).await? != 0 {
                bail!("upgrade refused: running interpret lease");
            }
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock)
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(error) => {
                    return Err(error)
                        .context("upgrade refused: worker is active; retry when idle");
                }
            }
        }
        if Store::status(&dir).await? != 0 {
            bail!("upgrade refused: running interpret lease");
        }
        let recovery = std::env::var_os("HX_INSTALL_RECOVERY").map(PathBuf::from);
        if let Some(path) = &recovery {
            // The state volume may differ from the executable prefix, and may
            // be mounted noexec. Only this non-executable anchor belongs there.
            let anchor = dir.with_file_name(format!(".hx-upgrade-lock-{}", ulid::Ulid::generate()));
            std::fs::hard_link(dir.join("locks/upgrade.lock"), &anchor)?;
            if let Err(error) = std::os::unix::fs::symlink(
                &anchor,
                path.parent().context("recovery parent")?.join("guard.lock"),
            ) {
                let _ = std::fs::remove_file(anchor);
                return Err(error.into());
            }
        }
        let shutdown = shutdown_signal()?;
        let mut lifecycle = tokio::process::Command::new(&command[0]);
        lifecycle.args(&command[1..]).process_group(0);
        if recovery.is_some() {
            // The installer does not use stdin. Keep the same open-file-
            // description lock in it too, so killing this supervisor cannot
            // admit jobs. Other guarded commands keep their ordinary stdin.
            lifecycle.stdin(lock.try_clone()?);
        }
        let mut child = lifecycle.spawn()?;
        let group = format!("-{}", child.id().context("installer child PID")?);
        let status = tokio::select! {
            status = child.wait() => status?,
            _ = shutdown => {
                tokio::process::Command::new("/bin/kill").args(["-TERM", "--", &group]).status().await?;
                child.wait().await?
            }
        };
        // SIGKILL cannot run the shell's EXIT trap. The outer supervisor uses
        // its durable progress record to finish the same idempotent rollback.
        if status.code().is_none()
            && let Some(path) = recovery
        {
            // A killed shell may leave a migration/copy child running. Stop
            // that entire isolated group before restoring either database.
            let _ = tokio::process::Command::new("/bin/kill")
                .args(["-KILL", "--", &group])
                .stderr(std::process::Stdio::null())
                .status()
                .await;
            let recovered = tokio::process::Command::new("/bin/sh")
                .arg(path)
                .arg("--recover")
                .stdin(lock.try_clone()?)
                .status()
                .await?;
            anyhow::ensure!(recovered.success(), "killed upgrade recovery failed");
        }
        // Keep the advisory lock until the lifecycle (including rollback) exits.
        drop(lock);
        if !status.success() {
            bail!("installer lifecycle failed: {status}");
        }
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
        Command::Doctor { .. }
        | Command::Status { .. }
        | Command::UpgradeGuard { .. }
        | Command::UpgradeRecover { .. }
        | Command::InstallLink { .. }
        | Command::InstallMarker { .. } => {
            unreachable!("readiness commands run before opening the store")
        }
        Command::Migrate => unreachable!("migrate runs before opening the store"),
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
        Command::Gc => unreachable!("gc is refused before opening the store"),
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
            if cfg!(target_os = "macos") && ocr_mode == hx_schema::protocol::OcrMode::Device {
                // Error level: the default filter hides anything lower.
                tracing::error!(
                    "Vision helper not found on this Mac: this server will not run OCR and accepts device results instead"
                );
            }
            let ocr_store = store.clone();
            let ocr_dir = dir.clone();
            let ocr_worker = tokio::spawn(async move {
                // A newly started service must not publish derived state
                // during the installer's verification/rollback window.
                let Ok(lock) = worker_lock(&ocr_dir) else {
                    return;
                };
                loop {
                    match lock.try_lock_shared() {
                        Ok(()) => {
                            if !upgrade_pending(&ocr_dir).unwrap_or(true) {
                                break;
                            }
                            let _ = lock.unlock();
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                        Err(std::fs::TryLockError::WouldBlock) => {
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await
                        }
                        Err(e) => {
                            tracing::error!("OCR startup lock failed: {e}");
                            return;
                        }
                    }
                }
                drop(lock);
                if hx_server::ocr::worker_in_mode(ocr_store, ocr_mode)
                    .await
                    .is_err()
                {
                    tracing::error!("OCR worker unavailable");
                }
            });
            let worker_lock = worker_lock(&dir)?;
            let worker_dir = dir.clone();
            let worker = tokio::spawn(async move {
                let mut next_schedule = tokio::time::Instant::now();
                loop {
                    // Shared across the scheduler/claim/provider future. An
                    // upgrade's exclusive lock prevents a check/stop claim race.
                    let guard = match worker_lock.try_lock_shared() {
                        Ok(()) => Some(&worker_lock),
                        Err(std::fs::TryLockError::WouldBlock) => None,
                        Err(e) => {
                            tracing::error!("worker lock failed: {e}");
                            return;
                        }
                    };
                    let Some(guard) = guard else {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        continue;
                    };
                    if upgrade_pending(&worker_dir).unwrap_or(true) {
                        let _ = guard.unlock();
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        continue;
                    }
                    if tokio::time::Instant::now() >= next_schedule {
                        next_schedule =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(60);
                        if let Err(e) = processing::schedule(&runner.store).await {
                            tracing::error!("scheduler failed: {e:#}");
                        }
                    }
                    match hx_jobs::run_one(&runner.store, &runner).await {
                        Ok(true) => {
                            let _ = guard.unlock();
                            continue;
                        }
                        Err(e) => tracing::error!("processing failed: {e:#}"),
                        Ok(false) => {}
                    }
                    let _ = guard.unlock();
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            });
            let stop_worker = worker.abort_handle();
            let stop_ocr = ocr_worker.abort_handle();
            let router = hx_server::router(store, policy)
                .layer(axum::Extension(ocr_mode))
                .layer(axum::middleware::from_fn_with_state(
                    dir.clone(),
                    maintenance_gate,
                ));
            let interrupted_upgrade = wait_for_interrupted_upgrade(dir.clone());
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    tokio::select! {
                        _ = shutdown => {},
                        _ = interrupted_upgrade => {},
                    }
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

fn worker_lock(dir: &std::path::Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("locks/upgrade.lock"))?)
}

/// A dropped advisory lock is not a commit. A pending record fences admission
/// until the installer removes it, even if the complete installer tree dies.
/// Treat unreadable/malformed managed metadata as blocked at every caller.
fn upgrade_pending(dir: &std::path::Path) -> Result<bool> {
    let prefix = match std::fs::read_to_string(dir.join(".install-prefix")) {
        Ok(prefix) => PathBuf::from(prefix.trim_end_matches('\n')),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(prefix.is_absolute(), "invalid installer prefix");
    anyhow::ensure!(
        prefix.join(".installed").metadata()?.is_file(),
        "installer marker missing"
    );
    match std::fs::symlink_metadata(prefix.join("pending-upgrade")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Recovery must not rename a live SQLite pool. Once an uncommitted installer
/// disappears, shut down the candidate so its manager runs the recovery wrapper
/// before reopening state. Until then all requests/workers remain fenced.
async fn wait_for_interrupted_upgrade(dir: PathBuf) {
    let Ok(lock) = worker_lock(&dir) else {
        return;
    };
    loop {
        if upgrade_pending(&dir).unwrap_or(true) {
            match lock.try_lock_shared() {
                Ok(()) => {
                    // The installer may have committed between the first check
                    // and lock acquisition. Recheck under the admission lock.
                    let pending = upgrade_pending(&dir).unwrap_or(true);
                    let _ = lock.unlock();
                    if pending {
                        tracing::error!(
                            "upgrade admission blocked without an installer; restarting for recovery"
                        );
                        return;
                    }
                }
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(_) => return,
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Keep uploads retryable during an installer rollback window. The shared lock
/// fences in-flight requests before an upgrade can take its exclusive lock;
/// the durable pending record keeps them fenced after a killed installer.
async fn maintenance_gate(
    axum::extract::State(dir): axum::extract::State<PathBuf>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }
    let Ok(lock) = worker_lock(&dir) else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if lock.try_lock_shared().is_err() || upgrade_pending(&dir).unwrap_or(true) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "upgrade in progress; retry later",
        )
            .into_response();
    }
    let response = next.run(request).await;
    drop(lock);
    response
}

// Metadata only; never follow links or inspect backup/secret contents. Errors
// are reported per copy, not turned into an upgrade refusal.
fn retained_copies(state: &std::path::Path) -> Vec<serde_json::Value> {
    fn size(path: &std::path::Path) -> std::io::Result<u64> {
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_dir() {
            return Ok(meta.len());
        }
        let mut bytes = 0;
        for entry in std::fs::read_dir(path)? {
            bytes += size(&entry?.path())?;
        }
        Ok(bytes)
    }
    let mut paths = Vec::new();
    if let (Some(parent), Some(name)) = (state.parent(), state.file_name()) {
        let start = format!("{}.failed-", name.to_string_lossy());
        if let Ok(entries) = std::fs::read_dir(parent) {
            paths.extend(
                entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&start))
                    .map(|e| e.path()),
            );
        }
    }
    // Written by the installer, so custom state paths also find their backups.
    if let Ok(prefix) = std::fs::read_to_string(state.join(".install-prefix")) {
        let prefix = PathBuf::from(prefix.trim_end());
        if prefix.is_absolute()
            && prefix.join(".installed").is_file()
            && let Ok(entries) = std::fs::read_dir(prefix.join("backups"))
        {
            paths.extend(entries.flatten().map(|e| e.path()));
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| match size(&path) {
            Ok(bytes) => serde_json::json!({"path":path,"bytes":bytes}),
            Err(error) => serde_json::json!({"path":path,"error":error.to_string()}),
        })
        .collect()
}
