//! Explicit transactions for callers that must group writes atomically.
use super::*;
pub struct StoreTransaction(pub(crate) sqlx::Transaction<'static, sqlx::Postgres>);
impl Store {
    pub async fn transaction(&self) -> Result<StoreTransaction> {
        Ok(StoreTransaction(self.pool.begin().await?))
    }
}
impl StoreTransaction {
    pub async fn commit(self) -> Result<()> {
        self.0.commit().await?;
        Ok(())
    }
    pub async fn rollback(self) -> Result<()> {
        self.0.rollback().await?;
        Ok(())
    }
    pub async fn enqueue(
        &mut self,
        key: &str,
        kind: &str,
        payload: Value,
        backfill: bool,
        at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO jobs(output_key,kind,payload,queue,available_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(output_key) DO NOTHING")
        .bind(key).bind(kind).bind(payload).bind(if backfill {"backfill"} else {"interactive"}).bind(at).execute(&mut *self.0).await?;
        Ok(())
    }
}
