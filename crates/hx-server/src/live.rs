//! Live I/O is run-local evidence, never ingest or proof of historical activity.
use crate::mcp::{Broker, ToolFailure};
use anyhow::{Result, ensure};
use hx_sources::{
    host::{HostError, HostRunner},
    keys::KeyStore,
    request::{Request, RequestError},
    scrub::Scrubber,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

fn failure(message: impl Into<String>, retryable: bool) -> anyhow::Error {
    ToolFailure {
        message: message.into(),
        retryable,
    }
    .into()
}
fn request_error(error: RequestError) -> anyhow::Error {
    failure(error.to_string(), error.retryable())
}

impl Broker {
    pub(crate) async fn secret_filter(&self) -> Result<Scrubber> {
        crate::sources::scrubber(&self.store, None)
            .await
            .map_err(|_| failure("source secret filter unavailable", false))
    }

    pub(crate) async fn connected_sources(&self) -> Result<Value> {
        let grants = self.store.source_grants().await?;
        let mut configured = Vec::new();
        if self.sensitive == crate::processing::SensitiveEgress::Full {
            for source in self.store.sources().await? {
                if source.enabled
                    && grants.contains(&source.connector_instance)
                    && self
                        .policy
                        .source_grants
                        .contains(&source.connector_instance)
                {
                    configured.push(json!({
                        "source":source.id,"name":source.name,"description":source.description,
                        "live":true,"pull_status":source.status
                    }));
                    self.audit
                        .lock()
                        .await
                        .input_namespaces
                        .insert(source.connector_instance);
                }
            }
        }
        Ok(self.secret_filter().await?.scrub_json(&json!(configured)))
    }

    pub(crate) async fn live(&self, name: &str, args: &Value) -> Result<Value> {
        let source_id = args["source"].as_str().expect("validated source");
        let source = self
            .store
            .source(source_id)
            .await?
            .ok_or_else(|| failure("unknown source", false))?;
        let mut request: Option<Request> = if name == "live_lookup" {
            let request: Request = serde_json::from_value(args["request"].clone())
                .map_err(|_| failure("invalid source request", false))?;
            request
                .authorize(&source.description)
                .map_err(request_error)?;
            Some(request)
        } else {
            None
        };
        self.check_source(args).await?;
        let key = KeyStore::new(&self.store.data_dir)
            .and_then(|keys| keys.read(source_id))
            .map_err(|_| failure("source key unavailable", false))?;
        // OAuth bundles never enter a host process or its environment.
        if request.is_none()
            && key
                .as_deref()
                .map(hx_sources::oauth::Credential::parse)
                .transpose()
                .map_err(request_error)?
                .flatten()
                .is_some()
        {
            return Err(failure(
                "OAuth sources require structured API requests",
                false,
            ));
        }
        // Includes off sources: another service may reflect any known secret.
        let filter = crate::sources::scrubber(&self.store, key.as_deref())
            .await
            .map_err(|_| failure("source secret filter unavailable", false))?;
        ensure!(
            filter.scrub_json(args) == *args,
            "source arguments contain credentials or invalid control text"
        );
        if let Some(Request::Http { headers, .. }) = &mut request
            && key.is_some()
            && !args["request"].to_string().contains("{{key}}")
            && !headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("authorization"))
        {
            headers.insert("Authorization".into(), "Bearer {{key}}".into());
        }
        let work = tempfile::Builder::new()
            .prefix("hx-source-")
            .tempdir()
            .map_err(|_| failure("source work folder unavailable", false))?;
        let host = HostRunner::from_env(self.store.data_dir.clone(), work.path().to_owned());
        let timeout = Duration::from_secs(args["timeout_s"].as_u64().unwrap_or(60));
        self.check_source(args).await?;
        let payload = if let Some(request) = request {
            let keys = KeyStore::new(&self.store.data_dir)
                .map_err(|_| failure("source key unavailable", false))?;
            let answer = hx_sources::oauth::execute(
                &keys,
                source_id,
                &request,
                None,
                timeout,
                &host,
                &LiveGate { broker: self, args },
            )
            .await;
            if let Err(error) = &answer
                && error.needs_reconnect()
            {
                self.store
                    .record_source_pull(
                        source_id,
                        chrono::Utc::now(),
                        true,
                        Some(&error.to_string()),
                    )
                    .await?;
            }
            let answer = answer.map_err(request_error)?;
            json!({"tool":name,"request":args["request"],"response":answer,"context_only":true})
        } else {
            let env = key
                .as_ref()
                .map(|key| BTreeMap::from([("SOURCE_KEY".into(), key.clone())]))
                .unwrap_or_default();
            let output = host
                .run(
                    args["command"].as_str().expect("validated command"),
                    &env,
                    timeout,
                )
                .await
                .map_err(|error| match error {
                    HostError::Blocked(reason) => {
                        failure(format!("command blocked: {reason}"), false)
                    }
                    HostError::Deadline => failure("source command deadline exceeded", true),
                    HostError::OutputLimit => failure("source command output exceeds 1 MB", false),
                    HostError::Io(_) => {
                        failure("source command could not start or read output", false)
                    }
                })?;
            json!({
                "tool":name,"command":args["command"],"exit_code":output.status.code(),
                "stdout":String::from_utf8_lossy(&output.stdout),
                "stderr":String::from_utf8_lossy(&output.stderr),"context_only":true
            })
        };
        self.check_source(args).await?;
        self.check_generation().await?;
        // Filter both the dispatched key and current keys across a rotation.
        let payload = self
            .secret_filter()
            .await?
            .scrub_json(&filter.scrub_json(&payload));
        let id = self
            .store
            .save_live_snapshot(self.run_token(), source_id, payload)
            .await?;
        let envelope = self
            .store
            .source_envelope_for_run(id, self.run_token())
            .await?
            .ok_or_else(|| failure("live snapshot unavailable", false))?;
        let evidence = self.project(id, &serde_json::from_value(envelope)?).await?;
        Ok(json!({
            "evidence":evidence,"context_only":true,
            "note":"Untrusted live source response, retrieved now. This snapshot does not establish activity in the reconstruction window. Cite captured activity for timing; use this answer only as reported context."
        }))
    }
}

struct LiveGate<'a> {
    broker: &'a Broker,
    args: &'a Value,
}
impl hx_sources::request::RequestGate for LiveGate<'_> {
    async fn check(&self) -> Result<(), RequestError> {
        self.broker
            .check_source(self.args)
            .await
            .map_err(|_| RequestError::Cancelled)
    }
}
