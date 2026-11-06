//! Deterministic pulls. Only the single queue worker calls this module: pulling
//! never launches a model and cannot interrupt a paid window run in that worker.
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use hx_sources::{
    host::HostRunner,
    keys::KeyStore,
    recipe::{MAX_ITEMS_PER_PULL, MapContext, PullCounts, PullMapper, Recipe},
    request::{MAX_TIMEOUT, Placeholders, RequestError},
    scrub::Scrubber,
};
use hx_store::{
    Store, Token, UploadBatch,
    sources::{RecipeState, Source, StoredRecipe},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_CURSOR_BYTES: usize = 16 * 1024;

#[derive(Debug, Default)]
pub struct PullReport {
    pub counts: PullCounts,
    pub accepted: u32,
    pub duplicates: u32,
    pub rejected: usize,
    pub stopped: bool,
    pub error: Option<String>,
}

/// A partial response is bounded by the transport's response limit. Keeping the
/// scrubbed remainder avoids re-fetching and slicing a changing page by offset.
/// This wrapper belongs to the runner, never to the remote API.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    pull_checkpoint_v1: bool,
    /// Incremental high-water mark, independent of a paginated traversal.
    cursor: Option<Value>,
    /// Present in new checkpoints, including JSON null when traversal is done.
    #[serde(default)]
    page_cursor: Option<Value>,
    pending: Option<Value>,
    pulled_at: Option<DateTime<Utc>>,
}
impl Checkpoint {
    fn load(cursor: Option<Value>, recipe: &Recipe) -> Result<Self, &'static str> {
        match cursor {
            Some(value) if value.get("pull_checkpoint_v1").is_some() => {
                let legacy = value.get("page_cursor").is_none();
                let mut checkpoint: Self =
                    serde_json::from_value(value).map_err(|_| "invalid pull checkpoint")?;
                if legacy && recipe.cursor.is_none() {
                    // Legacy snapshots conflated page and incremental tokens.
                    // Retain an already-fetched remainder; otherwise replay
                    // from the start rather than risk missing page-one edits.
                    if checkpoint.pending.is_some() {
                        checkpoint.page_cursor = checkpoint.cursor.take();
                    } else {
                        checkpoint.cursor = None;
                    }
                }
                Ok(checkpoint)
            }
            cursor => Ok(Self {
                cursor,
                ..Self::default()
            }),
        }
    }
    fn value(&self) -> Value {
        serde_json::to_value(self).expect("checkpoint serializes")
    }
}

pub async fn schedule(store: &Store, now: DateTime<Utc>) -> Result<()> {
    store.schedule_pulls(now).await
}

async fn enabled(store: &Store, id: &str) -> Result<Option<(StoredRecipe, Source)>> {
    let Some(recipe) = store.recipe(id).await? else {
        return Ok(None);
    };
    if !recipe.enabled || recipe.owner_off {
        return Ok(None);
    }
    let Some(source) = store.source(&recipe.source_id).await? else {
        return Ok(None);
    };
    Ok((source.enabled && source.status == "ready").then_some((recipe, source)))
}

