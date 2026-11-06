//! Durable OCR persistence. The claim holds an OS file lock across engine work.
use super::*;
impl Store {
    async fn scan_ocr_sources(&self, version: &str) -> Result<()> {
        // IDs are allocated and committed under the same single writer. No
        // lower event ID can commit after this snapshot's cursor. Source time
        // is deliberately not the cursor: old observations may arrive today.
        let cursor: (i64, String) =
            sqlx::query_as("SELECT event_id,sha256 FROM ocr_scan_progress WHERE engine_version=$1")
                .bind(version)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or_default();
        let sources: Vec<(i64,Option<String>)> = sqlx::query_as("SELECT e.id,eb.sha256 FROM events e INDEXED BY events_stills LEFT JOIN event_blobs eb ON eb.event_id=e.id WHERE e.kind='media.still' AND e.id >= $1 AND (e.id>$1 OR eb.sha256>$2) ORDER BY e.id,eb.sha256 LIMIT 128")
            .bind(cursor.0).bind(&cursor.1).fetch_all(&self.pool).await?;
        if let Some(last) = sources.last() {
            let mut tx = self.begin_write().await?;
            for (source, sha) in &sources {
                let Some(sha) = sha else { continue };
                sqlx::query(
                    "INSERT INTO ocr_pending_sources(engine_version,source_event_id,sha256) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
                )
                .bind(version)
                .bind(source)
                .bind(sha)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("INSERT INTO ocr_scan_progress VALUES($1,$2,$3) ON CONFLICT(engine_version) DO UPDATE SET event_id=EXCLUDED.event_id,sha256=EXCLUDED.sha256 WHERE (event_id,sha256)<(EXCLUDED.event_id,EXCLUDED.sha256)")
                .bind(version).bind(last.0).bind(last.1.as_deref().unwrap_or("")).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }

    pub async fn discover_ocr(&self, version: &str) -> Result<()> {
        self.scan_ocr_sources(version).await?;
        // Pending associations retain unpublished blobs and unfinished results.
        // Candidate scans are reads; only point rechecks and <=128 inserts hold
        // the writer. Empty, caught-up passes take no write transaction at all.
        let candidates: Vec<(i64,String)> = sqlx::query_as("SELECT (SELECT e.id FROM event_blobs eb JOIN events e ON e.id=eb.event_id WHERE eb.sha256=p.sha256 AND e.kind='media.still' ORDER BY e.source_start,e.id LIMIT 1),p.sha256 FROM ocr_pending_sources p CROSS JOIN blobs b ON b.sha256=p.sha256 WHERE p.engine_version=$1 AND NOT p.settled AND b.published AND NOT b.evidence_expired AND NOT EXISTS(SELECT 1 FROM ocr_jobs o WHERE o.sha256=p.sha256 AND o.engine_version=$1) ORDER BY p.source_event_id,p.sha256 LIMIT 128")
            .bind(version).fetch_all(&self.pool).await?;
        if !candidates.is_empty() {
            let mut tx = self.begin_write().await?;
            let now = Ts::now();
            for (source, sha) in candidates {
                sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,available_at) SELECT sha256,$1,$2,$3 FROM blobs WHERE sha256=$4 AND published AND NOT evidence_expired ON CONFLICT DO NOTHING")
                    .bind(version).bind(source).bind(now).bind(sha).execute(&mut *tx).await?;
            }
            tx.commit().await?;
        }
        self.publish_ocr_lineage(version).await
    }

    /// Device discovery has one persisted cursor independent of engine versions.
    /// The empty namespace cannot collide with a validated `vision:` engine ID.
    /// Keep unfinished associations until a successful result arrives, including
    /// after a failed result or a PUT committed before the lineage worker ran.
    pub async fn publish_device_ocr_lineage(&self) -> Result<()> {
        self.scan_ocr_sources("").await?;
        self.publish_ocr_lineage_for(None).await
    }

    pub async fn publish_ocr_lineage(&self, version: &str) -> Result<()> {
        self.publish_ocr_lineage_for(Some(version)).await
    }

    async fn publish_ocr_lineage_for(&self, version: Option<&str>) -> Result<()> {
        let device_mode = version.is_none();
        let scan_version = version.unwrap_or("");
        // All historical discovery and candidate selection stays outside the writer.
        // Device associations select the first stored successful job for each hash;
        // a sampled result settles only that observation and never borrows text.
        let query = if version.is_some() {
            "SELECT p.source_event_id,p.sha256,o.engine_version FROM ocr_pending_sources p CROSS JOIN ocr_jobs o ON o.sha256=p.sha256 AND o.engine_version=p.engine_version WHERE p.engine_version=$1 AND NOT p.settled AND o.status IN ('complete','sampled','failed') ORDER BY p.source_event_id,p.sha256 LIMIT 16"
        } else {
            "SELECT p.source_event_id,p.sha256,o.engine_version FROM ocr_pending_sources p CROSS JOIN ocr_jobs o ON o.rowid=(SELECT j.rowid FROM ocr_jobs j WHERE j.sha256=p.sha256 AND j.status IN ('complete','sampled') ORDER BY j.rowid LIMIT 1) WHERE p.engine_version=$1 AND NOT p.settled ORDER BY p.source_event_id,p.sha256 LIMIT 16"
        };
        let candidates: Vec<(i64, String, String)> = sqlx::query_as(query)
            .bind(scan_version)
            .fetch_all(&self.pool)
            .await?;
        if candidates.is_empty() {
            return Ok(());
        }
        // A lineage batch may dirty two large timelines per source. Keep it
        // smaller than discovery so those row updates also bound writer time.
        let mut tx = self.begin_write().await?;
        let mut windows = std::collections::BTreeMap::new();
        for (source, sha, version) in candidates {
            let status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM ocr_jobs WHERE sha256=$1 AND engine_version=$2",
            )
            .bind(&sha)
            .bind(&version)
            .fetch_optional(&mut *tx)
            .await?;
            // A store moved from server OCR can already have a citation from
            // another engine. Preserve it rather than minting duplicate evidence.
            // Recheck by source/hash under the writer to fence concurrent publishers.
            let linked = device_mode && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM ocr_evidence WHERE source_event_id=$1 AND sha256=$2)")
                .bind(source).bind(&sha).fetch_one(&mut *tx).await?;
            if status.as_deref() == Some("complete") && !linked
                && tx.insert_evidence(sqlx::query_scalar("INSERT INTO ocr_evidence(id,source_event_id,sha256,engine_version,observation_id) VALUES((SELECT next_id FROM evidence_seq),$1,$2,$3,$4) ON CONFLICT DO NOTHING RETURNING id")
                    .bind(source).bind(&sha).bind(&version).bind(hx_schema::ObservationId::generate().to_string())).await?.is_some() {
                    let at: Ts = sqlx::query_scalar("SELECT source_start FROM events WHERE id=$1").bind(source).fetch_one(&mut *tx).await?;
                    let at = DateTime::<Utc>::from(at);
                    for w in [window_at(at),window_at(at+Duration::minutes(30))] { windows.insert(w.start,w); }
            }
            if status.as_deref() == Some("complete")
                || (scan_version.is_empty() && status.as_deref() == Some("sampled"))
            {
                sqlx::query("DELETE FROM ocr_pending_sources WHERE engine_version=$1 AND source_event_id=$2 AND sha256=$3")
                    .bind(scan_version).bind(source).bind(&sha).execute(&mut *tx).await?;
            } else if matches!(status.as_deref(), Some("sampled" | "failed")) {
                // Keep dormant associations for an operator reset without
                // making idle scans revisit terminal history.
                sqlx::query("UPDATE ocr_pending_sources SET settled=1 WHERE engine_version=$1 AND source_event_id=$2 AND sha256=$3")
                    .bind(scan_version).bind(source).bind(&sha).execute(&mut *tx).await?;
            }
        }
        for w in windows.values() {
            let generation: i64 = sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET generation=windows.generation+1,dirty=true RETURNING generation")
                .bind(Ts::from(w.start)).bind(Ts::from(w.end)).fetch_one(&mut *tx).await?;
            request_window_job(&mut tx, "compile", *w, generation, Ts::now()).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Returns false only for a hash without an authorized still. No blob
    /// publication check: valid text may arrive before the blob upload completes.
    pub async fn accept_device_ocr(
        &self,
        sha: &str,
        request: &hx_schema::protocol::OcrRequest,
        token: &Token,
    ) -> Result<bool> {
        use hx_schema::protocol::OcrStatus;
        // Namespace lookup can visit multiple sources; do it before BEGIN IMMEDIATE.
        let source: Option<i64> = sqlx::query_scalar("SELECT e.id FROM event_blobs eb JOIN events e ON e.id=eb.event_id WHERE eb.sha256=$1 AND e.kind='media.still' AND EXISTS(SELECT 1 FROM json_each($2) p WHERE substr(e.connector_instance,1,length(p.value))=p.value) LIMIT 1")
            .bind(sha).bind(sqlx::types::Json(&token.prefixes)).fetch_optional(&self.pool).await?;
        let Some(source) = source else {
            return Ok(false);
        };
        // Events and their blob references are immutable. A single writer makes
        // competing versions/retries choose one success without replacing evidence.
        let mut tx = self.begin_write().await?;
        let done: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ocr_jobs WHERE sha256=$1 AND status IN ('complete','sampled'))")
            .bind(sha).fetch_one(&mut *tx).await?;
        if !done {
            let status = match request.status {
                OcrStatus::Complete => "complete",
                OcrStatus::Sampled => "sampled",
                OcrStatus::Failed => "failed",
            };
            sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,available_at,status,perceptual_hash,sampled_from,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(sha256,engine_version) DO UPDATE SET status=EXCLUDED.status,perceptual_hash=EXCLUDED.perceptual_hash,sampled_from=EXCLUDED.sampled_from,result=EXCLUDED.result")
                .bind(sha).bind(&request.engine_version).bind(source).bind(Ts::now()).bind(status)
                .bind(&request.perceptual_hash).bind(&request.sampled_from).bind(request.result.clone().map(stored_json))
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn claim_ocr(&self, version: &str) -> Result<Option<OcrClaim>> {
        let Some(lock) = self.try_file_lock("hx-ocr").await? else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, OcrCandidate>("SELECT o.sha256,o.source_event_id,e.envelope,e.source_start FROM ocr_jobs o JOIN events e ON e.id=o.source_event_id JOIN blobs b USING(sha256) WHERE o.engine_version=$1 AND o.status='pending' AND o.available_at<=$2 AND b.published AND NOT b.evidence_expired ORDER BY e.source_start,e.id LIMIT 1")
        .bind(version).bind(Ts::now()).fetch_optional(&self.pool).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(OcrClaim {
            store: self.clone(),
            _lock: lock,
            candidate: row,
        }))
    }
}
#[derive(Debug, sqlx::FromRow)]
pub struct OcrCandidate {
    pub sha256: String,
    pub source_event_id: i64,
    pub envelope: Value,
    #[sqlx(try_from = "Ts")]
    pub source_start: DateTime<Utc>,
}
pub struct OcrClaim {
    store: Store,
    _lock: crate::locks::FileLock,
    pub candidate: OcrCandidate,
}
impl OcrClaim {
    pub async fn previous(&self, version: &str) -> Result<Option<(String, String)>> {
        let source = &self.candidate.envelope;
        let at = self.candidate.source_start;
        // Compare only against a recognized frame on the same device/connector/surface,
        // within 30 seconds. Always keep the skipped observation, never reuse its text.
        let prior = sqlx::query_as("SELECT o.sha256,o.perceptual_hash FROM ocr_jobs o JOIN events e ON e.id=o.source_event_id WHERE o.engine_version=$1 AND o.status='complete' AND e.connector_instance=$2 AND e.device IS NOT DISTINCT FROM $3 AND e.envelope->'payload'->'display' IS NOT DISTINCT FROM $4 AND e.envelope->'payload'->'app_bundle_id' IS NOT DISTINCT FROM $5 AND e.source_start<=$6 AND e.source_start>=$6-30000000 ORDER BY e.source_start DESC,e.id DESC LIMIT 1")
        .bind(version).bind(source["connector_instance"].as_str().unwrap_or_default())
        .bind(source["device"].as_str()).bind(source["payload"]["display"].to_string())
        .bind(source["payload"]["app_bundle_id"].to_string()).bind(Ts::from(at)).fetch_optional(&self.store.pool).await?;
        Ok(prior)
    }
    pub async fn complete(
        self,
        version: &str,
        hash: u64,
        sampled: Option<String>,
        result: Option<Value>,
    ) -> Result<()> {
        let sha = &self.candidate.sha256;
        sqlx::query("UPDATE ocr_jobs SET status=$3,perceptual_hash=$4,sampled_from=$5,result=$6,attempts=attempts+1 WHERE sha256=$1 AND engine_version=$2 AND status='pending'")
                .bind(sha).bind(version).bind(if sampled.is_some() {"sampled"} else {"complete"})
                .bind(format!("{hash:016x}")).bind(sampled).bind(result.map(stored_json)).execute(&self.store.pool).await?;
        Ok(())
    }
    pub async fn fail(self, version: &str) -> Result<()> {
        let sha = &self.candidate.sha256;
        sqlx::query("UPDATE ocr_jobs SET attempts=attempts+1,status=CASE WHEN attempts>=4 THEN 'failed' ELSE 'pending' END,available_at=$3 WHERE sha256=$1 AND engine_version=$2 AND status='pending'")
                .bind(sha).bind(version).bind(Ts::from(Utc::now()+Duration::seconds(30))).execute(&self.store.pool).await?;
        Ok(())
    }
}
#[derive(Debug, sqlx::FromRow)]
pub struct OcrEvidence {
    pub id: i64,
    pub source_event_id: i64,
    pub sha256: String,
    pub engine_version: String,
    pub result: Value,
    pub observation_id: String,
    pub connector_instance: String,
}
impl Store {
    pub async fn ocr_evidence_for_sources(&self, ids: &[i64]) -> Result<Vec<OcrEvidence>> {
        Ok(sqlx::query_as("SELECT d.id,d.source_event_id,d.sha256,d.engine_version,o.result,e.observation_id,e.connector_instance FROM ocr_evidence d JOIN ocr_jobs o USING(sha256,engine_version) JOIN events e ON e.id=d.source_event_id WHERE d.source_event_id IN (SELECT value FROM json_each($1)) ORDER BY e.source_start,d.id").bind(sqlx::types::Json(ids)).fetch_all(&self.pool).await?)
    }
}
