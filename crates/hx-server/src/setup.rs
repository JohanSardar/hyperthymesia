//! One durable, bounded setup job per owner request. No window scheduling here.
use crate::{
    fx,
    mcp::{Broker, RunMode},
    processing::SensitiveEgress,
};
use anyhow::{Result, ensure};
use chrono::Utc;
use hx_sources::{
    host::HostRunner,
    keys::KeyStore,
    recipe::{MapContext, PullMapper, Recipe},
    request::{MAX_TIMEOUT, Placeholders, RequestGate},
};
use hx_store::Store;
use serde_json::{Value, json};

pub const PROMPT: &str = include_str!("../prompts/setup-v1.txt");

pub async fn run(store: &Store, job: &hx_jobs::Job) -> Result<()> {
    let source = job.payload["source"]
        .as_str()
        .ok_or(hx_jobs::Failure::UnsupportedJob)?;
    let generation = job.payload["setup_generation"]
        .as_i64()
        .ok_or(hx_jobs::Failure::UnsupportedJob)?;
    let Some(s) = store.source(source).await? else {
        return Ok(());
    };
    let Some(run) = store.setup_run(source, generation).await? else {
        return Ok(());
    };
    if s.setup_generation != generation || run.status != "pending" {
        return Ok(());
    }
    if !s.enabled {
        store
            .pause_source_setup(
                source,
                generation,
                job.id,
                &job.owner,
                "Source is off. Turn it on and reconnect when ready. No model run was started.",
            )
            .await?;
        return Ok(());
    }
    if let Some(probe) = store.source_check(source).await? {
        let work = tempfile::tempdir()?;
        let host = HostRunner::from_env(store.data_dir.clone(), work.path().to_owned());
        let failed = store
            .live_exchange(source, &probe, Some(generation))?
            .execute(
                Placeholders::default(),
                std::time::Duration::from_secs(30),
                &host,
            )
            .await
            .is_err();
        if failed {
            let message = "Login unavailable. Sign in to the CLI using the configured HOME, then reconnect. No model run was started.";
            store
                .pause_source_setup(source, generation, job.id, &job.owner, message)
                .await?;
            return Ok(());
        }
    }
    let mut broker = Broker::new(
        store.clone(),
        crate::disclosure::current(store).await?,
        SensitiveEgress::Full,
        hx_store::window_at(Utc::now()),
    )
    .for_setup(source.to_owned());
    broker.setup_generation = Some(generation);
    let mut config = fx::Config::from_env();
    config.max_steps = config.max_steps.min(60);
    config.timeout = config.timeout.min(std::time::Duration::from_secs(600));
    let result = async {
        // Do not put source-controlled prose in the instruction hierarchy.
        // source_info supplies it through the same untrusted tool boundary as responses.
        let answer = fx::run(&config, &broker, &format!("Set up connected source {source}. Call source_info first. Setup generation {generation}. Return the setup result JSON specified in your system instructions.")).await?;
        let result: Value = serde_json::from_str(answer["final_output"].as_str().unwrap_or(""))?;
        let recipes = store.recipes(Some(source)).await?;
        let has_pull = !recipes.is_empty();
        let no_pull = result["no_pull"].as_str().is_some_and(|reason| !reason.trim().is_empty());
        ensure!(has_pull || no_pull, "setup saved no recipe and gave no no_pull reason");
        ensure!(!broker.audit.lock().await.exhausted, "setup exhausted tool budget");
        ensure!(store.source(source).await?.is_some_and(|s| s.status != "needs_reconnect"), "source needs reconnect");
        // Store only a small, scrubbed owner-facing result, never arbitrary provider output.
        let result = broker.secret_filter().await?.scrub_json(&json!({
            "summary":crate::mcp::bounded(result["summary"].as_str().unwrap_or("Setup completed"), 2000),
            "no_pull":result["no_pull"].as_str().map(|reason| crate::mcp::bounded(reason,2000)), "recipes":recipes.iter().map(|r| &r.id).collect::<Vec<_>>()
        }));
        store.finish_source_setup(source, generation, result, None).await?;
        Ok::<_,anyhow::Error>(())
    }.await;
    match result {
        Ok(()) => Ok(()),
        Err(_) => {
            if store
                .source(source)
                .await?
                .is_some_and(|s| s.status == "needs_reconnect")
            {
                let message = "Login expired or missing. Reconnect this source on Connect sources.";
                store
                    .finish_source_setup(source, generation, json!({}), Some(message))
                    .await?;
                store
                    .record_source_pull(source, Utc::now(), true, Some(message))
                    .await?;
                return Ok(());
            }
            // Launched runs never refund their claims. Only a deterministic
            // preflight failure above can return an unused attempt.
            if job.attempts >= 2 {
                store.finish_source_setup(source, generation, json!({}), Some("Setup could not finish in two attempts. Review the connection and retry when ready.")).await?;
            }
            Err(hx_jobs::Failure::Provider.into())
        }
    }
}

