//! Durable OCR persistence. The claim holds an OS file lock across engine work.
use super::*;
impl Store {
    pub async fn discover_ocr(&self, version: &str) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,available_at) SELECT sha256,$1,id,$2 FROM (SELECT b.sha256,e.id,e.source_start,row_number() OVER (PARTITION BY b.sha256 ORDER BY e.source_start,e.id) AS rn FROM events e JOIN event_blobs eb ON eb.event_id=e.id JOIN blobs b USING(sha256) WHERE e.kind='media.still' AND b.published AND NOT b.evidence_expired AND NOT EXISTS(SELECT 1 FROM ocr_jobs o WHERE o.sha256=b.sha256 AND o.engine_version=$1)) WHERE rn=1 ORDER BY sha256,source_start,id LIMIT 128 ON CONFLICT DO NOTHING")
            .bind(version).bind(Ts::now()).execute(&mut *tx).await?;
        tx.commit().await?;
        self.publish_ocr_lineage(version).await
    }

    pub async fn publish_ocr_lineage(&self, version: &str) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let candidates: Vec<(i64,String,String,Ts)> = sqlx::query_as("SELECT e.id,o.sha256,o.engine_version,e.source_start FROM ocr_jobs o JOIN event_blobs eb USING(sha256) JOIN events e ON e.id=eb.event_id WHERE o.engine_version=$1 AND o.status='complete' AND e.kind='media.still' AND NOT EXISTS(SELECT 1 FROM ocr_evidence d WHERE d.source_event_id=e.id AND d.sha256=o.sha256 AND d.engine_version=o.engine_version) ORDER BY e.source_start,e.id LIMIT 128")
            .bind(version).fetch_all(&mut *tx).await?;
        let mut times = Vec::new();
        for (source, sha, engine, at) in candidates {
            if tx.insert_evidence(sqlx::query_scalar("INSERT INTO ocr_evidence(id,source_event_id,sha256,engine_version,observation_id) VALUES((SELECT next_id FROM evidence_seq),$1,$2,$3,$4) ON CONFLICT DO NOTHING RETURNING id")
                .bind(source).bind(sha).bind(engine).bind(hx_schema::ObservationId::generate().to_string())).await?.is_some() {
                times.push(DateTime::<Utc>::from(at));
            }
        }
        let mut windows = std::collections::BTreeMap::new();
        for at in times {
            // A point can also be evidence in the next window's 30-minute overlap.
            for w in [window_at(at), window_at(at + Duration::minutes(30))] {
                windows.insert(w.start, w);
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
        sqlx::query("UPDATE ocr_jobs SET status=$3,perceptual_hash=$4,sampled_from=$5,result=$6,attempts=attempts+1 WHERE sha256=$1 AND engine_version=$2")
                .bind(sha).bind(version).bind(if sampled.is_some() {"sampled"} else {"complete"})
                .bind(format!("{hash:016x}")).bind(sampled).bind(result.map(stored_json)).execute(&self.store.pool).await?;
        Ok(())
    }
    pub async fn fail(self, version: &str) -> Result<()> {
        let sha = &self.candidate.sha256;
        sqlx::query("UPDATE ocr_jobs SET attempts=attempts+1,status=CASE WHEN attempts>=4 THEN 'failed' ELSE 'pending' END,available_at=$3 WHERE sha256=$1 AND engine_version=$2")
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
