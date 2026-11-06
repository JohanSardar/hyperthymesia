//! Server-free loader and verifier. Hashes cover exact text, type, NULL and order.
use crate::schema::{Table, tables, target_table};
use anyhow::{Result, ensure};
use futures_util::TryStreamExt;
use hx_store::{Store, WriteTransaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, ValueRef};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    Null,
    Integer(i64),
    Text(String),
}
pub type DataRow = Vec<Cell>;
impl Cell {
    pub fn text(&self) -> Result<&str> {
        match self {
            Self::Text(s) => Ok(s),
            _ => anyhow::bail!("expected text"),
        }
    }
    pub fn integer(&self) -> Result<i64> {
        match self {
            Self::Integer(n) => Ok(*n),
            _ => anyhow::bail!("expected integer"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableHash {
    pub rows: u64,
    pub sha256: String,
}
#[derive(Default)]
pub struct RowHasher {
    rows: u64,
    digest: Sha256,
}
impl RowHasher {
    pub fn add(&mut self, row: &[Cell]) {
        self.digest.update((row.len() as u64).to_be_bytes());
        for cell in row {
            match cell {
                Cell::Null => self.digest.update([0]),
                Cell::Integer(n) => {
                    self.digest.update([1]);
                    self.digest.update(n.to_be_bytes());
                }
                Cell::Text(s) => {
                    self.digest.update([2]);
                    self.digest.update((s.len() as u64).to_be_bytes());
                    self.digest.update(s.as_bytes());
                }
            }
        }
        self.rows += 1;
    }
    pub fn finish(self) -> TableHash {
        TableHash {
            rows: self.rows,
            sha256: hex::encode(self.digest.finalize()),
        }
    }
}
pub type Inventory = BTreeMap<String, TableHash>;

pub async fn insert(tx: &mut WriteTransaction, table: &Table, row: &DataRow) -> Result<()> {
    ensure!(row.len() == table.columns.len(), "column count mismatch");
    let placeholders = (1..=row.len())
        .map(|n| format!("${n}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {}({}) VALUES({placeholders})",
        table.name,
        table.names()
    );
    // Identifiers come exclusively from the frozen schema inventory; values bind.
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for cell in row {
        query = match cell {
            Cell::Null => query.bind(None::<String>),
            Cell::Integer(n) => query.bind(*n),
            Cell::Text(s) => query.bind(s),
        };
    }
    query.execute(&mut **tx).await?;
    Ok(())
}

pub async fn table_hash(store: &Store, table: &Table) -> Result<TableHash> {
    let sql = format!(
        "SELECT {} FROM {} ORDER BY {}",
        table.names(),
        table.name,
        table.key
    );
    let mut stream = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).fetch(&store.pool);
    let mut hash = RowHasher::default();
    while let Some(row) = stream.try_next().await? {
        let cells = table
            .columns
            .iter()
            .enumerate()
            .map(|(i, (_, kind))| {
                Ok(if row.try_get_raw(i)?.is_null() {
                    Cell::Null
                } else if matches!(kind, 'i' | 'b' | 't') {
                    Cell::Integer(row.try_get(i)?)
                } else {
                    Cell::Text(row.try_get(i)?)
                })
            })
            .collect::<Result<DataRow>>()?;
        hash.add(&cells);
    }
    Ok(hash.finish())
}

pub async fn inventory(store: &Store) -> Result<Inventory> {
    let mut result = Inventory::new();
    for table in tables() {
        result.insert(
            table.name.to_owned(),
            table_hash(store, &target_table(&table)).await?,
        );
    }
    Ok(result)
}

pub async fn verify(store: &Store, expected: &Inventory) -> Result<()> {
    ensure!(
        expected.len() == tables().len(),
        "incomplete verification inventory"
    );
    let actual = inventory(store).await?;
    for table in tables() {
        ensure!(
            actual.get(table.name) == expected.get(table.name),
            "{} row count or hash differs",
            table.name
        );
    }
    let foreign_keys = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&store.pool)
        .await?;
    ensure!(foreign_keys.is_empty(), "foreign key check failed");
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&store.pool)
        .await?;
    ensure!(integrity == ["ok"], "integrity check failed");
    // rank=1 checks the external content against the index, not just FTS internals.
    let mut tx = store.begin_write().await?;
    sqlx::query("INSERT INTO normalized_fts(normalized_fts,rank) VALUES('integrity-check',1)")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
