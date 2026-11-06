use crate::disclosure::{DisclosurePolicy, POLICY_VERSION};
use anyhow::Result;
use chrono::{Duration, Utc};
use hx_compile::Timeline;
use hx_interpret::{EvidenceItem, Provider};
use hx_jobs::{Deferred, Failure, Handler, Job};
use hx_schema::{Interval, Sensitivity};
use hx_store::Store;
use serde_json::json;

pub enum ProviderConfig {
    Stub,
    Fx,
    ClaudeCli { model: String },
}
impl ProviderConfig {
    /// Select `fx` for the tool agent or `claude` for the legacy CLI; otherwise use the stub.
    pub fn from_env() -> Self {
        match std::env::var("HX_PROVIDER").as_deref() {
            Ok("fx") => Self::Fx,
            Ok("claude") => Self::ClaudeCli {
                model: std::env::var("HX_MODEL").unwrap_or_else(|_| "sonnet".into()),
            },
            _ => Self::Stub,
        }
    }
}
pub enum AnyProvider {
    Stub(StubProvider),
    Fx,
    ClaudeCli(hx_interpret::providers::ClaudeCliProvider),
}
impl Provider for AnyProvider {
    fn tool_agent(&self) -> bool {
        matches!(self, Self::Fx)
    }
    fn name(&self) -> String {
        match self {
            Self::Fx => "fx".into(),
            Self::Stub(p) => p.name(),
            Self::ClaudeCli(p) => p.name(),
        }
    }
    async fn complete(
        &self,
        system: &str,
        user: &str,
    ) -> Result<String, hx_interpret::InterpretError> {
        match self {
            Self::Fx => Err(hx_interpret::InterpretError::Provider(
                "fx requires the server broker".into(),
            )),
            Self::Stub(p) => p.complete(system, user).await,
            Self::ClaudeCli(p) => p.complete(system, user).await,
        }
    }
}
pub fn coordinator_provider(config: ProviderConfig) -> AnyProvider {
    match config {
        ProviderConfig::Fx => AnyProvider::Fx,
        ProviderConfig::Stub => AnyProvider::Stub(StubProvider),
        ProviderConfig::ClaudeCli { model } => {
            AnyProvider::ClaudeCli(hx_interpret::providers::ClaudeCliProvider { model })
        }
    }
}
pub struct StubProvider;
impl Provider for StubProvider {
    fn name(&self) -> String {
        "stub".into()
    }
    async fn complete(
        &self,
        _system: &str,
        _user: &str,
    ) -> Result<String, hx_interpret::InterpretError> {
        Ok(
            json!({"episodes":[],"tasks":[],"summary":"Stub provider: no inferred activity."})
                .to_string(),
        )
    }
}
pub async fn compile_window(store: &Store, w: Interval) -> Result<(Timeline, i64)> {
    let generation = store.generation(w).await?;
    let pending = store.pending_normalization(w).await?;
    if pending > 0 {
        return Err(Deferred::NormalizationPending.into());
    }
    let cached: Option<serde_json::Value> = store
        .cached_timeline(w.start, generation, hx_compile::COMPILER_VERSION)
        .await?;
    if let Some(timeline) = cached {
        return Ok((serde_json::from_value(timeline)?, generation));
    }
    let observations = store.observations(w, true).await?;
    let timeline = hx_compile::compile(&observations, w);
    if !store.save_timeline(&timeline, generation).await? {
        return Err(Deferred::InputChanged.into());
    }
    Ok((timeline, generation))
}
/// Admission is based on the resolved in-window timeline and the same visible
/// projection fx uses. Lead-in context and hidden/raw media cannot buy a run.
pub(crate) async fn admit_visible_window(store: &Store, w: Interval) -> Result<bool> {
    let (timeline, generation) = compile_window(store, w).await?;
    let ids: std::collections::HashSet<_> = timeline
        .lanes
        .iter()
        .flat_map(|l| &l.intervals)
        .flat_map(|i| i.evidence.iter().copied())
        .chain(timeline.points.iter().map(|p| p.evidence))
        .chain(
            timeline
                .aggregates
                .iter()
                .flat_map(|a| a.evidence.iter().copied()),
        )
        .collect();
    let broker = crate::mcp::Broker::new(
        store.clone(),
        DisclosurePolicy::from_env()?,
        SensitiveEgress::from_env(),
        w,
    );
    for o in store.observations(w, true).await? {
        if ids.contains(&o.id)
            && !matches!(
                o.envelope.kind.as_str(),
                "collector.health" | "clock.calibration" | "presence.span"
            )
            && !broker.project(o.id, &o.envelope).await?.is_empty()
        {
            return Ok(true);
        }
    }
    if !store.finish_empty(w, generation).await? {
        return Err(Deferred::InputChanged.into());
    }
    Ok(false)
}
pub async fn interpret_window<P: Provider>(
    store: &Store,
    w: Interval,
    provider: &P,
) -> Result<bool> {
    interpret_window_key(
        store,
        w,
        provider,
        false,
        &format!(
            "manual:{}",
            hx_store::hash(format!("{}:{}", w.start, Utc::now()).as_bytes())
        ),
    )
    .await
}
async fn interpret_window_key<P: Provider>(
    store: &Store,
    w: Interval,
    provider: &P,
    require_settled: bool,
    output_key: &str,
) -> Result<bool> {
    if !admit_visible_window(store, w).await? {
        return Ok(false);
    }
    if provider.tool_agent() {
        return crate::fx::interpret_window_with_settle(
            store,
            w,
            output_key,
            &crate::fx::Config::from_env(),
            require_settled,
        )
        .await;
    }
    let guard = interpretation_guard(store, w).await?;
    let result = async {
        let PreparedInput {
            packet,
            manifest,
            generation,
            namespaces,
        } = prepare_input(store, w, &provider.name()).await?;
        if require_settled {
            ensure_settled(store, w, generation).await?;
        }
        if revision_exists(store, w, generation, &manifest_identity(&manifest)).await? {
            return Ok(false);
        }
        let interpretation = hx_interpret::interpret(&packet, provider)
            .await
            .map_err(|error| {
                let category = match &error {
                    hx_interpret::InterpretError::Provider(_) => Failure::Provider,
                    hx_interpret::InterpretError::Parse(_) => Failure::ModelOutput,
                };
                anyhow::Error::new(error).context(category)
            })?;
        let failures = hx_interpret::check(&interpretation, &packet);
        if !failures.is_empty() {
            return Err(Failure::EvidenceChecks.into());
        }
        let mut value = serde_json::to_value(interpretation)?;
        value["input_manifest"] = manifest;
        Ok(store
            .publish_revision_manifest(w, generation, value, output_key, &namespaces)
            .await?
            .1)
    }
    .await;
    drop(guard);
    result
}
pub(crate) fn self_generated(e: &hx_schema::Envelope) -> bool {
    e.connector_instance.as_str().starts_with("hyperthymesia:")
        || ["window_title", "title", "surface"].iter().any(|key| {
            e.payload[key].as_str().is_some_and(|v| {
                v.starts_with("Hyperthymesia")
                    || v.contains("127.0.0.1:7377")
                    || v.contains("localhost:7377")
            })
        })
}
pub struct PreparedInput {
    pub packet: hx_interpret::Packet,
    pub manifest: serde_json::Value,
    pub generation: i64,
    pub namespaces: Vec<String>,
}
/// How `sensitive` evidence leaves the server for the LLM. `restricted` evidence never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveEgress {
    /// Full text, like `normal` evidence.
    Full,
    /// Kind and time only; hx-interpret replaces the text.
    Redact,
}
impl SensitiveEgress {
    /// `HX_EGRESS_SENSITIVE=redact` withholds sensitive text; the default sends it in full.
    pub fn from_env() -> Self {
        match std::env::var("HX_EGRESS_SENSITIVE").as_deref() {
            Ok("redact") => Self::Redact,
            _ => Self::Full,
        }
    }
}
pub async fn prepare_input(
    store: &Store,
    w: Interval,
    provider_name: &str,
) -> Result<PreparedInput> {
    prepare_input_with(store, w, provider_name, SensitiveEgress::from_env()).await
}
pub async fn prepare_input_with(
    store: &Store,
    w: Interval,
    provider_name: &str,
    sensitive: SensitiveEgress,
) -> Result<PreparedInput> {
    prepare_input_with_policy(
        store,
        w,
        provider_name,
        sensitive,
        &DisclosurePolicy::from_env()?,
    )
    .await
}
/// Explicit owner policy for deterministic replay without process-global env changes.
pub async fn prepare_input_with_policy(
    store: &Store,
    w: Interval,
    provider_name: &str,
    sensitive: SensitiveEgress,
    disclosure: &DisclosurePolicy,
) -> Result<PreparedInput> {
    let (_, generation) = compile_window(store, w).await?;
    let expanded = Interval {
        start: w.start - Duration::minutes(30),
        end: w.end,
    };
    let pending = store.pending_normalization(expanded).await?;
    if pending > 0 {
        return Err(Deferred::NormalizationPending.into());
    }
    let observations = store.observations(expanded, true).await?;
    let all: Vec<EvidenceItem> = observations
        .iter()
        .filter_map(|o| {
            // Every stored observation passed Store::ingest's token scope and namespace
            // checks. Policy grants apply only inside that authenticated boundary.
            if matches!(o.envelope.kind.as_str(), "policy.gap" | "media.chunk") {
                return None;
            }
            let effective_class = disclosure.effective_class(&o.envelope);
            let class = match (effective_class, sensitive) {
                (Sensitivity::Normal, _) | (Sensitivity::Sensitive, SensitiveEgress::Full) => {
                    "normal"
                }
                (Sensitivity::Sensitive, SensitiveEgress::Redact) => "sensitive",
                (Sensitivity::Restricted, _) => return None,
            };
            (hx_schema::kinds::is_core(o.envelope.kind.as_str())
                || o.envelope
                    .connector_instance
                    .as_str()
                    .starts_with("companion:"))
            .then_some((o, class, effective_class))
        })
        .map(|(o, class, effective_class)| EvidenceItem {
            effective_class,
            id: o.id,
            at: o
                .envelope
                .time
                .source_start
                .unwrap_or(o.envelope.time.observed_at),
            kind: format!("{class}:{}", o.envelope.kind),
            text: json!({
                "connector_instance": o.envelope.connector_instance,
                "subject": o.envelope.subject,
                "resource": o.envelope.resource,
                "payload": o.envelope.payload,
            })
            .to_string(),
            self_generated: self_generated(&o.envelope),
        })
        .collect();
    // Stills are lineage sources for OCR text only; their own records never enter the packet.
    let (stills, mut evidence): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|e| e.kind.ends_with(":media.still"));
    evidence.extend(crate::ocr::derived_evidence_for_sources(store, &stills).await?);
    let broker = crate::mcp::Broker::new(store.clone(), disclosure.clone(), sensitive, w);
    let prior = broker.prior_context(w.start).await?;
    let entities: Vec<String> = prior["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .map(ToString::to_string)
        .collect();
    let open_tasks: Vec<hx_interpret::Task> = prior["revisions"]
        .as_array()
        .into_iter()
        .flatten()
        .take(1)
        .flat_map(|r| r["tasks"].as_array().into_iter().flatten())
        .filter_map(|v| serde_json::from_value::<hx_interpret::Task>(v.clone()).ok())
        .filter(|t| t.open)
        .collect();
    let evidence_ids: std::collections::HashSet<_> = evidence.iter().map(|e| e.id).collect();
    let mut namespaces: std::collections::BTreeSet<String> = observations
        .iter()
        .filter(|o| evidence_ids.contains(&o.id))
        .map(|o| o.envelope.connector_instance.to_string())
        .collect();
    // Include every provenance namespace used by prior context, even outside this window.
    for id in prior["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(prior["revisions"].as_array().into_iter().flatten())
        .flat_map(|v| v["evidence"].as_array().into_iter().flatten())
        .filter_map(serde_json::Value::as_i64)
    {
        if let Some(v) = store.evidence(id).await?
            && let Some(ns) = v["envelope"]["connector_instance"].as_str()
        {
            namespaces.insert(ns.to_owned());
        }
    }
    let namespaces: Vec<String> = namespaces.into_iter().collect();
    // Compile only the public evidence projection: captured strings never bypass egress policy.
    let safe_observations: Vec<_> = observations
        .into_iter()
        .filter(|o| evidence_ids.contains(&o.id))
        .collect();
    let mut safe_timeline = hx_compile::compile(&safe_observations, expanded);
    safe_timeline.window = w;
    let mut packet = hx_interpret::build_packet(&safe_timeline, &evidence, &entities, &open_tasks);
    packet.window = w;
    let timeline_hash = store
        .compiled_timeline_hash(w, generation)
        .await?
        .ok_or(Deferred::InputChanged)?;
    let manifest = json!({"window":w,"timeline_hash":timeline_hash,"overlap_window":expanded,"input_generation":generation,"compiler_version":hx_compile::COMPILER_VERSION,"normalizer_version":"normalize/v1","prompt_version":hx_interpret::PROMPT_VERSION,"provider":provider_name,"sensitive_egress":sensitive,"disclosure_policy":{"version":POLICY_VERSION,"digest":disclosure.digest(),"policy":disclosure},"packet":packet,"input_namespaces":namespaces,"prior_revision_ids":prior["revisions"].as_array().into_iter().flatten().take(1).filter(|_| !packet.open_tasks.is_empty()).filter_map(|r| r["revision_id"].as_i64()).collect::<Vec<_>>()});
    packet.input_digest = hx_store::hash(&serde_json::to_vec(&manifest)?);
    Ok(PreparedInput {
        packet,
        manifest,
        generation,
        namespaces,
    })
}
pub struct Runner<P> {
    pub store: Store,
    pub provider: P,
}
impl<P: Provider> Handler for Runner<P> {
    async fn handle(&self, job: &Job) -> Result<()> {
        match job.kind.as_str() {
            "pull" => {
                let recipe = job.payload["recipe"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("pull job requires recipe"))?;
                let report = crate::pull::run(&self.store, recipe, Utc::now()).await?;
                tracing::info!(recipe, ?report, "source pull finished");
            }
            "normalize" => {
                if let Some(id) = job.payload["event_id"].as_i64() {
                    self.store.normalize(id).await?;
                } else {
                    let w = serde_json::from_value(job.payload["window"].clone())?;
                    self.store.normalize_window(w).await?;
                }
            }
            "compile" => {
                let w: Interval = serde_json::from_value(job.payload["window"].clone())?;
                if !admit_visible_window(&self.store, w).await? {
                    return Ok(());
                }
                let (_, generation) = compile_window(&self.store, w).await?;
                let state = self.store.interpretation_state(w).await?;
                self.store
                    .schedule_interpretations(w, generation, &state, interpret_settle_s())
                    .await?;
            }
            "interpret" => {
                let w: Interval = serde_json::from_value(job.payload["window"].clone())?;
                let revision24 = job.payload["revision24"].as_bool().unwrap_or(false);
                let ready_at: chrono::DateTime<Utc> = self
                    .store
                    .interpretation_ready_at(w.start, revision24, interpret_settle_s() as f64)
                    .await?;
                if ready_at > Utc::now() {
                    return Err(Deferred::Until(ready_at).into());
                }
                let sweep_day = job
                    .payload
                    .get("sweep_day")
                    .cloned()
                    .map(serde_json::from_value::<chrono::NaiveDate>)
                    .transpose()?;
                if !admit_visible_window(&self.store, w).await? {
                    if let Some(day) = sweep_day {
                        self.store.skip_empty_daily_sweep(w, day).await?;
                        self.store.schedule_daily_sweep(Utc::now()).await?;
                    }
                    return Ok(());
                }
                let state = self.store.interpretation_state(w).await?;
                ensure_settled(&self.store, w, state.generation).await?;
                if revision24 {
                    // Jobs queued during the grace period may survive prolonged downtime.
                    if Utc::now() >= w.end + Duration::hours(48)
                        || state.revision24_finished
                        || state.canonical_revision.is_none()
                    {
                        return Ok(());
                    }
                    if !state.timeline_changed {
                        self.store.finish_revision24(w, state.generation).await?;
                        return Ok(());
                    }
                } else if let Some(day) = sweep_day {
                    // Rescues without a canonical revision spend the same daily start.
                    if !self.store.start_daily_sweep(w, day).await? {
                        return Ok(());
                    }
                } else if state.canonical_revision.is_some()
                    || Utc::now() >= w.end + Duration::hours(24)
                {
                    // Old first-run jobs, including pre-upgrade retries, cannot
                    // spend outside the capped sweep. Explicit calls bypass this handler.
                    return Ok(());
                }
                let result = interpret_window_key(
                    &self.store,
                    w,
                    &self.provider,
                    true,
                    &format!("job:{}:{}", job.id, ulid::Ulid::generate()),
                )
                .await;
                if let (Some(day), Err(error)) = (sweep_day, &result)
                    && matches!(
                        error.downcast_ref::<Deferred>(),
                        Some(
                            Deferred::ProviderBusy
                                | Deferred::Until(_)
                                | Deferred::NormalizationPending
                        )
                    )
                {
                    self.store.release_daily_sweep(w, day).await?;
                }
                result?;
                if self.store.generation(w).await? != state.generation {
                    return Err(Deferred::InputChanged.into());
                }
                if revision24 {
                    self.store.finish_revision24(w, state.generation).await?;
                }
            }
            _ => return Err(Failure::UnsupportedJob.into()),
        }
        Ok(())
    }
}
pub async fn schedule(store: &Store) -> Result<()> {
    // Retry bounded rounds after a cooldown, including interpretations whose
    // compilation is already clean or whose previous canonical view is stale.
    hx_jobs::revive_window_jobs(store).await?;
    let current = hx_store::window_at(Utc::now() - Duration::hours(4));
    store.generation(current).await?;
    store.schedule_compilations().await?;
    store.schedule_daily_sweep(Utc::now()).await?;
    crate::pull::schedule(store, Utc::now()).await?;
    Ok(())
}

fn interpret_settle_s() -> i64 {
    std::env::var("HX_INTERPRET_SETTLE_S")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|s| *s >= 0)
        .unwrap_or(600)
}

