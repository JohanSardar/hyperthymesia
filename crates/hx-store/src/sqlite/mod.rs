//! SQLite core and the only entry point for write transactions.
//!
//! Only `create` initializes a state directory. `open` refuses missing or
//! unmigrated cores; it never silently creates a new history.
use anyhow::{Context, Result, ensure};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Connection, Sqlite, SqliteConnection, SqlitePool, sqlite::SqliteArguments};
use std::{
    ops::{Deref, DerefMut},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

mod ts;
pub use ts::Ts;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations-sqlite");

#[derive(Clone)]
pub struct Store {
    pub pool: SqlitePool,
    pub data_dir: PathBuf,
    #[cfg(test)]
    pub(crate) gc_boundary: Option<std::sync::Arc<crate::concurrency_tests::GcBoundary>>,
}

impl Store {
    pub fn new(pool: SqlitePool, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            pool,
            data_dir: data_dir.into(),
            #[cfg(test)]
            gc_boundary: None,
        }
    }

    /// Create a new core and an empty agent database, refusing an existing core.
    ///
    /// The core is built aside and published last, so a failed or interrupted
    /// create leaves nothing that a retry or `open` would have to refuse.
    pub async fn create(state_dir: impl AsRef<Path>) -> Result<Self> {
        let state_dir = state_dir.as_ref();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(state_dir)?;
        ensure!(
            std::fs::symlink_metadata(state_dir)?.is_dir(),
            "state must be a directory, not a symlink"
        );
        std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;
        let path = state_dir.join("core.sqlite");
        ensure!(
            !tokio::fs::try_exists(&path).await?,
            "{} already exists",
            path.display()
        );
        let blobs = state_dir.join("blobs");
        if tokio::fs::try_exists(&blobs).await? {
            ensure!(
                tokio::fs::read_dir(&blobs)
                    .await?
                    .next_entry()
                    .await?
                    .is_none(),
                "refusing non-empty blobs folder without core.sqlite"
            );
        }
        let aside = tempfile::Builder::new()
            .prefix(".core-creating")
            .tempdir_in(state_dir)?;
        let building = aside.path().join("core.sqlite");
        private_file(&building)?;
        // No WAL here: closing leaves the whole database in this one file.
        let store = Self::connect(&building, SqliteJournalMode::Delete).await?;
        let mut tx = store.begin_write().await?;
        MIGRATOR.run(&mut *tx).await?;
        tx.commit().await?;
        store.pool.close().await;
        tokio::fs::create_dir_all(state_dir.join("blobs")).await?;
        tokio::fs::create_dir_all(state_dir.join("locks")).await?;
        // Initialize an empty outbox if absent. A failed create can leave this
        // empty sibling behind; retries preserve it and still publish core last.
        let agent = state_dir.join("agent.sqlite");
        if !tokio::fs::try_exists(&agent).await? {
            let building_agent = aside.path().join("agent.sqlite");
            private_file(&building_agent)?;
            let mut conn = sqlx::SqliteConnection::connect_with(
                &SqliteConnectOptions::new()
                    .filename(&building_agent)
                    .create_if_missing(true),
            )
            .await?;
            sqlx::query("VACUUM").execute(&mut conn).await?;
            conn.close().await?;
            std::fs::File::open(&building_agent)?.sync_all()?;
            match tokio::fs::hard_link(&building_agent, &agent).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        // Unlike rename, a hard link cannot replace a core created meanwhile.
        tokio::fs::hard_link(&building, &path)
            .await
            .with_context(|| format!("create {}", path.display()))?;
        std::fs::File::open(state_dir)?.sync_all()?;
        Self::open(state_dir).await
    }

    /// Explicitly apply missing migrations. `serve` continues to use `open`.
    /// Validate the complete applied set before any writable connection or DDL,
    /// then revalidate under BEGIN IMMEDIATE. The whole upgrade is one commit.
    pub async fn migrate(state_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = state_dir.as_ref();
        if !dir.join("core.sqlite").try_exists()? {
            return Self::create(dir).await;
        }
        ensure!(
            Self::status(dir).await? == 0,
            "upgrade refused: running interpret lease"
        );
        Self::upgrade_with(dir, &MIGRATOR).await?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        for name in ["core.sqlite", "agent.sqlite"] {
            let path = dir.join(name);
            ensure!(
                std::fs::symlink_metadata(&path)?.is_file(),
                "expected a regular database file"
            );
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Self::open(dir).await
    }

    async fn upgrade_with(state_dir: &Path, migrator: &sqlx::migrate::Migrator) -> Result<()> {
        let path = state_dir.join("core.sqlite");
        let mut validation = readonly(&path).await?;
        validate_schema(&mut validation, migrator, false).await?;
        validation.close().await?;
        let store = Self::connect(&path, SqliteJournalMode::Wal).await?;
        let result = async {
            let mut tx = store.begin_write().await?;
            validate_schema(&mut tx, migrator, false).await?;
            // Repeat the lease check under the same writer as the DDL. Tests
            // with synthetic migration histories need not contain a jobs table.
            let has_jobs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='jobs')").fetch_one(&mut *tx).await?;
            if has_jobs {
                let live: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='interpret' AND state='running' AND lease_until>$1").bind(Ts::now()).fetch_one(&mut *tx).await?;
                ensure!(live == 0, "upgrade refused: running interpret lease");
            }
            migrator.run(&mut *tx).await?;
            tx.commit().await
        }.await;
        store.pool.close().await;
        result
    }

    /// A version-tolerant, read-only lease check for installers. No pool, WAL
    /// mode change, migration, job revival or provider call is performed.
    pub async fn status(state_dir: impl AsRef<Path>) -> Result<i64> {
        let mut conn = readonly(&state_dir.as_ref().join("core.sqlite")).await?;
        validate_schema(&mut conn, &MIGRATOR, false).await?;
        let count = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='interpret' AND state='running' AND lease_until>$1")
            .bind(Ts::now()).fetch_one(&mut conn).await?;
        conn.close().await?;
        Ok(count)
    }

    /// Readiness without creating state, changing journals or reading secrets.
    pub async fn check_state(state_dir: impl AsRef<Path>) -> Result<()> {
        Self::check_state_schema(state_dir.as_ref(), true).await
    }

    /// Installer preflight: same privacy/integrity checks, permitting only
    /// missing embedded migrations. No writes or provider checks.
    pub async fn check_upgrade_state(state_dir: impl AsRef<Path>) -> Result<()> {
        Self::check_state_schema(state_dir.as_ref(), false).await
    }

    async fn check_state_schema(dir: &Path, exact: bool) -> Result<()> {
        let metadata = std::fs::symlink_metadata(dir)?;
        ensure!(
            metadata.is_dir() && metadata.permissions().mode() & 0o7777 == 0o700,
            "state directory must be a real directory with mode 700"
        );
        for name in ["core.sqlite", "agent.sqlite", "server.env", "server.log"] {
            let metadata = std::fs::symlink_metadata(dir.join(name))?;
            ensure!(
                metadata.is_file() && metadata.permissions().mode() & 0o7777 == 0o600,
                "{name} must be a regular file with mode 600"
            );
        }
        for name in ["blobs", "locks"] {
            ensure!(
                std::fs::symlink_metadata(dir.join(name))?.is_dir(),
                "{name} must be a real directory"
            );
        }
        let mut conn = readonly(&dir.join("core.sqlite")).await?;
        validate_schema(&mut conn, &MIGRATOR, exact).await?;
        let checks: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_all(&mut conn)
            .await?;
        ensure!(checks == ["ok"], "core.sqlite integrity check failed");
        conn.close().await?;
        let mut conn = readonly(&dir.join("agent.sqlite")).await?;
        let checks: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_all(&mut conn)
            .await?;
        ensure!(checks == ["ok"], "agent.sqlite integrity check failed");
        conn.close().await?;
        Ok(())
    }

    /// Open only the exact embedded schema, without implicitly migrating it.
    pub async fn open(state_dir: impl AsRef<Path>) -> Result<Self> {
        let path = state_dir.as_ref().join("core.sqlite");
        let mut validation = readonly(&path).await?;
        validate_schema(&mut validation, &MIGRATOR, true).await?;
        validation.close().await?;
        Self::connect(&path, SqliteJournalMode::Wal).await
    }

    async fn connect(path: &Path, journal: SqliteJournalMode) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .journal_mode(journal)
            .busy_timeout(Duration::from_secs(30))
            .foreign_keys(true)
            .pragma("recursive_triggers", "ON")
            .synchronous(SqliteSynchronous::Full);
        #[cfg(target_os = "macos")]
        let options = options.pragma("fullfsync", "ON");
        Ok(Self {
            data_dir: path.parent().context("core parent")?.to_owned(),
            #[cfg(test)]
            gc_boundary: None,
            pool: SqlitePoolOptions::new()
                .max_connections(8)
                .connect_with(options)
                .await?,
        })
    }

    /// Take the writer lock before reading any state. A deferred read followed
    /// by a write could fail with SQLITE_BUSY_SNAPSHOT despite busy_timeout.
    pub async fn begin_write(&self) -> Result<WriteTransaction> {
        Ok(WriteTransaction(
            self.pool.begin_with("BEGIN IMMEDIATE").await?,
            std::time::Instant::now(),
            #[cfg(test)]
            None,
        ))
    }
}

