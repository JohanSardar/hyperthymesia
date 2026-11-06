//! Owner-run, verified, one-way cutover. No production API writes to Postgres.
pub mod core;
pub mod postgres;
pub mod schema;
mod verification;

use anyhow::{Result, ensure};
use hx_store::Store;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub struct Options {
    pub postgres_url: String,
    /// The legacy data directory containing blobs/.
    pub source_data_dir: PathBuf,
    /// Must be absent or an empty directory on a local filesystem.
    pub state_dir: PathBuf,
    pub verify_all_blobs: bool,
}
#[derive(Serialize, Deserialize)]
pub struct Report {
    pub tables: core::Inventory,
    pub evidence_checked: u64,
    pub citations_checked: u64,
    pub search_checked: u64,
    pub blobs_checked: u64,
    pub blobs_rehashed: u64,
}

pub async fn move_postgres(options: &Options) -> Result<Report> {
    refuse_target(&options.state_dir)?;
    let mut source = postgres::Snapshot::open(&options.postgres_url).await?;
    let before = source.hashes().await?;
    let parent = options
        .state_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".hx-move-")
        .tempdir_in(parent)?;
    let state = staging.path().join("state");
    // Create the core BEFORE any blobs arrive. Keep Store::create's refusal
    // of a blob-only history intact for all other callers.
    let initial = Store::create(&state).await?;
    initial.pool.close().await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::rename(state.join("core.sqlite"), state.join("core.sqlite.moving"))?;
    let sqlite_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(state.join("core.sqlite.moving"))
        .foreign_keys(true)
        .pragma("recursive_triggers", "ON")
        .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete);
    #[cfg(target_os = "macos")]
    let sqlite_options = sqlite_options.pragma("fullfsync", "ON");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(sqlite_options)
        .await?;
    let store = Store::new(pool, &state);
    let result = async {
        let expected = source.load(&store).await?;
        core::verify(&store, &expected).await?;
        ensure!(
            source.hashes().await? == before,
            "source row hashes changed"
        );
        let (citations_checked, evidence_checked, search_checked) =
            verification::evidence(&mut source, &store).await?;
        let (blobs_checked, blobs_rehashed) =
            verification::blobs(&store, &options.source_data_dir, options.verify_all_blobs).await?;
        // Re-read after all verification to catch any unintended staging edits.
        core::verify(&store, &expected).await?;
        source.ensure_unchanged().await?;
        Ok::<_, anyhow::Error>(Report {
            tables: expected,
            evidence_checked,
            citations_checked,
            search_checked,
            blobs_checked,
            blobs_rehashed,
        })
    }
    .await;
    // Closing before cleanup/rename also works on platforms that forbid
    // deleting open database files. Errors never publish a target core.
    store.pool.close().await;
    source.close().await?;
    let report = result?;
    std::fs::rename(state.join("core.sqlite.moving"), state.join("core.sqlite"))?;
    std::fs::File::open(state.join("core.sqlite"))?.sync_all()?;
    std::fs::File::open(state.join("agent.sqlite"))?.sync_all()?;
    std::fs::File::open(&state)?.sync_all()?;
    refuse_target(&options.state_dir)?;
    // Directory rename publishes core, agent and blobs together. On supported
    // Unix hosts it cannot replace a nonempty directory created by a competitor.
    std::fs::rename(&state, &options.state_dir)?;
    if let Err(error) = std::fs::File::open(parent).and_then(|directory| directory.sync_all()) {
        std::fs::rename(&options.state_dir, &state)?;
        return Err(error.into());
    }
    Ok(report)
}
fn refuse_target(path: &Path) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "target must be a directory, not a symlink"
        );
        ensure!(
            std::fs::read_dir(path)?.next().is_none(),
            "target must be empty; refusing an existing core or other files"
        );
    } else {
        ensure!(!path.exists(), "target unavailable");
    }
    Ok(())
}
