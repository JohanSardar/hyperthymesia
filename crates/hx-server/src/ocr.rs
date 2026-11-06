//! Durable server/device OCR publication. One OS file lock bounds expensive work across
//! server processes; blocking image decoding and Vision never run on a Tokio thread.
//! Successful exact results are reusable; sampled frames carry no borrowed text.
use anyhow::{Context, Result};
use hx_interpret::EvidenceItem;
use hx_store::Store;
use hx_visual::{OcrEngine, OcrResult, VisionCliEngine};
use serde_json::json;
use std::sync::Arc;

pub use hx_visual::vision_version;

/// Discover both upload orderings and old stills. Called repeatedly, including on startup.
/// Missing/unpublished blobs do not spend attempts.
pub async fn discover(store: &Store, version: &str) -> Result<()> {
    store.discover_ocr(version).await
}

/// Run one durable job. The OS releases the file lock after process death. The
/// global try-lock also prevents a lease retry from overlapping a live OCR process.
pub async fn run_one<E: OcrEngine + 'static>(
    store: &Store,
    engine: Arc<E>,
    version: &str,
) -> Result<bool> {
    let Some(claim) = store.claim_ocr(version).await? else {
        return Ok(false);
    };
    let sha = claim.candidate.sha256.clone();
    let source = claim.candidate.envelope.clone();
    let previous = claim.previous(version).await?;
    let path = store.blob_path(&sha)?;
    let media = source["blobs"]
        .as_array()
        .and_then(|b| b.iter().find(|b| b["sha256"] == sha))
        .and_then(|b| b["media_type"].as_str())
        .unwrap_or("image/jpeg")
        .to_owned();
    // A blocking OCR task cannot be aborted. Move the claim into it so the OS
    // lock still excludes other workers if the async caller is cancelled.
    let (claim, result) = tokio::task::spawn_blocking(move || {
        let result = (|| -> Result<(u64, Option<String>, Option<OcrResult>)> {
            let bytes = std::fs::read(path)?;
            let hash = hx_visual::thumbnail_hash(&bytes)?;
            if let Some((sha, prior)) = previous
                && hx_visual::similar_thumbnail_hashes(hash, u64::from_str_radix(&prior, 16)?)
            {
                return Ok((hash, Some(sha), None));
            }
            Ok((hash, None, Some(engine.ocr(&bytes, &media)?)))
        })();
        (claim, result)
    })
    .await
    .context("OCR worker join")?;
    match result {
        Ok((hash, sampled, result)) => {
            claim
                .complete(
                    version,
                    hash,
                    sampled,
                    result.map(serde_json::to_value).transpose()?,
                )
                .await?;
        }
        Err(_) => {
            // Do not persist/log engine errors: they may contain captured text.
            claim.fail(version).await?;
        }
    }
    store.publish_ocr_lineage(version).await?;
    Ok(true)
}

/// Small disclosure-policy integration point: derived text receives the decision
/// of its source item *after* the server policy, including exclusion/redaction and
/// self-generated attribution. No OCR payload can grant itself disclosure.
pub async fn derived_evidence_for_sources(
    store: &Store,
    sources: &[EvidenceItem],
) -> Result<Vec<EvidenceItem>> {
    let ids: Vec<_> = sources.iter().map(|e| e.id).collect();
    let rows = store.ocr_evidence_for_sources(&ids).await?;
    let by_id: std::collections::HashMap<_, _> = sources.iter().map(|e| (e.id, e)).collect();
    let mut out = Vec::new();
    for row in rows {
        let source = by_id[&row.source_event_id];
        let result: OcrResult = serde_json::from_value(row.result)?;
        let text = result
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        // PacketBudget applies the final per-item and total serialized byte caps.
        let cap = hx_interpret::PacketBudget::default().max_item_chars;
        let text: String = text.chars().take(cap).collect();
        out.push(EvidenceItem {
            effective_class: source.effective_class,
            id: row.id, at: source.at,
            kind: format!("{}:ocr.text", source.kind.split(':').next().unwrap_or("sensitive")),
            text: json!({"source_event_id":source.id,"source_observation_id":row.observation_id,"sha256":row.sha256,"connector_instance":row.connector_instance,"engine_version":row.engine_version,"text":text,"coverage":result.coverage}).to_string(),
            self_generated: source.self_generated,
        });
    }
    Ok(out)
}

