//! Independent source audit: SQL aggregates read original named columns, never
//! the loader projection/decoder. Keyed per-column MD5 sums are a second check
//! on reader mistakes; the exact ordered SHA-256 row check remains mandatory.
use crate::{
    postgres::Snapshot,
    schema::{Table, tables},
};
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use hx_store::Store;
use md5::{Digest, Md5};
use sqlx::{Row, ValueRef};
use std::collections::BTreeMap;

#[derive(Default, Debug, PartialEq, Eq)]
struct ColumnAudit {
    non_null: u64,
    high: i128,
    low: i128,
}
#[derive(Default, Debug, PartialEq, Eq)]
struct TableAudit {
    rows: u64,
    columns: Vec<ColumnAudit>,
}
pub struct SourceAudit(BTreeMap<&'static str, TableAudit>);

// This conversion is intentionally separate from Table::postgres_select.
// JSONB's own text/key order is authoritative: SQLite must retain that verbatim
// text, including spaces and numbers. Arrays use PostgreSQL's JSONB rendering.
fn pg_text(name: &str, kind: char) -> String {
    match kind {
        't' => format!("to_char({name} AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US')"),
        'b' => format!("CASE WHEN {name} THEN '1' ELSE '0' END"),
        'a' => format!("to_jsonb({name})::text"),
        _ => format!("{name}::text"),
    }
}
fn key_columns(table: &Table) -> Vec<usize> {
    table
        .key
        .split(',')
        .map(|key| {
            let name = key.split_whitespace().next().expect("key name");
            table
                .columns
                .iter()
                .position(|c| c.0 == name)
                .expect("key column")
        })
        .collect()
}
fn sql_key(table: &Table) -> String {
    key_columns(table)
        .iter()
        .map(|&i| {
            let (name, kind) = table.columns[i];
            let text = pg_text(name, kind);
            format!("octet_length({text})::text || ':' || ({text})")
        })
        .collect::<Vec<_>>()
        .join(" || ',' || ")
}
fn audit_sql(table: &Table) -> String {
    let key = sql_key(table);
    let mut expressions = vec!["count(*)::text".to_owned()];
    for (name, kind) in &table.columns {
        let value = pg_text(name, *kind);
        let fingerprint = format!(
            "md5(convert_to(({key}) || '|' || octet_length({value})::text || ':' || ({value}),'UTF8'))"
        );
        expressions.push(format!("count({name})::text"));
        for start in [1, 17] {
            expressions.push(format!("coalesce(sum(CASE WHEN {name} IS NULL THEN 0::numeric ELSE (('x' || substr({fingerprint},{start},16))::bit(64)::bigint)::numeric END),0)::text"));
        }
    }
    format!("SELECT {} FROM {}", expressions.join(","), table.name)
}
impl SourceAudit {
    pub async fn read(source: &mut Snapshot) -> Result<Self> {
        let mut result = BTreeMap::new();
        for table in tables().into_iter().filter(|t| t.legacy) {
            // All identifiers and expressions come from the frozen inventory.
            let sql = audit_sql(&table);
            let row = sqlx::query(sqlx::AssertSqlSafe(sql))
                .fetch_one(&mut source.conn)
                .await?;
            let mut columns = Vec::new();
            for i in 0..table.columns.len() {
                columns.push(ColumnAudit {
                    non_null: row.try_get::<String, _>(1 + 3 * i)?.parse()?,
                    high: row.try_get::<String, _>(2 + 3 * i)?.parse()?,
                    low: row.try_get::<String, _>(3 + 3 * i)?.parse()?,
                });
            }
            result.insert(
                table.name,
                TableAudit {
                    rows: row.try_get::<String, _>(0)?.parse()?,
                    columns,
                },
            );
        }
        Ok(Self(result))
    }
    pub async fn verify(&self, store: &Store) -> Result<()> {
        for table in tables().into_iter().filter(|t| t.legacy) {
            let mut actual = TableAudit {
                rows: 0,
                columns: (0..table.columns.len())
                    .map(|_| ColumnAudit::default())
                    .collect(),
            };
            // Read named target columns directly; no loader Cell or projection.
            let sql = format!("SELECT {} FROM {}", table.names(), table.name);
            let mut rows = sqlx::query(sqlx::AssertSqlSafe(sql)).fetch(&store.pool);
            while let Some(row) = rows.try_next().await? {
                let mut values = Vec::new();
                for (i, (_, kind)) in table.columns.iter().enumerate() {
                    let value = if row.try_get_raw(i)?.is_null() {
                        None
                    } else {
                        Some(match kind {
                            't' => DateTime::<Utc>::from_timestamp_micros(row.try_get(i)?)
                                .ok_or_else(|| {
                                    anyhow::anyhow!("timestamp outside supported range")
                                })?
                                .format("%Y-%m-%dT%H:%M:%S%.6f")
                                .to_string(),
                            'i' | 'b' => row.try_get::<i64, _>(i)?.to_string(),
                            'n' => {
                                let value = row.try_get::<String, _>(i)?;
                                let value = value.trim_start_matches('0');
                                if value.is_empty() {
                                    "0".to_owned()
                                } else {
                                    value.to_owned()
                                }
                            }
                            _ => row.try_get::<String, _>(i)?,
                        })
                    };
                    values.push(value);
                }
                let key = key_columns(&table)
                    .iter()
                    .map(|&i| {
                        let value = values[i].as_ref().expect("non-null primary key");
                        format!("{}:{value}", value.len())
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                for (i, value) in values.into_iter().enumerate() {
                    if let Some(value) = value {
                        let digest =
                            Md5::digest(format!("{key}|{}:{value}", value.len()).as_bytes());
                        let column = &mut actual.columns[i];
                        column.non_null += 1;
                        column.high += i64::from_be_bytes(digest[..8].try_into()?) as i128;
                        column.low += i64::from_be_bytes(digest[8..].try_into()?) as i128;
                    }
                }
                actual.rows += 1;
            }
            let expected = &self.0[table.name];
            ensure!(
                actual.rows == expected.rows,
                "independent source audit: {} row count differs",
                table.name
            );
            for (i, (actual, expected)) in actual.columns.iter().zip(&expected.columns).enumerate()
            {
                ensure!(
                    actual == expected,
                    "independent source audit: {}.{} differs",
                    table.name,
                    table.columns[i].0
                );
            }
        }
        Ok(())
    }
}