fn private_file(path: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    Ok(())
}

async fn readonly(path: &Path) -> Result<SqliteConnection> {
    ensure!(
        std::fs::symlink_metadata(path)?.is_file(),
        "expected a regular database file"
    );
    Ok(SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false),
    )
    .await?)
}

async fn validate_schema(
    conn: &mut SqliteConnection,
    migrator: &sqlx::migrate::Migrator,
    complete: bool,
) -> Result<()> {
    let applied: Vec<(i64, bool, Vec<u8>)> =
        sqlx::query_as("SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(conn)
            .await
            .context("core.sqlite has not been initialized")?;
    // Compare the applied set, not MAX(version): reserved numbers can leave gaps.
    for (version, success, checksum) in &applied {
        let expected = migrator
            .iter()
            .find(|m| m.version == *version)
            .with_context(|| {
                format!("core.sqlite migration {version} is newer than or unknown to this binary")
            })?;
        ensure!(
            *success && checksum.as_slice() == expected.checksum.as_ref(),
            "core.sqlite migration {version} is incomplete or has a different checksum"
        );
    }
    ensure!(
        !complete || applied.len() == migrator.iter().len(),
        "core.sqlite schema version mismatch; run hx-server migrate explicitly"
    );
    Ok(())
}

pub struct WriteTransaction(
    sqlx::Transaction<'static, Sqlite>,
    std::time::Instant,
    #[cfg(test)] Option<std::sync::Arc<crate::concurrency_tests::GcBoundary>>,
);

impl WriteTransaction {
    /// Execute an INSERT using `(SELECT next_id FROM evidence_seq)` for its ID,
    /// `ON CONFLICT ... DO NOTHING RETURNING id`, then advance only on success.
    /// Both the row and the allocator roll back together. As with any write
    /// error, the caller must roll back/drop the transaction on an error here.
    pub async fn insert_evidence<'q>(
        &mut self,
        insert: sqlx::query::QueryScalar<'q, Sqlite, i64, SqliteArguments>,
    ) -> Result<Option<i64>> {
        let id = insert.fetch_optional(&mut **self).await?;
        if let Some(id) = id {
            let updated = sqlx::query("UPDATE evidence_seq SET next_id=next_id+1 WHERE next_id=$1")
                .bind(id)
                .execute(&mut **self)
                .await?;
            ensure!(
                updated.rows_affected() == 1,
                "evidence INSERT must use evidence_seq.next_id"
            );
        }
        Ok(id)
    }

    #[cfg(test)]
    pub(crate) fn observe_gc_commit(
        &mut self,
        boundary: std::sync::Arc<crate::concurrency_tests::GcBoundary>,
    ) {
        self.2 = Some(boundary);
    }
    pub async fn commit(self) -> Result<()> {
        self.0.commit().await?;
        tracing::debug!(
            writer_hold_us = self.1.elapsed().as_micros() as u64,
            "SQLite writer committed"
        );
        #[cfg(test)]
        if let Some(boundary) = self.2 {
            boundary.after_commit().await;
        }
        Ok(())
    }

    pub async fn rollback(self) -> Result<()> {
        self.0.rollback().await?;
        Ok(())
    }
}