// A file lock under the data dir serializes manual runs and lease recovery as well
// as queue workers. Dropping/cancelling the future releases it, including on crashes.
pub(crate) async fn interpretation_guard(
    store: &Store,
    window: Interval,
) -> Result<hx_store::InterpretationGuard> {
    store
        .try_interpretation_guard(window)
        .await?
        .ok_or_else(|| Deferred::ProviderBusy.into())
}

pub(crate) fn manifest_identity(manifest: &serde_json::Value) -> serde_json::Value {
    json!({"input_generation":manifest["input_generation"],
        "prompt_version":manifest["prompt_version"],"provider":manifest["provider"],
        "sensitive_egress":manifest["sensitive_egress"],
        "disclosure_policy":{"digest":manifest["disclosure_policy"]["digest"]}})
}

pub(crate) async fn revision_exists(
    store: &Store,
    window: Interval,
    generation: i64,
    identity: &serde_json::Value,
) -> Result<bool> {
    store
        .revision_exists(window.start, generation, identity)
        .await
}

// Recheck after preparing input: ingest/OCR may have raced the queue readiness
// check. Never send a newly prepared generation before its own settle period.
pub(crate) async fn ensure_settled(store: &Store, w: Interval, generation: i64) -> Result<()> {
    let row = store
        .settled_generation(w.start, interpret_settle_s() as f64)
        .await?;
    if row.generation != generation {
        return Err(Deferred::InputChanged.into());
    }
    let ready_at = row.ready_at;
    if ready_at > Utc::now() {
        return Err(Deferred::Until(ready_at).into());
    }
    Ok(())
}
