//! No model SQL ever opens core.sqlite. The authorizer is deny-by-default.
use crate::mcp::{Broker, RunMode};
use anyhow::{Result, ensure};
use hx_store::Store;
use rusqlite::{
    Connection, OpenFlags,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::{Value, json};

pub async fn tables(store: &Store, source: Option<&str>) -> Result<Value> {
    let path = store.data_dir.join("agent.sqlite");
    let prefixes: Vec<String> = store
        .sources()
        .await?
        .iter()
        .filter(|s| source.is_none_or(|id| id == s.id))
        .map(|s| format!("s_{}_", s.id))
        .collect();
    tokio::task::spawn_blocking(move || {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let names = table_names(&conn)?;
        let mut result = vec![];
        for name in names
            .into_iter()
            .filter(|n| prefixes.iter().any(|p| n.starts_with(p)))
        {
            let count: i64 =
                conn.query_row(&format!("SELECT count(*) FROM {}", quote(&name)), [], |r| {
                    r.get(0)
                })?;
            result.push(json!({"name":name,"row_count":count}));
        }
        Ok(json!(result))
    })
    .await?
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn table_names(conn: &Connection) -> Result<Vec<String>> {
    Ok(conn
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
fn scalar(value: ValueRef<'_>) -> Result<Value> {
    Ok(match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => json!(v),
        ValueRef::Real(v) => json!(v),
        ValueRef::Text(v) => json!(std::str::from_utf8(v)?),
        ValueRef::Blob(_) => anyhow::bail!("binary values are not supported"),
    })
}
impl Broker {
    pub(crate) async fn agent_sql(&self, args: &Value) -> Result<Value> {
        self.check_source(args).await?;
        let source = args["source"].as_str().unwrap().to_owned();
        let setup = matches!(self.mode(), RunMode::Setup { .. });
        let generation = self.setup_generation;
        let sql = args["sql"].as_str().unwrap().to_owned();
        let params = args["params"].as_array().unwrap();
        ensure!(params.len() <= 100, "at most 100 SQL parameters");
        let params: Vec<SqlValue> = params
            .iter()
            .map(|v| {
                Ok(match v {
                    Value::Null => SqlValue::Null,
                    Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            SqlValue::Integer(i)
                        } else {
                            SqlValue::Real(
                                n.as_f64()
                                    .ok_or_else(|| anyhow::anyhow!("invalid number"))?,
                            )
                        }
                    }
                    Value::String(s) => SqlValue::Text(s.clone()),
                    _ => anyhow::bail!("SQL params must be scalars"),
                })
            })
            .collect::<Result<_>>()?;
        let filter = self.secret_filter().await?;
        let path = self.store.data_dir.join("agent.sqlite");
        // Hold the core writer through the bounded agent transaction. Off/setup
        // generation changes serialize here, even if the caller is cancelled.
        let mut gate = self.store.begin_write().await?;
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sources WHERE id=$1 AND enabled AND ($2 IS NULL OR setup_generation=$2))")
            .bind(&source).bind(generation).fetch_one(&mut *gate).await?;
        ensure!(allowed, "source is off or setup superseded");
        let result = tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let flags = if setup {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            };
            let conn = Connection::open_with_flags(path, flags)?;
            conn.busy_timeout(std::time::Duration::from_secs(2))?;
            conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, 64 * 1024)?;
            conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 16 * 1024)?;
            conn.set_limit(Limit::SQLITE_LIMIT_COLUMN, 64)?;
            conn.set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 100)?;
            conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
            conn.execute_batch(if setup {
                "PRAGMA trusted_schema=OFF; BEGIN IMMEDIATE"
            } else {
                "PRAGMA trusted_schema=OFF; BEGIN"
            })?;
            let prefix = format!("s_{source}_");
            let own_prefix = prefix.clone();
            // SQLite emits internal sqlite_master edits for ordinary DDL.
            // writable_schema remains off and all PRAGMAs are denied.
            let mut own_ddl = false;
            conn.authorizer(Some(move |ctx: AuthContext<'_>| {
                use AuthAction::*;
                let own =
                    |name: &str| name.starts_with(&own_prefix) && name.len() > own_prefix.len();
                let schema = |name: &str| matches!(name, "sqlite_master" | "sqlite_schema");
                if matches!(ctx.action, CreateTable { table_name } if setup && own(table_name)) {
                    own_ddl = true;
                }
                let allow = ctx.accessor.is_none()
                    && ctx.database_name.is_none_or(|db| db == "main")
                    && match ctx.action {
                        Select | Recursive => true,
                        Read {
                            table_name,
                            column_name,
                        } => {
                            own(table_name)
                                || (setup
                                    && own_ddl
                                    && schema(table_name)
                                    && column_name.eq_ignore_ascii_case("rowid"))
                        }
                        Insert { table_name }
                        | Delete { table_name }
                        | Update { table_name, .. } => {
                            setup && (own(table_name) || schema(table_name))
                        }
                        CreateTable { table_name } | DropTable { table_name } => {
                            setup && own(table_name)
                        }
                        CreateIndex {
                            index_name,
                            table_name,
                        }
                        | DropIndex {
                            index_name,
                            table_name,
                        } => {
                            setup
                                && own(table_name)
                                && (own(index_name)
                                    || index_name
                                        .starts_with(&format!("sqlite_autoindex_{own_prefix}")))
                        }
                        Function { function_name } => matches!(
                            function_name.to_ascii_lowercase().as_str(),
                            "count"
                                | "sum"
                                | "avg"
                                | "min"
                                | "max"
                                | "total"
                                | "coalesce"
                                | "ifnull"
                                | "nullif"
                                | "length"
                                | "lower"
                                | "upper"
                                | "trim"
                                | "ltrim"
                                | "rtrim"
                                | "substr"
                                | "substring"
                                | "replace"
                                | "abs"
                                | "round"
                                | "like"
                                | "glob"
                                | "json"
                                | "json_extract"
                                | "json_array"
                                | "json_object"
                                | "json_type"
                                | "json_valid"
                                | "date"
                                | "datetime"
                                | "strftime"
                                | "unixepoch"
                                | "julianday"
                        ),
                        _ => false,
                    };
                if allow {
                    Authorization::Allow
                } else {
                    Authorization::Deny
                }
            }))?;
            let started = std::time::Instant::now();
            let mut ticks = 0;
            conn.progress_handler(
                1000,
                Some(move || {
                    ticks += 1;
                    ticks > 1000 || started.elapsed() > std::time::Duration::from_secs(2)
                }),
            )?;
            let result = (|| {
                let mut statement = conn.prepare(&sql)?;
                ensure!(setup || statement.readonly(), "window SQL is read-only");
                let columns: Vec<String> = statement
                    .column_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let mut rows = statement.query(rusqlite::params_from_iter(params))?;
                let mut values = vec![];
                let mut bytes = 0;
                while let Some(row) = rows.next()? {
                    ensure!(
                        values.len() < 200,
                        "SQL result exceeds 200 rows; narrow the query"
                    );
                    let value = (0..columns.len())
                        .map(|i| scalar(row.get_ref(i)?))
                        .collect::<Result<Vec<_>>>()?;
                    bytes += serde_json::to_vec(&value)?.len();
                    ensure!(
                        bytes <= 32 * 1024,
                        "SQL result exceeds 32 KiB; narrow the query"
                    );
                    values.push(value);
                }
                Ok::<_, anyhow::Error>(json!({"columns":columns,"rows":values,"context_only":true}))
            })();
            conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            let value = result?;
            ensure!(
                filter.scrub_json(&value) == value,
                "SQL result contains credentials"
            );
            // Reject computed secrets too, before the agent DB transaction commits.
            if setup {
                let mut total = 0;
                for name in table_names(&conn)?
                    .into_iter()
                    .filter(|n| n.starts_with(&prefix))
                {
                    ensure!(
                        !filter.contains_secret(&name),
                        "table name contains credentials"
                    );
                    let mut statement = conn.prepare(&format!("SELECT * FROM {}", quote(&name)))?;
                    let count = statement.column_count();
                    let mut rows = statement.query([])?;
                    while let Some(row) = rows.next()? {
                        total += 1;
                        ensure!(total <= 1000, "source tables exceed 1000 rows");
                        for i in 0..count {
                            let v = scalar(row.get_ref(i)?)?;
                            ensure!(
                                filter.scrub_json(&v) == v,
                                "table value contains credentials"
                            );
                        }
                    }
                }
            }
            conn.execute_batch("COMMIT")?;
            Ok::<_, anyhow::Error>(value)
        })
        .await?;
        self.check_source(args).await?;
        // SQLite/serde errors can echo SQL or values; no raw diagnostic leaves here.
        result.map_err(|_| crate::mcp::ToolFailure { message: "agent SQL denied, invalid, or exceeded its limits; use one statement on this source's ordinary tables".into(), retryable: false }.into())
    }
}