impl Deref for WriteTransaction {
    type Target = SqliteConnection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for WriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Treat each whitespace-separated term as a literal phrase, joined by AND.
/// Quoting also prevents FTS operators from changing the query's meaning.
pub fn search_query(text: &str) -> String {
    // Ask the bundled tokenizer itself: Rust's Unicode tables have a different
    // version/category definition from unicode61 (which includes private use).
    let terms: Vec<_> = text.split_whitespace().collect();
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory tokenizer");
    conn.execute_batch("CREATE VIRTUAL TABLE terms USING fts5(term, tokenize='unicode61 remove_diacritics 0'); CREATE VIRTUAL TABLE tokens USING fts5vocab(terms,instance);").expect("bundled unicode61");
    for (id, term) in terms.iter().enumerate() {
        conn.execute(
            "INSERT INTO terms(rowid,term) VALUES(?1,?2)",
            rusqlite::params![id as i64 + 1, term],
        )
        .expect("tokenize term");
    }
    let mut query = conn
        .prepare("SELECT DISTINCT doc FROM tokens ORDER BY doc")
        .expect("tokenizer vocabulary");
    let retained: Vec<String> = query
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("tokenizer rows")
        .map(|id| {
            format!(
                "\"{}\"",
                terms[id.expect("token row") as usize - 1].replace('"', "\"\"")
            )
        })
        .collect();
    if retained.is_empty() {
        "\"\"".into()
    } else {
        retained.join(" AND ")
    }
}

#[cfg(test)]
mod upgrade_tests {
    use super::*;
    use sqlx::migrate::{Migration, MigrationType, Migrator};
    use std::{borrow::Cow, os::unix::fs::PermissionsExt};

