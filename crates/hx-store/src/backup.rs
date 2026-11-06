//! Online, independently consistent SQLite snapshots. The manifest is the commit
//! marker: without it a backup is incomplete and restore must refuse it.
//!
//! VACUUM INTO uses read-only source connections, so WAL writers keep running.
//! Blob I/O and hashing never take the source writer. If explicit GC removes a
//! snapshotted blob before we open it, backup fails closed; retry after GC ends.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MANIFEST: &str = "manifest.json";
const DATABASES: [&str; 2] = ["core.sqlite", "agent.sqlite"];
const BACKUP_OWNER: &[u8] = b"hyperthymesia-backup-v1\n";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    files: BTreeMap<String, FileHash>,
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileHash {
    bytes: u64,
    sha256: String,
}

/// Reuse verified blobs in an unfinished destination. Completed backups are
/// immutable: choose a new destination for each completed snapshot.
pub async fn backup(state_dir: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let source = state_dir.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    tokio::task::spawn_blocking(move || backup_blocking(&source, &to)).await?
}

/// Build and verify beside the target, then publish the whole state by rename.
/// Even process death before publication leaves the target absent or empty.
pub async fn restore(from: impl AsRef<Path>, state_dir: impl AsRef<Path>) -> Result<()> {
    let from = from.as_ref().to_owned();
    let target = state_dir.as_ref().to_owned();
    tokio::task::spawn_blocking(move || restore_blocking(&from, &target)).await?
}

fn backup_blocking(source: &Path, to: &Path) -> Result<()> {
    let source = source.canonicalize()?;
    fs::create_dir_all(to)?;
    directory(to)?;
    let to = to.canonicalize()?;
    ensure!(
        !to.starts_with(&source) && !source.starts_with(&to),
        "backup and source must be separate directories"
    );
    let lock_path = to.join(".backup.lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(_) => regular_file(&lock_path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Keep this inode across retries; closing releases the lock after a crash.
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock()
        .context("another backup is using this destination")?;
    ensure!(
        !to.join(MANIFEST).try_exists()?,
        "backup is already complete; choose a new --to directory"
    );
    let mut owner = Vec::new();
    lock.read_to_end(&mut owner)?;
    if owner.is_empty() {
        // Do not mistake another state directory for an interrupted backup and
        // replace its history. Only a blob cache may predate the first attempt.
        for entry in fs::read_dir(&to)? {
            let name = entry?.file_name();
            ensure!(
                name == ".backup.lock" || name == "blobs",
                "backup destination contains unrelated files"
            );
        }
        lock.write_all(BACKUP_OWNER)?;
        lock.sync_all()?;
        sync_dir(&to)?;
    } else {
        ensure!(
            owner == BACKUP_OWNER,
            "unrecognized backup destination lock"
        );
    }
    let stage = tempfile::Builder::new()
        .prefix(".snapshot-")
        .tempdir_in(&to)?;
    for name in DATABASES {
        let source_file = source.join(name);
        regular_file(&source_file)?;
        let conn = Connection::open_with_flags(&source_file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.execute(
            "VACUUM main INTO ?1",
            [stage
                .path()
                .join(name)
                .to_str()
                .context("UTF-8 backup path")?],
        )
        .with_context(|| format!("snapshot {name}"))?;
        check_database(&stage.path().join(name), name == "core.sqlite")?;
    }
    let mut manifest = Manifest {
        version: 1,
        files: BTreeMap::new(),
    };
    for (sha, bytes) in published_blobs(&stage.path().join("core.sqlite"))? {
        let relative = blob_name(&sha)?;
        let dest = to.join(&relative);
        make_directory(&to.join("blobs"))?;
        make_directory(dest.parent().context("blob parent")?)?;
        let expected = FileHash { bytes, sha256: sha };
        if dest.try_exists()? {
            ensure!(
                hash_file(&dest)? == expected,
                "cached blob hash mismatch: {relative}"
            );
        } else {
            let src = checked_path(&source, &relative)?;
            let mut temp = tempfile::NamedTempFile::new_in(dest.parent().context("blob parent")?)?;
            let actual = copy_hash(&src, temp.as_file_mut())?;
            ensure!(actual == expected, "source blob hash mismatch: {relative}");
            temp.persist_noclobber(&dest)?;
        }
        sync_dir(dest.parent().context("blob parent")?)?;
        manifest.files.insert(relative, expected);
    }
    for name in DATABASES {
        let snapshot = stage.path().join(name);
        let digest = hash_file(&snapshot)?;
        File::open(&snapshot)?.sync_all()?;
        // Only unfinished backups can reach here. Never replace a symlink.
        let dest = to.join(name);
        if dest.try_exists()? {
            regular_file(&dest)?;
        }
        fs::rename(snapshot, dest)?;
        manifest.files.insert(name.into(), digest);
    }
    if to.join("blobs").try_exists()? {
        sync_dir(&to.join("blobs"))?;
    }
    sync_dir(&to)?;
    let mut marker = tempfile::NamedTempFile::new_in(&to)?;
    serde_json::to_writer_pretty(marker.as_file_mut(), &manifest)?;
    marker.as_file_mut().write_all(b"\n")?;
    marker.as_file().sync_all()?;
    // All data and directory entries are durable before the completion marker.
    marker.persist_noclobber(to.join(MANIFEST))?;
    if let Err(error) = sync_dir(&to) {
        fs::remove_file(to.join(MANIFEST))?;
        return Err(error);
    }
    Ok(())
}

fn restore_blocking(from: &Path, target: &Path) -> Result<()> {
    directory(from)?;
    let from = from.canonicalize()?;
    let target = std::path::absolute(target)?;
    empty_target(&target)?;
    let manifest: Manifest = serde_json::from_reader(File::open(checked_path(&from, MANIFEST)?)?)
        .context("missing or invalid backup manifest")?;
    ensure!(manifest.version == 1, "unsupported backup manifest version");
    for name in DATABASES {
        ensure!(
            manifest.files.contains_key(name),
            "manifest is missing {name}"
        );
    }
    for (name, hash) in &manifest.files {
        ensure!(
            valid_name(name) && crate::valid_hash(&hash.sha256),
            "invalid manifest entry: {name}"
        );
        if let Some(sha) = name
            .strip_prefix("blobs/")
            .and_then(|s| s.split('/').nth(1))
        {
            ensure!(sha == hash.sha256, "blob name/hash mismatch: {name}");
        }
    }
    let parent = target.parent().context("restore target parent")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".hx-restoring-")
        .tempdir_in(parent)?;
    for (name, expected) in &manifest.files {
        let dest = stage.path().join(name);
        fs::create_dir_all(dest.parent().context("restore file parent")?)?;
        let actual = copy_hash(&checked_path(&from, name)?, &mut File::create_new(&dest)?)?;
        ensure!(actual == *expected, "backup hash mismatch: {name}");
    }
    for name in DATABASES {
        check_database(&stage.path().join(name), name == "core.sqlite")?;
    }
    // Hashes alone cannot detect an omitted blob entry in an edited manifest.
    let blobs = published_blobs(&stage.path().join("core.sqlite"))?;
    ensure!(
        manifest.files.len() == DATABASES.len() + blobs.len(),
        "manifest blob inventory mismatch"
    );
    for (sha, bytes) in blobs {
        ensure!(
            manifest.files.get(&blob_name(&sha)?) == Some(&FileHash { bytes, sha256: sha }),
            "manifest is missing a published blob"
        );
    }
    fs::create_dir_all(stage.path().join("blobs"))?;
    fs::create_dir(stage.path().join("locks"))?;
    sync_tree(stage.path())?;
    empty_target(&target)?;
    // rename refuses a target directory populated meanwhile. No target files
    // are installed one at a time, so failures and interruptions are retryable.
    fs::rename(stage.path(), &target)?;
    if let Err(error) = sync_dir(parent) {
        fs::rename(&target, stage.path()).context("roll back restore publication")?;
        return Err(error);
    }
    Ok(())
}

fn published_blobs(core: &Path) -> Result<Vec<(String, u64)>> {
    let conn = Connection::open_with_flags(core, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement =
        conn.prepare("SELECT sha256,bytes FROM blobs WHERE published ORDER BY sha256")?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    rows.map(|r| {
        let (sha, bytes) = r?;
        Ok((sha, u64::try_from(bytes)?))
    })
    .collect()
}

fn check_database(path: &Path, core: bool) -> Result<()> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let checks = conn
        .prepare("PRAGMA integrity_check")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        checks == ["ok"],
        "SQLite integrity check failed: {}",
        path.display()
    );
    ensure!(
        !conn.prepare("PRAGMA foreign_key_check")?.exists([])?,
        "SQLite foreign key check failed: {}",
        path.display()
    );
    if core {
        conn.execute(
            "INSERT INTO normalized_fts(normalized_fts,rank) VALUES('integrity-check',1)",
            [],
        )
        .context("FTS integrity check failed")?;
    }
    Ok(())
}

