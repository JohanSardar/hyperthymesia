//! Durable server-only OCR worker. One advisory lock bounds expensive work across
//! server processes; blocking image decoding and Vision never run on a Tokio thread.
//! Successful exact results are reusable; sampled frames carry no borrowed text.
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use hx_interpret::EvidenceItem;
use hx_store::Store;
use hx_visual::{OcrEngine, OcrResult, VisionCliEngine};
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;

/// Includes the helper source/config and OS build: Vision behavior can change with macOS.
pub fn vision_version() -> Result<String> {
    anyhow::ensure!(cfg!(target_os = "macos"), "Vision requires macOS");
    let os = std::process::Command::new("sw_vers")
        .arg("-buildVersion")
        .output()?;
    anyhow::ensure!(os.status.success(), "cannot identify Vision OS build");
    Ok(format!(
        "vision:{}:{}",
        hx_store::hash(include_bytes!("../../hx-visual/swift/ocr.swift")),
        String::from_utf8(os.stdout)?.trim()
    ))
}

/// Discover both upload orderings and old stills. Called repeatedly, including on startup.
/// Missing/unpublished blobs do not spend attempts.
pub async fn discover(store: &Store, version: &str) -> Result<()> {
    sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id) SELECT DISTINCT ON(b.sha256) b.sha256,$1,e.id FROM events e JOIN event_blobs eb ON eb.event_id=e.id JOIN blobs b USING(sha256) WHERE e.kind='media.still' AND b.published AND NOT b.evidence_expired AND NOT EXISTS(SELECT 1 FROM ocr_jobs o WHERE o.sha256=b.sha256 AND o.engine_version=$1) ORDER BY b.sha256,e.source_start,e.id LIMIT 128 ON CONFLICT DO NOTHING")
        .bind(version).execute(&store.pool).await?;
    publish_lineage(store, version).await
}

