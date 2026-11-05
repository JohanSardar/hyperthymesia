//! SQLite foundation, unused by the Postgres store until the separate port.
//!
//! Only `create` initializes a state directory. `open` refuses missing or
//! unmigrated cores; it never silently creates a new history.
use anyhow::{Context, Result, ensure};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, sqlite::SqliteArguments};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    time::Duration,
};

mod ts;
pub use ts::Ts;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations-sqlite");

#[derive(Clone)]
pub struct Store {
    pub pool: SqlitePool,
}

impl Store {
    /// Create a new core, refusing to overwrite an existing one. The CLI's
    /// separate agent outbox initialization belongs to the cutover, not here.
    ///
    /// The core is built aside and published last, so a failed or interrupted
    /// create leaves nothing that a retry or `open` would have to refuse.
    pub async fn create(state_dir: impl AsRef<Path>) -> Result<Self> {
        let state_dir = state_dir.as_ref();
        tokio::fs::create_dir_all(state_dir).await?;
        let path = state_dir.join("core.sqlite");
        ensure!(
            !tokio::fs::try_exists(&path).await?,
            "{} already exists",
            path.display()
        );
        let aside = tempfile::Builder::new()
            .prefix(".core-creating")
            .tempdir_in(state_dir)?;
        let building = aside.path().join("core.sqlite");
        tokio::fs::File::create_new(&building).await?;
        // No WAL here: closing leaves the whole database in this one file.
        let store = Self::connect(&building, SqliteJournalMode::Delete).await?;
        let mut tx = store.begin_write().await?;
        MIGRATOR.run(&mut *tx).await?;
        tx.commit().await?;
        store.pool.close().await;
        tokio::fs::create_dir_all(state_dir.join("blobs")).await?;
        tokio::fs::create_dir_all(state_dir.join("locks")).await?;
        // Unlike rename, a hard link cannot replace a core created meanwhile.
        tokio::fs::hard_link(&building, &path)
            .await
            .with_context(|| format!("create {}", path.display()))?;
        std::fs::File::open(state_dir)?.sync_all()?;
        Self::open(state_dir).await
    }

    /// Open only a core initialized with this schema. Applying future
    /// migrations remains an explicit operation, never a side effect of serve.
    pub async fn open(state_dir: impl AsRef<Path>) -> Result<Self> {
        let path = state_dir.as_ref().join("core.sqlite");
        let store = Self::connect(&path, SqliteJournalMode::Wal).await?;
        let applied: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
            "SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&store.pool)
        .await
        .context("core.sqlite has not been initialized")?;
        ensure!(
            applied.len() == MIGRATOR.iter().len(),
            "core.sqlite schema version mismatch"
        );
        for ((version, success, checksum), expected) in applied.iter().zip(MIGRATOR.iter()) {
            ensure!(
                *success
                    && *version == expected.version
                    && checksum.as_slice() == expected.checksum.as_ref(),
                "core.sqlite migration {version} is incomplete or has a different checksum"
            );
        }
        Ok(store)
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
        ))
    }
}

pub struct WriteTransaction(sqlx::Transaction<'static, Sqlite>);

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

    pub async fn commit(self) -> Result<()> {
        self.0.commit().await?;
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
    let terms: Vec<_> = text
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        "\"\"".to_owned()
    } else {
        terms.join(" AND ")
    }
}