    fn migrations(versions: &[i64]) -> Migrator {
        Migrator {
            migrations: Cow::Owned(
                versions
                    .iter()
                    .map(|v| {
                        Migration::new(
                            *v,
                            Cow::Owned(format!("test {v}")),
                            MigrationType::Simple,
                            sqlx::SqlStr::from_static(match v {
                                1 => "CREATE TABLE test_1(value TEXT PRIMARY KEY);",
                                2 => "CREATE TABLE rebuilt(value TEXT REFERENCES test_1(value), extra TEXT); INSERT INTO rebuilt(value) SELECT value FROM test_3; DROP TABLE test_3; ALTER TABLE rebuilt RENAME TO test_3;",
                                3 => "CREATE TABLE test_3(value TEXT REFERENCES test_1(value));",
                                _ => "CREATE TABLE test_4(value TEXT);",
                            }),
                            false,
                        )
                    })
                    .collect(),
            ),
            ..Migrator::DEFAULT
        }
    }

    #[tokio::test]
    async fn upgrade_applies_late_lower_migration_and_rejects_newer_or_tampered() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("core.sqlite");
        std::fs::File::create(&file)?;
        let store = Store::connect(&file, SqliteJournalMode::Delete).await?;
        let mut tx = store.begin_write().await?;
        migrations(&[1, 3]).run(&mut *tx).await?;
        sqlx::query("INSERT INTO test_1 VALUES('synthetic evidence')")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO test_3 VALUES('synthetic evidence')")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        store.pool.close().await;
        // A later DDL failure must also roll back the newly added lower number.
        let mut broken = migrations(&[1, 2, 3, 4]);
        broken.migrations.to_mut()[3] = Migration::new(
            4,
            Cow::Borrowed("broken"),
            MigrationType::Simple,
            sqlx::SqlStr::from_static("CREATE TABLE test_3(value TEXT);"),
            false,
        );
        assert!(Store::upgrade_with(dir.path(), &broken).await.is_err());
        let check = Store::connect(&file, SqliteJournalMode::Delete).await?;
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&check.pool)
                .await?;
        assert_eq!(versions, [1, 3]);
        check.pool.close().await;
        Store::upgrade_with(dir.path(), &migrations(&[1, 2, 3])).await?;
        Store::upgrade_with(dir.path(), &migrations(&[1, 2, 3])).await?;
        let store = Store::connect(&file, SqliteJournalMode::Delete).await?;
        let value: String = sqlx::query_scalar("SELECT value FROM test_3")
            .fetch_one(&store.pool)
            .await?;
        assert_eq!(value, "synthetic evidence");
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&store.pool)
            .await?;
        assert_eq!(foreign_keys, 1);
        assert!(
            sqlx::query("INSERT INTO test_3(value) VALUES('orphan')")
                .execute(&store.pool)
                .await
                .is_err()
        );
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&store.pool)
            .await?;
        assert!(violations.is_empty());
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&store.pool)
                .await?;
        assert_eq!(
            versions,
            migrations(&[1, 2, 3])
                .iter()
                .map(|m| m.version)
                .collect::<Vec<_>>()
        );
        store.pool.close().await;
        let before = std::fs::read(&file)?;
        assert!(
            Store::upgrade_with(dir.path(), &migrations(&[1, 2]))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&file)?, before);
        let store = Store::connect(&file, SqliteJournalMode::Delete).await?;
        let mut tx = store.begin_write().await?;
        sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00' WHERE version=3")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        store.pool.close().await;
        let before = std::fs::read(&file)?;
        assert!(
            Store::upgrade_with(dir.path(), &migrations(&[1, 2, 3, 4]))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&file)?, before);
        Ok(())
    }

    #[tokio::test]
    async fn real_sources_migration_upgrades_1_3_without_changing_evidence_or_pairing() -> Result<()>
    {
        use crate::{Token, UploadBatch};
        use hx_schema::Envelope;
        let dir = tempfile::tempdir()?;
        for name in ["core.sqlite", "agent.sqlite"] {
            private_file(&dir.path().join(name))?;
        }
        rusqlite::Connection::open(dir.path().join("agent.sqlite"))?.execute_batch("VACUUM;")?;
        for name in ["blobs", "locks"] {
            std::fs::create_dir(dir.path().join(name))?;
        }
        // Reconstruct the actual pre-#66 core, not a synthetic migration 0002.
        let previous = Migrator {
            migrations: Cow::Owned(
                MIGRATOR
                    .iter()
                    .filter(|m| matches!(m.version, 1 | 3))
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        assert_eq!(
            previous.iter().map(|m| m.version).collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(
            MIGRATOR
                .iter()
                .find(|m| m.version == 2)
                .unwrap()
                .sql
                .as_str(),
            include_str!("../../migrations-sqlite/0002_sources.sql")
        );
        let store =
            Store::connect(&dir.path().join("core.sqlite"), SqliteJournalMode::Delete).await?;
        let mut tx = store.begin_write().await?;
        previous.run(&mut *tx).await?;
        tx.commit().await?;
        let event: Envelope = serde_json::from_value(serde_json::json!({
            "envelope_version": 1, "observation_id": "01J00000000000000000000001",
            "connector_instance": "test:upgrade", "source_key": "synthetic", "revision": "1",
            "operation": "observe", "kind": "web.visit", "schema_version": 1,
            "time": {"observed_at": "2026-01-01T00:00:00Z", "resolution_ms": 1000},
            "sensitivity": "normal", "payload": {"url": "https://example.invalid", "title": "Synthetic upgrade evidence"}
        }))?;
        let token = Token {
            prefixes: vec!["test:upgrade".into()],
            scopes: vec!["ingest".into()],
        };
        let batch = || UploadBatch {
            batch_id: ulid::Ulid::generate(),
            envelopes: vec![event.clone()],
        };
        assert_eq!(store.ingest(batch(), &token).await?.accepted, 1);
        let code = store.create_pair_code("synthetic upgrade device").await?;
        let before: Vec<(i64, String)> =
            sqlx::query_as("SELECT id,envelope FROM events ORDER BY id")
                .fetch_all(&store.pool)
                .await?;
        let checksums: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&store.pool)
                .await?;
        assert_eq!(
            checksums.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
            [1, 3]
        );
        // Seed the OCR parent/child pair too: a later embedded table rebuild
        // must preserve existing rows, not merely succeed on empty tables.
        sqlx::query("INSERT INTO blobs(sha256,bytes,created_at) VALUES($1,0,0)")
            .bind("a".repeat(64))
            .execute(&store.pool)
            .await?;
        let event_id = before[0].0;
        sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,available_at,status,result) VALUES($1,'test-upgrade',$2,0,'complete','{}')")
            .bind("a".repeat(64)).bind(event_id).execute(&store.pool).await?;
        let mut tx = store.begin_write().await?;
        let ocr_id = tx.insert_evidence(sqlx::query_scalar("INSERT INTO ocr_evidence(id,observation_id,source_event_id,sha256,engine_version) VALUES((SELECT next_id FROM evidence_seq),'synthetic-ocr',$1,$2,'test-upgrade') RETURNING id")
            .bind(event_id).bind("a".repeat(64))).await?.unwrap();
        tx.commit().await?;
        store.pool.close().await;
        assert!(
            Store::open(dir.path()).await.is_err(),
            "serve must not migrate"
        );
        assert_eq!(Store::status(dir.path()).await?, 0);
        let store = Store::migrate(dir.path()).await?;
        let fresh = tempfile::tempdir()?;
        let fresh_store = Store::create(fresh.path()).await?;
        let upgraded_fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&store.pool)
            .await?;
        let created_fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&fresh_store.pool)
            .await?;
        assert_eq!(upgraded_fk, created_fk);
        assert_eq!(upgraded_fk, 1);
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&store.pool)
                .await?
                .is_empty()
        );
        fresh_store.pool.close().await;
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&store.pool)
                .await?;
        assert_eq!(
            versions,
            MIGRATOR.iter().map(|m| m.version).collect::<Vec<_>>()
        );
        let after: Vec<(i64, String)> =
            sqlx::query_as("SELECT id,envelope FROM events ORDER BY id")
                .fetch_all(&store.pool)
                .await?;
        assert_eq!(after, before);
        let ocr: (i64, String, String) = sqlx::query_as("SELECT e.id,e.observation_id,j.result FROM ocr_evidence e JOIN ocr_jobs j USING(sha256,engine_version) WHERE e.id=$1")
            .bind(ocr_id).fetch_one(&store.pool).await?;
        assert_eq!(ocr, (ocr_id, "synthetic-ocr".into(), "{}".into()));
        let old_checksums: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT version,checksum FROM _sqlx_migrations WHERE version IN (1,3) ORDER BY version",
        )
        .fetch_all(&store.pool)
        .await?;
        assert_eq!(old_checksums, checksums);
        let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('sources','recipes','live_snapshots')").fetch_one(&store.pool).await?;
        assert_eq!(tables, 3);
        let retry = store.ingest(batch(), &token).await?;
        assert_eq!(
            (retry.accepted, retry.duplicates, retry.rejected.len()),
            (0, 1, 0)
        );
        assert!(
            store
                .redeem_pair_code(&code, "synthetic-upgrade")
                .await?
                .is_some()
        );
        store.pool.close().await;
        let store = Store::migrate(dir.path()).await?;
        store.pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn created_state_is_private_without_relying_on_umask() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let dir = parent.path().join("state");
        let store = Store::create(&dir).await?;
        assert_eq!(std::fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
        for file in ["core.sqlite", "agent.sqlite"] {
            assert_eq!(
                std::fs::metadata(dir.join(file))?.permissions().mode() & 0o777,
                0o600
            );
        }
        store.pool.close().await;
        Ok(())
    }
}