async fn publish_lineage(store: &Store, version: &str) -> Result<()> {
    let mut tx = store.pool.begin().await?;
    let mut candidates: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('source_event_id',e.id,'sha256',o.sha256,'engine_version',o.engine_version) FROM ocr_jobs o JOIN event_blobs eb USING(sha256) JOIN events e ON e.id=eb.event_id WHERE o.engine_version=$1 AND o.status='complete' AND e.kind='media.still' AND NOT EXISTS(SELECT 1 FROM ocr_evidence d WHERE d.source_event_id=e.id AND d.sha256=o.sha256 AND d.engine_version=o.engine_version) ORDER BY e.source_start,e.id LIMIT 128")
        .bind(version).fetch_all(&mut *tx).await?;
    for candidate in &mut candidates {
        candidate["observation_id"] = json!(hx_schema::ObservationId::generate());
    }
    let times: Vec<chrono::DateTime<Utc>> = sqlx::query_scalar("WITH added AS (INSERT INTO ocr_evidence(source_event_id,sha256,engine_version,observation_id) SELECT source_event_id,sha256,engine_version,observation_id FROM jsonb_to_recordset($1) AS x(source_event_id bigint,sha256 text,engine_version text,observation_id text) ON CONFLICT DO NOTHING RETURNING source_event_id) SELECT e.source_start FROM events e JOIN added a ON a.source_event_id=e.id")
        .bind(json!(candidates)).fetch_all(&mut *tx).await?;
    let mut windows = std::collections::BTreeMap::new();
    for at in times {
        // A point can also be evidence in the next window's 30-minute overlap.
        for w in [
            hx_store::window_at(at),
            hx_store::window_at(at + Duration::minutes(30)),
        ] {
            windows.insert(w.start, w);
        }
    }
    for w in windows.values() {
        let generation: i64 = sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET generation=windows.generation+1,dirty=true RETURNING generation")
            .bind(w.start).bind(w.end).fetch_one(&mut *tx).await?;
        hx_store::request_window_job(&mut tx, "compile", *w, generation).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Run one durable job. Transaction/row locks recover after process death. The
/// global try-lock also prevents a lease retry from overlapping a live OCR process.
pub async fn run_one<E: OcrEngine + 'static>(
    store: &Store,
    engine: Arc<E>,
    version: &str,
) -> Result<bool> {
    let mut tx = store.pool.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended(current_database() || ':' || current_schema() || ':hx-ocr',0))")
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Ok(false);
    }
    let row = sqlx::query("SELECT o.sha256,o.source_event_id,e.envelope,e.source_start FROM ocr_jobs o JOIN events e ON e.id=o.source_event_id JOIN blobs b USING(sha256) WHERE o.engine_version=$1 AND o.status='pending' AND o.available_at<=now() AND b.published AND NOT b.evidence_expired ORDER BY e.source_start,e.id FOR UPDATE OF o SKIP LOCKED LIMIT 1")
        .bind(version).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let sha: String = row.get("sha256");
    let source: Value = row.get("envelope");
    let at: chrono::DateTime<Utc> = row.get("source_start");
    // Compare only against a recognized frame on the same device/connector/surface,
    // within 30 seconds. Always keep the skipped observation, never reuse its text.
    let prior = sqlx::query("SELECT o.sha256,o.perceptual_hash FROM ocr_jobs o JOIN events e ON e.id=o.source_event_id WHERE o.engine_version=$1 AND o.status='complete' AND e.connector_instance=$2 AND e.device IS NOT DISTINCT FROM $3 AND e.envelope->'payload'->'display' IS NOT DISTINCT FROM $4 AND e.envelope->'payload'->'app_bundle_id' IS NOT DISTINCT FROM $5 AND e.source_start<=$6 AND e.source_start>=$6-interval '30 seconds' ORDER BY e.source_start DESC,e.id DESC LIMIT 1")
        .bind(version).bind(source["connector_instance"].as_str().unwrap_or_default())
        .bind(source["device"].as_str()).bind(&source["payload"]["display"])
        .bind(&source["payload"]["app_bundle_id"]).bind(at).fetch_optional(&mut *tx).await?;
    let previous = prior.map(|r| {
        (
            r.get::<String, _>("sha256"),
            r.get::<String, _>("perceptual_hash"),
        )
    });
    let path = store.blob_path(&sha)?;
    let media = source["blobs"]
        .as_array()
        .and_then(|b| b.iter().find(|b| b["sha256"] == sha))
        .and_then(|b| b["media_type"].as_str())
        .unwrap_or("image/jpeg")
        .to_owned();
    let result = tokio::task::spawn_blocking(
        move || -> Result<(u64, Option<String>, Option<OcrResult>)> {
            let bytes = std::fs::read(path)?;
            let mut reader =
                image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(16384);
            limits.max_image_height = Some(16384);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let thumb = reader
                .decode()?
                .resize_exact(8, 8, image::imageops::FilterType::Triangle)
                .to_luma8();
            let hash = hx_visual::perceptual_hash(thumb.as_raw().as_slice().try_into()?);
            if let Some((sha, prior)) = previous
                && (hash ^ u64::from_str_radix(&prior, 16)?).count_ones() <= 3
            {
                return Ok((hash, Some(sha), None));
            }
            Ok((hash, None, Some(engine.ocr(&bytes, &media)?)))
        },
    )
    .await
    .context("OCR worker join")?;
    match result {
        Ok((hash, sampled, result)) => {
            sqlx::query("UPDATE ocr_jobs SET status=$3,perceptual_hash=$4,sampled_from=$5,result=$6,attempts=attempts+1 WHERE sha256=$1 AND engine_version=$2")
                .bind(&sha).bind(version).bind(if sampled.is_some() {"sampled"} else {"complete"})
                .bind(format!("{hash:016x}")).bind(sampled).bind(result.map(serde_json::to_value).transpose()?).execute(&mut *tx).await?;
        }
        Err(_) => {
            // Do not persist/log engine errors: they may contain captured text.
            sqlx::query("UPDATE ocr_jobs SET attempts=attempts+1,status=CASE WHEN attempts>=4 THEN 'failed' ELSE 'pending' END,available_at=now()+interval '30 seconds' WHERE sha256=$1 AND engine_version=$2")
                .bind(&sha).bind(version).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    publish_lineage(store, version).await?;
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
    let rows = sqlx::query("SELECT d.id,d.source_event_id,d.sha256,d.engine_version,o.result,e.observation_id,e.connector_instance FROM ocr_evidence d JOIN ocr_jobs o USING(sha256,engine_version) JOIN events e ON e.id=d.source_event_id WHERE d.source_event_id=ANY($1) ORDER BY e.source_start,d.id")
        .bind(&ids).fetch_all(&store.pool).await?;
    let by_id: std::collections::HashMap<_, _> = sources.iter().map(|e| (e.id, e)).collect();
    let mut out = Vec::new();
    for row in rows {
        let source = by_id[&row.get::<i64, _>("source_event_id")];
        let result: OcrResult = serde_json::from_value(row.get("result"))?;
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
            id: row.get("id"), at: source.at,
            kind: format!("{}:ocr.text", source.kind.split(':').next().unwrap_or("sensitive")),
            text: json!({"source_event_id":source.id,"source_observation_id":row.get::<String,_>("observation_id"),"sha256":row.get::<String,_>("sha256"),"connector_instance":row.get::<String,_>("connector_instance"),"engine_version":row.get::<String,_>("engine_version"),"text":text,"coverage":result.coverage}).to_string(),
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
