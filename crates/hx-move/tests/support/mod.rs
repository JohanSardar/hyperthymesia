use anyhow::{Result, ensure};
use futures_util::FutureExt;
use sqlx::{Connection, Executor, PgConnection, postgres::PgConnectOptions};
use std::{
    future::Future,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

pub async fn with_database<F, Fut>(test: F) -> Result<()>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let url = std::env::var("HX_MOVE_TEST_POSTGRES")
        .unwrap_or_else(|_| "postgres://hx:hx@localhost:55432/hx".into());
    let base = PgConnectOptions::from_str(&url)?;
    ensure!(
        base.get_database() == Some("hx"),
        "test admin database must be hx"
    );
    let mut admin = PgConnection::connect_with(&base).await?;
    let name = format!(
        "hx_move_{}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await?;
    let scratch = format!(
        "{}/{}",
        url.rsplit_once('/')
            .ok_or_else(|| anyhow::anyhow!("test URL"))?
            .0,
        name
    );
    let result = std::panic::AssertUnwindSafe(test(scratch))
        .catch_unwind()
        .await;
    admin
        .execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {name} WITH (FORCE)"
        )))
        .await?;
    admin.close().await?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
