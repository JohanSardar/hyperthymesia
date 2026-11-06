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
    cancel: watch::Sender<bool>,
    deadline: Option<tokio::time::Instant>,
    live: bool,
    command: Option<String>,
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
            cancel: cancel.clone(),
        });
        Ok(SourceExchange {
            store: self.clone(),
            source: source.to_owned(),
            identity,
            recipe: recipe.clone(),
            pull_id: pull_id.map(str::to_owned),
            setup_generation,
            cancelled,
            cancel,
            deadline: None,
            live: false,
            command: None,
        })
    }

    /// Live lookups share the same off-switch registry and refresh lifecycle as
    /// pulls. The synthetic recipe is never saved, mapped or scheduled.
    pub fn live_exchange(
        &self,
        source: &str,
        request: &hx_sources::request::Request,
        generation: Option<i64>,
    ) -> anyhow::Result<SourceExchange> {
        let recipe = Recipe {
            source: source.into(),
            name: "live".into(),
            request: request.clone(),
            items: String::new(),
            kind: "source.live".into(),
            map: Default::default(),
            time_format: hx_sources::recipe::TimeFormat::Rfc3339,
            cursor: None,
            pages: None,
            ignore: vec![],
            reconnect_exit_codes: vec![],
            detail: None,
            missing_items_empty: false,
        };
        let mut exchange = self.recipe_exchange(source, &recipe, None, generation)?;
        exchange.live = true;
        Ok(exchange)
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
            .map_err(|_| RequestError::Cancelled)?;
        if self
            .deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(RequestError::Transport);
        }
        Ok(())
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
        if self.live {
            return Ok(());
        }
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

    async fn page(
        &self,
        values: Placeholders<'_>,
        timeout: Duration,
        host: &HostRunner,
    ) -> Result<Value, RequestError> {
        let source = self
            .store
            .source(&self.source)
            .await
            .map_err(|_| RequestError::Cancelled)?
            .ok_or(RequestError::Cancelled)?;
        let since = source.connected_at - chrono::Duration::days(source.history_days);
        let until = chrono::Utc::now();
        let bind_dates = |request: &hx_sources::request::Request| {
            request
                .bind("since", &since.to_rfc3339())?
                .bind("since_unix", &since.timestamp().to_string())?
                .bind("until", &until.to_rfc3339())
        };
        let keys = hx_sources::keys::KeyStore::new(&self.store.data_dir)
            .map_err(|_| RequestError::MissingKey)?;
        if let Some(command) = &self.command {
            let key = keys
                .read(&self.source)
                .map_err(|_| RequestError::MissingKey)?;
            // Owner-bound CLIs and OAuth are structured-only: bash must never
            // bypass the pinned identity or expose an opaque credential bundle.
            if self
                .store
                .source_cli(&self.source)
                .await
                .map_err(|_| RequestError::Transport)?
                .is_some()
                || key
                    .as_deref()
                    .map(hx_sources::oauth::Credential::parse)
                    .transpose()?
                    .flatten()
                    .is_some()
            {
                return Err(RequestError::CliInvalid);
            }
            let env = key
                .map(|key| std::collections::BTreeMap::from([("SOURCE_KEY".into(), key)]))
                .unwrap_or_default();
            self.check().await?;
            let output = tokio::select! {
                biased;
                error = self.cancelled() => return Err(error),
                output = host.run(command, &env, timeout) => output.map_err(|error| match error {
                    hx_sources::host::HostError::Blocked(reason) => RequestError::HostBlocked(reason),
                    hx_sources::host::HostError::Deadline => RequestError::HostDeadline,
                    hx_sources::host::HostError::OutputLimit => RequestError::HostOutputLimit,
                    hx_sources::host::HostError::Io(_) => RequestError::HostIo,
                })?,
            };
            return Ok(
                serde_json::json!({"exit_code":output.status.code(),"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)}),
            );
        }
        let request = bind_dates(&self.recipe.request)?;
        self.authorize(&request, &source.description).await?;
        let (request, host) = self
            .store
            .prepare_source_request(&self.source, &request, host)
            .await
            .map_err(|_| RequestError::CliInvalid)?;
        let mut response = hx_sources::oauth::execute(
            &keys,
            &self.source,
            &request,
            values.cursor,
            timeout,
            &host,
            self,
        )
        .await?;
        if !self.live {
            response = decode_response(&request, response);
        }
        if let Some(detail) = &self.recipe.detail {
            let request = bind_dates(&detail.request)?;
            self.authorize(&request, &source.description).await?;
            let mut bytes = serde_json::to_vec(&response)
                .map_err(|_| RequestError::Json)?
                .len();
            if let Some(items) = response
                .pointer_mut(&self.recipe.items)
                .and_then(Value::as_array_mut)
            {
                if items.len() > hx_sources::recipe::MAX_ITEMS_PER_PULL {
                    return Err(RequestError::TooLarge);
                }
                for item in items {
                    let id = item
                        .pointer(&detail.id)
                        .and_then(Value::as_str)
                        .filter(|id| id.len() <= 4096 && !id.contains("{{"))
                        .ok_or(RequestError::Invalid)?;
                    let request = request.bind("item", id)?;
                    self.authorize(&request, &source.description).await?;
                    let (request, detail_host) = self
                        .store
                        .prepare_source_request(&self.source, &request, &host)
                        .await
                        .map_err(|_| RequestError::CliInvalid)?;
                    let answer = hx_sources::oauth::execute(
                        &keys,
                        &self.source,
                        &request,
                        None,
                        timeout,
                        &detail_host,
                        self,
                    )
                    .await?;
                    let answer = decode_response(&request, answer);
                    let size = |value: &Value| {
                        serde_json::to_vec(value)
                            .map(|bytes| bytes.len())
                            .map_err(|_| RequestError::Json)
                    };
                    bytes = bytes.saturating_sub(size(item)?) + size(&answer)?;
                    if bytes > hx_sources::request::MAX_HTTP_RESPONSE_BYTES {
                        return Err(RequestError::TooLarge);
                    }
                    *item = answer;
                }
            }
        }
        Ok(response)
    }

    async fn authorize(
        &self,
        request: &hx_sources::request::Request,
        description: &str,
    ) -> Result<(), RequestError> {
        if let Some(cli) = self
            .store
            .source_cli(&self.source)
            .await
            .map_err(|_| RequestError::Transport)?
        {
            cli.pin(request).map(|_| ())
        } else {
            request.authorize(description)
        }
    }

    pub async fn execute_bash(
        mut self,
        command: String,
        timeout: Duration,
        host: &HostRunner,
    ) -> Result<Value, RequestError> {
        self.command = Some(command);
        self.execute(Placeholders::default(), timeout, host).await
    }

    /// Keep the registered exchange alive if its caller is dropped. Cancelling
    /// resource I/O is immediate; an already-dispatched refresh has its own
    /// bounded timeout and must durably save rotation before the switch returns.
    pub async fn execute(
        mut self,
        values: Placeholders<'_>,
        timeout: Duration,
        host: &HostRunner,
    ) -> Result<Value, RequestError> {
        struct CancelOnDrop(watch::Sender<bool>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.send_replace(true);
            }
        }
        let _cancel = CancelOnDrop(self.cancel.clone());
        self.deadline =
            Some(tokio::time::Instant::now() + timeout.min(hx_sources::request::MAX_TIMEOUT));
        let host = host.clone();
        let cursor = values.cursor.map(str::to_owned);
        tokio::spawn(async move {
            let result = self
                .page(
                    Placeholders {
                        key: None,
                        cursor: cursor.as_deref(),
                    },
                    timeout,
                    &host,
                )
                .await;
            self.finish(result).await
        })
        .await
        .map_err(|_| RequestError::Transport)?
    }

    async fn finish(&self, result: Result<Value, RequestError>) -> Result<Value, RequestError> {
        if let Err(error) = &result
            && self.pull_id.is_none()
            && (error.needs_reconnect()
                || matches!(error, RequestError::Exit(code) if self.recipe.reconnect_exit_codes.contains(code)))
        {
            self.store
                .record_source_pull(
                    &self.source,
                    chrono::Utc::now(),
                    true,
                    Some("Login expired or missing. Reconnect this source on Connect sources."),
                )
                .await
                .map_err(|_| RequestError::Transport)?;
        }
        result
    }
}

/// Both list and detail MCP tool results use the same bounded JSON data decoder.
fn decode_response(request: &hx_sources::request::Request, response: Value) -> Value {
    if !matches!(request, hx_sources::request::Request::Mcp { .. }) {
        return response;
    }
    response
        .get("structuredContent")
        .cloned()
        .or_else(|| {
            let content = response.get("content")?.as_array()?;
            if content.len() != 1 {
                return None;
            }
            serde_json::from_str(content[0].get("text")?.as_str()?).ok()
        })
        .unwrap_or(response)
}