fn blob_name(sha: &str) -> Result<String> {
    ensure!(crate::valid_hash(sha), "invalid blob hash");
    Ok(format!("blobs/{}/{sha}", &sha[..2]))
}
fn valid_name(name: &str) -> bool {
    DATABASES.contains(&name)
        || name
            .rsplit('/')
            .next()
            .and_then(|sha| blob_name(sha).ok())
            .as_deref()
            == Some(name)
}
fn checked_path(root: &Path, name: &str) -> Result<PathBuf> {
    let path = root.join(name);
    let mut current = root.to_owned();
    let parts: Vec<_> = Path::new(name).components().collect();
    for (i, part) in parts.iter().enumerate() {
        ensure!(
            matches!(part, std::path::Component::Normal(_)),
            "invalid relative path"
        );
        current.push(part);
        if i + 1 == parts.len() {
            regular_file(&current)?;
        } else {
            directory(&current)?;
        }
    }
    Ok(path)
}
fn regular_file(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "expected a regular file: {}",
        path.display()
    );
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_dir(),
        "expected a directory: {}",
        path.display()
    );
    Ok(())
}
fn make_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => directory(path),
        Err(e) => Err(e.into()),
    }
}
fn empty_target(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            directory(path)?;
            ensure!(
                fs::read_dir(path)?.next().is_none(),
                "restore requires an empty target: {}",
                path.display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
fn hash_file(path: &Path) -> Result<FileHash> {
    regular_file(path)?;
    digest(&mut File::open(path)?, &mut std::io::sink())
}
fn copy_hash(path: &Path, to: &mut File) -> Result<FileHash> {
    regular_file(path)?;
    let hash = digest(&mut File::open(path)?, to)?;
    to.sync_all()?;
    Ok(hash)
}
fn digest(from: &mut impl Read, to: &mut impl Write) -> Result<FileHash> {
    let mut sha = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        to.write_all(&buffer[..n])?;
        sha.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok(FileHash {
        bytes,
        sha256: hex::encode(sha.finalize()),
    })
}
fn sync_dir(path: &Path) -> Result<()> {
    Ok(File::open(path)?.sync_all()?)
}
fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        } else {
            File::open(entry.path())?.sync_all()?;
        }
    }
    sync_dir(path)
}