/// A SIGKILL on the final attempt cannot leave the UI permanently setting_up.
/// Reconcile terminal leases too, without reviving or creating any job.
pub async fn reconcile(store: &Store) -> Result<()> {
    let mut tx = store.begin_write().await?;
    sqlx::query("UPDATE setup_runs SET status='failed',last_error='Setup attempts exhausted; retry from the owner page.',finished_at=$1 WHERE status='pending' AND EXISTS(SELECT 1 FROM jobs j WHERE j.output_key='setup:' || setup_runs.source_id || ':' || setup_runs.generation AND (j.state='failed' OR (j.attempts>=j.max_attempts AND j.state='running' AND j.lease_until<$1)))")
        .bind(hx_store::Ts::now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE sources SET status='needs_attention',last_error='Setup attempts exhausted; retry from the owner page.' WHERE status='setting_up' AND EXISTS(SELECT 1 FROM setup_runs r WHERE r.source_id=sources.id AND r.generation=sources.setup_generation AND r.status='failed')")
        .execute(&mut *tx).await?;
    tx.commit().await
}

impl Broker {
    pub(crate) async fn setup_tool(&self, name: &str, args: &Value) -> Result<Value> {
        ensure!(matches!(self.mode(), RunMode::Setup { .. }), "setup only");
        self.check_source(args).await?;
        let source_id = args["source"].as_str().expect("validated source");
        let source = self
            .store
            .source(source_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("unknown source"))?;
        if name == "source_info" {
            let recipes = self.store.recipes(Some(source_id)).await?;
            let tables = crate::agent_sql::tables(&self.store, Some(source_id)).await?;
            return Ok(
                json!({"source":source,"recipes":recipes.iter().map(|r| json!({"id":r.id,"recipe":r.recipe.0,"enabled":r.enabled,"owner_off":r.owner_off})).collect::<Vec<_>>(),"addons":self.store.addons(Some(source_id)).await?,"tables":tables,"table_prefix":format!("s_{source_id}_"),"connection_check":self.store.source_check(source_id).await?,"cli_identity":self.store.source_cli(source_id).await?,"cadence_hours":4}),
            );
        }
        if name == "addon_save" {
            let addon = self
                .store
                .save_addon_for_setup(
                    source_id,
                    args["name"].as_str().unwrap(),
                    args["text"].as_str().unwrap(),
                    self.setup_generation,
                )
                .await?;
            return Ok(json!({"saved":addon}));
        }
        // Strict shape, kind, pointers, budgets, destination and secrets are
        // checked before either a test request or a persistent save.
        let recipe: Recipe = serde_json::from_value(args.clone())
            .map_err(|_| anyhow::anyhow!("invalid recipe shape"))?;
        recipe.validate()?;
        recipe.request.authorize(&source.description)?;
        if let Some(detail) = &recipe.detail {
            detail.request.authorize(&source.description)?;
        }
        ensure!(
            self.secret_filter().await?.scrub_json(args) == *args,
            "recipe contains credentials"
        );
        if name == "recipe_save" {
            let saved = self
                .store
                .save_recipe_for_setup(source_id, &recipe, self.setup_generation)
                .await?;
            return Ok(json!({"id":saved.id,"enabled":saved.enabled,"owner_off":saved.owner_off}));
        }
        let exchange =
            self.store
                .recipe_exchange(source_id, &recipe, None, self.setup_generation)?;
        exchange.check().await?;
        let key = KeyStore::new(&self.store.data_dir)?.read(source_id)?;
        let filter = self.secret_filter().await?;
        let work = tempfile::tempdir()?;
        let host = HostRunner::from_env(self.store.data_dir.clone(), work.path().to_owned());
        self.check_source(args).await?;
        let response = exchange
            .execute(
                Placeholders {
                    key: key.as_deref(),
                    cursor: None,
                },
                MAX_TIMEOUT,
                &host,
            )
            .await?;
        self.check_source(args).await?;
        let response = self
            .secret_filter()
            .await?
            .scrub_json(&filter.scrub_json(&response));
        let mut mapper = PullMapper::new(
            &recipe,
            MapContext {
                recipe_id: "preview",
                source_id,
                connected_at: source.connected_at,
                history_days: source.history_days.try_into()?,
                pulled_at: Utc::now(),
            },
        )?;
        let page = mapper.map_page(&response)?;
        let counts = mapper.counts();
        Ok(
            json!({"counts":{"seen":counts.items_seen,"mapped":counts.mapped,"bad_time":counts.bad_time,"too_old":counts.too_old,"too_future":counts.too_future,"too_large":counts.too_large,"long_span":counts.long_span},"sample":page.envelopes.iter().take(3).map(|e| json!({"source_key":e.source_key,"time":e.time,"payload_excerpt":crate::mcp::bounded(&e.payload.to_string(),4096)})).collect::<Vec<_>>(),"cursor":page.cursor,"next_page":page.next_page,"ingested":false}),
        )
    }
}

/// Only sources actually disclosed to a run join its add-on identity.
pub(crate) async fn active_addons(
    store: &Store,
    namespaces: &[String],
) -> Result<Vec<hx_store::sources::Addon>> {
    let grants = store.source_grants().await?;
    Ok(store
        .addons(None)
        .await?
        .into_iter()
        .filter(|a| {
            a.enabled
                && !a.owner_off
                && namespaces.contains(&format!("source:{}", a.source_id))
                && grants.contains(&format!("source:{}", a.source_id))
        })
        .collect())
}
impl Broker {
    pub(crate) async fn addon_context(&self, namespaces: &[String]) -> Result<Value> {
        if !matches!(self.mode(), RunMode::Window) {
            return Ok(json!([]));
        }
        for namespace in namespaces.iter().filter(|n| n.starts_with("source:")) {
            if self
                .audit
                .lock()
                .await
                .source_addons
                .contains_key(namespace)
            {
                continue;
            }
            let addons = active_addons(&self.store, std::slice::from_ref(namespace)).await?;
            self.audit
                .lock()
                .await
                .source_addons
                .entry(namespace.clone())
                .or_insert(addons);
        }
        let audit = self.audit.lock().await;
        Ok(json!(
            audit.source_addons.values().flatten().collect::<Vec<_>>()
        ))
    }
}
