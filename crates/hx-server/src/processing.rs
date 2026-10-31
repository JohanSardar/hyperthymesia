use crate::disclosure::{DisclosurePolicy, POLICY_VERSION};
use anyhow::Result;
use chrono::{Duration, Utc};
use hx_compile::Timeline;
use hx_interpret::{EvidenceItem, Provider};
use hx_jobs::{Deferred, Failure, Handler, Job};
use hx_schema::{Interval, Sensitivity};
use hx_store::Store;
use serde_json::json;
use sqlx::Row;

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
    let cached: Option<serde_json::Value> = sqlx::query_scalar("SELECT timeline FROM windows WHERE start_at=$1 AND generation=$2 AND compiled_generation=$2 AND compiler_version=$3 AND timeline IS NOT NULL")
        .bind(w.start).bind(generation).bind(hx_compile::COMPILER_VERSION).fetch_optional(&store.pool).await?;
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
pub async fn interpret_window<P: Provider>(
    store: &Store,
    w: Interval,
    provider: &P,
) -> Result<bool> {
    interpret_window_key(
        store,
        w,
        provider,
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
    output_key: &str,
) -> Result<bool> {
    if provider.tool_agent() {
        return crate::fx::interpret_window(store, w, output_key, &crate::fx::Config::from_env())
            .await;
    }
    let PreparedInput {
        packet,
        manifest,
        generation,
        namespaces,
    } = prepare_input(store, w, &provider.name()).await?;
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
    let manifest = json!({"window":w,"overlap_window":expanded,"input_generation":generation,"compiler_version":hx_compile::COMPILER_VERSION,"normalizer_version":"normalize/v1","prompt_version":hx_interpret::PROMPT_VERSION,"provider":provider_name,"sensitive_egress":sensitive,"disclosure_policy":{"version":POLICY_VERSION,"digest":disclosure.digest(),"policy":disclosure},"packet":packet,"input_namespaces":namespaces});
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
                let (_, generation) = compile_window(&self.store, w).await?;
                let mut tx = self.store.pool.begin().await?;
                hx_jobs::enqueue(
                    &mut tx,
                    &format!("interpret:{}:{generation}", w.start),
                    "interpret",
                    json!({"window":w}),
                    false,
                    w.end.max(Utc::now()),
                )
                .await?;
                hx_jobs::enqueue(
                    &mut tx,
                    &format!("revision24:{}:{generation}", w.start),
                    "interpret",
                    json!({"window":w}),
                    true,
                    (w.end + Duration::hours(24)).max(Utc::now()),
                )
                .await?;
                tx.commit().await?;
            }
            "interpret" => {
                let w = serde_json::from_value(job.payload["window"].clone())?;
                interpret_window_key(&self.store, w, &self.provider, &format!("job:{}", job.id))
                    .await?;
            }
            _ => return Err(Failure::UnsupportedJob.into()),
        }
        Ok(())
    }
}
pub async fn schedule(store: &Store) -> Result<()> {
    // Retry bounded rounds after a cooldown, including interpretations whose
    // compilation is already clean or whose previous canonical view is stale.
    hx_jobs::revive_window_jobs(&store.pool).await?;
    let current = hx_store::window_at(Utc::now() - Duration::hours(4));
    store.generation(current).await?;
    let rows=sqlx::query("SELECT start_at,end_at,generation FROM windows WHERE dirty OR (end_at<=now() AND canonical_revision IS NULL) ORDER BY start_at LIMIT 1000").fetch_all(&store.pool).await?;
    let mut tx = store.pool.begin().await?;
    for r in rows {
        let w = Interval {
            start: r.get("start_at"),
            end: r.get("end_at"),
        };
        let generation: i64 = r.get("generation");
        hx_store::request_window_job(&mut tx, "compile", w, generation).await?;
    }
    tx.commit().await?;
    Ok(())
}
