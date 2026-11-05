//! Durable server-only OCR worker. One OS file lock bounds expensive work across
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

pub async fn worker(store: Store) -> Result<()> {
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
