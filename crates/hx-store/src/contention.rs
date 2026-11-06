//! Retain paid output and lease work across transient SQLite writer contention.
use std::{future::Future, time::Duration};

pub fn is_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let Some(sqlx::Error::Database(db)) = cause.downcast_ref::<sqlx::Error>() else {
            return false;
        };
        db.code()
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| matches!(code & 255, 5 | 6))
    })
}
/// Cancellation still stops the retry when its owning process/task stops.
/// Only static operation names and wait durations are logged, never SQL/data.
pub async fn retry_busy<T, F, Fut>(operation: &'static str, mut attempt: F) -> anyhow::Result<T>
where
    F: FnMut(bool) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut waited = false;
    let mut delay = Duration::from_millis(10);
    loop {
        match attempt(waited).await {
            Err(error) if is_busy(&error) => {
                waited = true;
                tracing::warn!(
                    operation,
                    wait_ms = delay.as_millis() as u64,
                    "waiting for SQLite writer"
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(500));
            }
            result => return result,
        }
    }
}
