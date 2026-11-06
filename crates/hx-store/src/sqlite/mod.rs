//! SQLite core and the only entry point for write transactions.
//!
//! Only `create` initializes a state directory. `open` refuses missing or
//! unmigrated cores; it never silently creates a new history.
use anyhow::{Context, Result, ensure};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Connection, Sqlite, SqliteConnection, SqlitePool, sqlite::SqliteArguments};
use std::{
    ops::{Deref, DerefMut},
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
        tokio::fs::create_dir_all(state_dir).await?;
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
        tokio::fs::File::create_new(&building).await?;
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

    /// Open only a core initialized with this schema. Applying future
    /// migrations remains an explicit operation, never a side effect of serve.
    pub async fn open(state_dir: impl AsRef<Path>) -> Result<Self> {
        let path = state_dir.as_ref().join("core.sqlite");
        // Validate through a read-only connection without journal pragmas. Even
        // refusing an unrelated SQLite file must leave its journal unchanged.
        let mut validation = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .read_only(true)
                .create_if_missing(false),
        )
        .await?;
        let applied: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
            "SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&mut validation)
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