/// Helper capability, shared by negotiation and worker selection. A failed
/// resolution selects device OCR; a task/runtime failure remains retryable.
pub async fn mode() -> Result<hx_schema::protocol::OcrMode> {
    use hx_schema::protocol::OcrMode;
    let available =
        tokio::task::spawn_blocking(|| VisionCliEngine::default().executable().is_ok()).await?;
    Ok(if available {
        OcrMode::Server
    } else {
        OcrMode::Device
    })
}

/// Validate both the protocol envelope and the helper payload. The shared
/// OcrResult accepts extensions for other callers; the device boundary is strict
/// at every level, so a nested producer-supplied class is also rejected.
pub(crate) fn valid_device_request(request: &hx_schema::protocol::OcrRequest) -> bool {
    use hx_schema::protocol::OcrStatus;
    fn fields(value: &serde_json::Value, allowed: &[&str]) -> bool {
        value
            .as_object()
            .is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
    }
    fn unit(value: &serde_json::Value) -> bool {
        value.as_f64().is_some_and(|v| (0.0..=1.0).contains(&v))
    }
    let hash_valid = request
        .perceptual_hash
        .as_ref()
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    if !request.engine_version.starts_with("vision:") {
        return false;
    }
    match request.status {
        OcrStatus::Complete => {
            let Some(value) = &request.result else {
                return false;
            };
            let Ok(result) = serde_json::from_value::<OcrResult>(value.clone()) else {
                return false;
            };
            hash_valid
                && request.sampled_from.is_none()
                && result.coverage == "partial"
                && fields(
                    value,
                    &["lines", "engine_name", "engine_version", "coverage"],
                )
                && value["lines"].as_array().is_some_and(|lines| {
                    lines.iter().all(|line| {
                        fields(line, &["text", "confidence", "bbox"])
                            && unit(&line["confidence"])
                            && fields(&line["bbox"], &["x", "y", "width", "height"])
                            && ["x", "y", "width", "height"]
                                .iter()
                                .all(|k| unit(&line["bbox"][k]))
                    })
                })
        }
        OcrStatus::Sampled => {
            hash_valid
                && request.result.is_none()
                && request
                    .sampled_from
                    .as_deref()
                    .is_some_and(hx_store::valid_hash)
        }
        OcrStatus::Failed => {
            request.result.is_none()
                && request.sampled_from.is_none()
                && request.perceptual_hash.is_none()
        }
    }
}

pub async fn worker(store: Store) -> Result<()> {
    worker_in_mode(store, mode().await?).await
}

pub async fn worker_in_mode(store: Store, mode: hx_schema::protocol::OcrMode) -> Result<()> {
    if mode == hx_schema::protocol::OcrMode::Device {
        loop {
            if store.publish_device_ocr_lineage().await.is_err() {
                tracing::error!("device OCR lineage publication failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    let version = vision_version()?;
    let engine = Arc::new(VisionCliEngine::default());
    loop {
        if discover(&store, &version).await.is_err() {
            tracing::error!("OCR discovery failed");
        }
        match run_one(&store, engine.clone(), &version).await {
            Ok(true) => continue,
            Err(_) => tracing::error!("OCR job failed"),
            Ok(false) => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn vision_engine_id_preserves_existing_owner_cache_key() {
        // Freeze the pre-N2 algorithm independently of the shared implementation.
        let os = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-buildVersion")
            .output()
            .unwrap();
        assert!(os.status.success());
        let legacy = format!(
            "vision:{}:{}",
            hx_store::hash(include_bytes!("../../hx-visual/swift/ocr.swift")),
            String::from_utf8(os.stdout).unwrap().trim()
        );
        assert_eq!(super::vision_version().unwrap(), legacy);
    }
}
