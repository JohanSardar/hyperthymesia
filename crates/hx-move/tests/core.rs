use anyhow::{Result, ensure};
use hx_move::{
    core::{self, Cell},
    schema,
};
use hx_store::Store;
use std::collections::BTreeSet;

#[tokio::test]
async fn every_table_is_hashed_and_one_changed_byte_fails_verification() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let table = schema::tables()
        .into_iter()
        .find(|t| t.name == "device_tokens")
        .unwrap();
    let mut tx = store.begin_write().await?;
    core::insert(
        &mut tx,
        &table,
        &vec![
            Cell::Text("synthetic".into()),
            Cell::Text("a".into()),
            Cell::Text("[]".into()),
            Cell::Text("[]".into()),
            Cell::Integer(0),
        ],
    )
    .await?;
    tx.commit().await?;
    let hashes = core::inventory(&store).await?;
    let persistent: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'normalized_fts%' AND name<>'_sqlx_migrations'").fetch_all(&store.pool).await?;
    ensure!(
        persistent.into_iter().collect::<BTreeSet<_>>() == hashes.keys().cloned().collect(),
        "every application table needs a hash"
    );
    for table in schema::tables() {
        let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info($1)")
            .bind(table.name)
            .fetch_all(&store.pool)
            .await?;
        let expected: BTreeSet<_> = schema::target_table(&table)
            .columns
            .into_iter()
            .map(|c| c.0.to_owned())
            .collect();
        ensure!(
            columns.into_iter().collect::<BTreeSet<_>>() == expected,
            "{} has uncovered columns",
            table.name
        );
    }
    ensure!(hashes.values().all(|h| h.sha256.len() == 64));
    core::verify(&store, &hashes).await?;
    sqlx::query("UPDATE device_tokens SET name='b'")
        .execute(&store.pool)
        .await?;
    ensure!(
        core::verify(&store, &hashes).await.is_err(),
        "one changed byte must fail verify"
    );
    store.pool.close().await;
    Ok(())
}