/// Ingest is committed before every cursor checkpoint. Storage failures bubble
/// out without advancing the cadence, so lease recovery replays as duplicates.
/// Remote/recipe failures are recorded and wait until the next boundary.
pub async fn run(store: &Store, id: &str, now: DateTime<Utc>) -> Result<PullReport> {
    let mut report = PullReport::default();
    let Some((stored, source)) = enabled(store, id).await? else {
        return Ok(report);
    };
    let mut state = store.recipe_state(id).await?;
    if state.next_run_at > now {
        return Ok(report);
    }
    let recipe = &stored.recipe.0;
    if recipe.request.authorize(&source.description).is_err() {
        return failed(
            store,
            &source.id,
            &mut state,
            now,
            report,
            "recipe destination is not authorized",
            false,
        )
        .await;
    }
    let mut checkpoint = match Checkpoint::load(state.cursor.clone(), recipe) {
        Ok(checkpoint) => checkpoint,
        Err(error) => {
            return failed(store, &source.id, &mut state, now, report, error, false).await;
        }
    };
    let pulled_at = checkpoint.pulled_at.unwrap_or(now);
    let context = MapContext {
        recipe_id: id,
        source_id: &source.id,
        connected_at: source.connected_at,
        history_days: source
            .history_days
            .try_into()
            .context("invalid source history limit")?,
        pulled_at,
    };
    let mut mapper = match PullMapper::new(recipe, context) {
        Ok(mapper) => mapper,
        Err(error) => {
            return failed(
                store,
                &source.id,
                &mut state,
                now,
                report,
                &error.to_string(),
                false,
            )
            .await;
        }
    };
    // CLI runs outside state; its env supplies any persistent source config.
    let work = tempfile::tempdir()?;
    let host = HostRunner::from_env(store.data_dir.clone(), work.path().to_owned());
    loop {
        // Never consult a key for an off source/recipe, even from a stale job.
        if !unchanged(store, id, recipe).await? {
            report.stopped = true;
            break;
        }
        let sources = store.sources().await?;
        if !unchanged(store, id, recipe).await? {
            report.stopped = true;
            break;
        }
        let loaded = (|| {
            let keys = KeyStore::new(&store.data_dir)?;
            let mut request_key = None;
            let mut secrets = Vec::new();
            // Disabled sources must not be called, but their saved credentials
            // can still occur in other sources' responses and need redaction.
            for configured in &sources {
                if let Some(secret) = keys.read(&configured.id)? {
                    if configured.id == source.id {
                        request_key = Some(secret.clone());
                    }
                    secrets.push(secret);
                }
            }
            Ok::<_, hx_sources::keys::KeyError>((request_key, secrets))
        })();
        let (key, secrets) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                report.counts = mapper.counts();
                return failed(
                    store,
                    &source.id,
                    &mut state,
                    now,
                    report,
                    &error.to_string(),
                    false,
                )
                .await;
            }
        };
        let scrubber = match Scrubber::new(&secrets) {
            Ok(scrubber) => scrubber,
            Err(error) => {
                report.counts = mapper.counts();
                return failed(
                    store,
                    &source.id,
                    &mut state,
                    now,
                    report,
                    &error.to_string(),
                    false,
                )
                .await;
            }
        };
        if scrubber.scrub_json(&serde_json::to_value(recipe)?) != serde_json::to_value(recipe)? {
            return failed(
                store,
                &source.id,
                &mut state,
                now,
                report,
                "recipe contains credentials or invalid control text",
                false,
            )
            .await;
        }
        let cursor = match cursor_text(
            checkpoint
                .page_cursor
                .as_ref()
                .or(checkpoint.cursor.as_ref()),
        ) {
            Ok(cursor) => cursor,
            Err(error) => {
                report.counts = mapper.counts();
                return failed(store, &source.id, &mut state, now, report, error, false).await;
            }
        };
        let response = if let Some(pending) = checkpoint.pending.take() {
            pending
        } else {
            match recipe
                .request
                .execute(
                    Placeholders {
                        key: key.as_deref(),
                        cursor: cursor.as_deref(),
                    },
                    MAX_TIMEOUT,
                    &host,
                )
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let reconnect = error.needs_reconnect()
                        || matches!(error, RequestError::Exit(code) if recipe.reconnect_exit_codes.contains(&code))
                        || matches!(error, RequestError::MissingKey);
                    report.counts = mapper.counts();
                    return failed(
                        store,
                        &source.id,
                        &mut state,
                        now,
                        report,
                        &error.to_string(),
                        reconnect,
                    )
                    .await;
                }
            }
        };
        // An off during I/O also prevents storing its in-flight response.
        if !unchanged(store, id, recipe).await? {
            report.stopped = true;
            break;
        }
        let mut response = scrub_json(&response, &scrubber);
        let mapped = match mapper.map_page(&response) {
            Ok(page) => page,
            Err(error) => {
                report.counts = mapper.counts();
                return failed(
                    store,
                    &source.id,
                    &mut state,
                    now,
                    report,
                    &error.to_string(),
                    false,
                )
                .await;
            }
        };
        let next = recipe
            .pages
            .as_ref()
            .and_then(|pages| response.pointer(&pages.next))
            .filter(|v| !v.is_null() && v.as_str() != Some("") && **v != Value::Bool(false))
            .cloned();
        if mapped.unprocessed_items {
            let consumed = MAX_ITEMS_PER_PULL - report.counts.items_seen;
            let items = response
                .pointer_mut(&recipe.items)
                .and_then(Value::as_array_mut)
                .expect("mapper checked items array");
            items.drain(..consumed);
            checkpoint.pending = Some(response);
            checkpoint.pulled_at = Some(pulled_at);
        } else {
            checkpoint.page_cursor = next;
            checkpoint.cursor = if recipe.cursor.is_some() {
                mapped.cursor.or(checkpoint.cursor)
            } else {
                None
            };
            checkpoint.pulled_at = None;
        }
        checkpoint.pull_checkpoint_v1 = true;
        let ack = store
            .ingest(
                UploadBatch {
                    batch_id: ulid::Ulid::generate(),
                    envelopes: mapped.envelopes,
                },
                &Token {
                    prefixes: vec![format!("source:{}", source.id)],
                    scopes: vec!["ingest".into()],
                },
            )
            .await?;
        report.accepted += ack.accepted;
        report.duplicates += ack.duplicates;
        report.rejected += ack.rejected.len();
        report.counts = mapper.counts();
        state.cursor = Some(checkpoint.value());
        store.save_recipe_state(&state).await?;
        if mapped.next_page.is_none() {
            break;
        }
    }
    report.counts = mapper.counts();
    if !report.stopped {
        state.last_ok_at = Some(now);
        state.last_error = None;
        state.next_run_at = hx_store::window_at(now.max(Utc::now())).end;
        store
            .finish_recipe_pull(&source.id, &state, now, false)
            .await?;
    }
    Ok(report)
}

async fn unchanged(store: &Store, id: &str, recipe: &Recipe) -> Result<bool> {
    Ok(enabled(store, id)
        .await?
        .is_some_and(|(stored, _)| stored.recipe.0 == *recipe))
}

async fn failed(
    store: &Store,
    source: &str,
    state: &mut RecipeState,
    now: DateTime<Utc>,
    mut report: PullReport,
    error: &str,
    reconnect: bool,
) -> Result<PullReport> {
    // All error strings here come from fixed, secret-free transport/key/mapper
    // classifications. Never persist provider bodies, argv or underlying errors.
    state.last_error = Some(error.to_owned());
    state.next_run_at = hx_store::window_at(now.max(Utc::now())).end;
    store
        .finish_recipe_pull(source, state, now, reconnect)
        .await?;
    report.error = Some(error.to_owned());
    Ok(report)
}

fn cursor_text(value: Option<&Value>) -> Result<Option<String>, &'static str> {
    let text = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err("cursor must be a string or number"),
    };
    if text.len() > MAX_CURSOR_BYTES || text.contains('\0') {
        return Err("cursor exceeds limit or contains NUL");
    }
    Ok(Some(text))
}

fn scrub_json(value: &Value, scrubber: &Scrubber) -> Value {
    scrubber.scrub_json(value)
}
