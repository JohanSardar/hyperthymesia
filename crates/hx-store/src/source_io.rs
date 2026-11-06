//! Owner switches cancel active source I/O, then wait for its futures (and
//! process-group guards) to drop before returning success. Register before the
//! first database check, so racing registration cannot miss an off commit.
use crate::{Store, sources::recipe_identity};
use hx_sources::{
    host::HostRunner,
    recipe::Recipe,
    request::{Placeholders, RequestError, RequestGate},
};
use serde_json::Value;
use std::{path::PathBuf, sync::Mutex, time::Duration};
use tokio::sync::watch;

struct Active {
    state: PathBuf,
    source: String,
    identity: String,
    pull_id: Option<String>,
    cancel: watch::Sender<bool>,
}
static ACTIVE: Mutex<Vec<Active>> = Mutex::new(Vec::new());

pub struct SourceExchange {
    store: Store,
    source: String,
    identity: String,
    recipe: Recipe,
    pull_id: Option<String>,
    setup_generation: Option<i64>,
    cancelled: watch::Receiver<bool>,
}

impl Store {
    /// `pull_id` requires an enabled, unchanged saved pull. A setup preview can
    /// test an unsaved recipe, but can never test any owner-disabled identity.
    pub fn recipe_exchange(
        &self,
        source: &str,
        recipe: &Recipe,
        pull_id: Option<&str>,
        setup_generation: Option<i64>,
    ) -> anyhow::Result<SourceExchange> {
        let identity = recipe_identity(recipe)?;
        let (cancel, cancelled) = watch::channel(false);
        let mut active = ACTIVE.lock().expect("source exchanges lock");
        active.retain(|entry| !entry.cancel.is_closed());
        active.push(Active {
            state: std::fs::canonicalize(&self.data_dir)?,
            source: source.to_owned(),
            identity: identity.clone(),
            pull_id: pull_id.map(str::to_owned),
            cancel,
        });
        Ok(SourceExchange {
            store: self.clone(),
            source: source.to_owned(),
            identity,
            recipe: recipe.clone(),
            pull_id: pull_id.map(str::to_owned),
            setup_generation,
            cancelled,
        })
    }

    pub(crate) async fn cancel_source_exchanges(&self, source: &str, recipe: Option<(&str, &str)>) {
        let state = std::fs::canonicalize(&self.data_dir).unwrap_or_else(|_| self.data_dir.clone());
        let cancelled: Vec<_> = {
            let mut active = ACTIVE.lock().expect("source exchanges lock");
            active.retain(|entry| !entry.cancel.is_closed());
            active
                .iter()
                .filter(|entry| {
                    entry.state == state
                        && entry.source == source
                        && recipe.is_none_or(|(id, identity)| {
                            entry.identity == identity || entry.pull_id.as_deref() == Some(id)
                        })
                })
                .map(|entry| {
                    entry.cancel.send_replace(true);
                    entry.cancel.clone()
                })
                .collect()
        };
        // No database transaction or registry lock is held while the exchange
        // unwinds. Dropping a transport future kills its child process group.
        for cancel in cancelled {
            cancel.closed().await;
        }
    }
}

impl RequestGate for SourceExchange {
    async fn check(&self) -> Result<(), RequestError> {
        self.check_state()
            .await
            .map_err(|_| RequestError::Cancelled)
    }
}

impl SourceExchange {
    async fn check_state(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!*self.cancelled.borrow(), "source request cancelled");
        let source = self.store.source(&self.source).await?;
        anyhow::ensure!(
            source.is_some_and(|source| source.enabled
                && (self.pull_id.is_none() || source.status == "ready")
                && self
                    .setup_generation
                    .is_none_or(|generation| source.setup_generation == generation)),
            "source request cancelled"
        );
        let recipes = self.store.recipes(Some(&self.source)).await?;
        for saved in &recipes {
            anyhow::ensure!(
                !(saved.owner_off && recipe_identity(&saved.recipe.0)? == self.identity),
                "pull is off; only the owner can enable it"
            );
        }
        if let Some(id) = &self.pull_id {
            anyhow::ensure!(
                recipes.iter().any(|saved| saved.id == *id
                    && saved.enabled
                    && !saved.owner_off
                    && saved.recipe.0 == self.recipe),
                "pull changed or disabled"
            );
        }
        Ok(())
    }

    /// Cancellation races the *whole* exchange, including response bodies and
    /// child processes. The gate additionally checks SQLite before every later
    /// HTTP request / stdio message. Polling catches switches from other opens
    /// or processes while I/O is blocked; in-process switches signal immediately.
    pub async fn execute(
        self,
        values: Placeholders<'_>,
        timeout: Duration,
        host: &HostRunner,
    ) -> Result<Value, RequestError> {
        let mut cancelled = self.cancelled.clone();
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|off| *off) => Err(RequestError::Cancelled),
            _ = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if self.check().await.is_err() { break; }
                }
            } => Err(RequestError::Cancelled),
            result = self.recipe.request.execute_guarded(values, timeout, host, &self) => result,
        }
    }
}
