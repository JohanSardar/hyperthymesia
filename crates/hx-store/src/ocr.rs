//! Durable OCR persistence. The claim handle retains its transaction across engine work.
use super::*;
impl Store {
    pub async fn discover_ocr(&self, version: &str) -> Result<()> {
        sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id) SELECT DISTINCT ON(b.sha256) b.sha256,$1,e.id FROM events e JOIN event_blobs eb ON eb.event_id=e.id JOIN blobs b USING(sha256) WHERE e.kind='media.still' AND b.published AND NOT b.evidence_expired AND NOT EXISTS(SELECT 1 FROM ocr_jobs o WHERE o.sha256=b.sha256 AND o.engine_version=$1) ORDER BY b.sha256,e.source_start,e.id LIMIT 128 ON CONFLICT DO NOTHING")
        .bind(version).execute(&self.pool).await?;
        self.publish_ocr_lineage(version).await
    }

    pub async fn publish_ocr_lineage(&self, version: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
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
            for w in [window_at(at), window_at(at + Duration::minutes(30))] {
                windows.insert(w.start, w);
            }
        }
        for w in windows.values() {
            let generation: i64 = sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET generation=windows.generation+1,dirty=true RETURNING generation")
            .bind(w.start).bind(w.end).fetch_one(&mut *tx).await?;
            request_window_job(&mut tx, "compile", *w, generation).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn claim_ocr(&self, version: &str) -> Result<Option<OcrClaim>> {
        let mut tx = self.pool.begin().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended(current_database() || ':' || current_schema() || ':hx-ocr',0))")
        .fetch_one(&mut *tx)
        .await?;
        if !locked {
            return Ok(None);
        }
        let row = sqlx::query_as::<_, OcrCandidate>("SELECT o.sha256,o.source_event_id,e.envelope,e.source_start FROM ocr_jobs o JOIN events e ON e.id=o.source_event_id JOIN blobs b USING(sha256) WHERE o.engine_version=$1 AND o.status='pending' AND o.available_at<=now() AND b.published AND NOT b.evidence_expired ORDER BY e.source_start,e.id FOR UPDATE OF o SKIP LOCKED LIMIT 1")
        .bind(version).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(OcrClaim { tx, candidate: row }))
    }
}
#[derive(Debug, sqlx::FromRow)]
pub struct OcrCandidate {
    pub sha256: String,
    pub source_event_id: i64,
    pub envelope: Value,
    pub source_start: DateTime<Utc>,
}
pub struct OcrClaim {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
    pub candidate: OcrCandidate,
}
impl OcrClaim {
    pub async fn previous(&mut self, version: &str) -> Result<Option<(String, String)>> {
        let source = &self.candidate.envelope;
        let at = self.candidate.source_start;
        // Compare only against a recognized frame on the same device/connector/surface,
        // within 30 seconds. Always keep the skipped observation, never reuse its text.
        let prior = sqlx::query_as("SELECT o.sha256,o.perceptual_hash FROM ocr_jobs o JOIN events e ON e.id=o.source_event_id WHERE o.engine_version=$1 AND o.status='complete' AND e.connector_instance=$2 AND e.device IS NOT DISTINCT FROM $3 AND e.envelope->'payload'->'display' IS NOT DISTINCT FROM $4 AND e.envelope->'payload'->'app_bundle_id' IS NOT DISTINCT FROM $5 AND e.source_start<=$6 AND e.source_start>=$6-interval '30 seconds' ORDER BY e.source_start DESC,e.id DESC LIMIT 1")
        .bind(version).bind(source["connector_instance"].as_str().unwrap_or_default())
        .bind(source["device"].as_str()).bind(&source["payload"]["display"])
        .bind(&source["payload"]["app_bundle_id"]).bind(at).fetch_optional(&mut *self.tx).await?;
        Ok(prior)
    }
    pub async fn complete(
        &mut self,
        version: &str,
        hash: u64,
        sampled: Option<String>,
        result: Option<Value>,
    ) -> Result<()> {
        let sha = &self.candidate.sha256;
        sqlx::query("UPDATE ocr_jobs SET status=$3,perceptual_hash=$4,sampled_from=$5,result=$6,attempts=attempts+1 WHERE sha256=$1 AND engine_version=$2")
                .bind(sha).bind(version).bind(if sampled.is_some() {"sampled"} else {"complete"})
                .bind(format!("{hash:016x}")).bind(sampled).bind(result).execute(&mut *self.tx).await?;
        Ok(())
    }
    pub async fn fail(&mut self, version: &str) -> Result<()> {
        let sha = &self.candidate.sha256;
        sqlx::query("UPDATE ocr_jobs SET attempts=attempts+1,status=CASE WHEN attempts>=4 THEN 'failed' ELSE 'pending' END,available_at=now()+interval '30 seconds' WHERE sha256=$1 AND engine_version=$2")
                .bind(sha).bind(version).execute(&mut *self.tx).await?;
        Ok(())
    }
    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await?;
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
        Ok(sqlx::query_as("SELECT d.id,d.source_event_id,d.sha256,d.engine_version,o.result,e.observation_id,e.connector_instance FROM ocr_evidence d JOIN ocr_jobs o USING(sha256,engine_version) JOIN events e ON e.id=d.source_event_id WHERE d.source_event_id=ANY($1) ORDER BY e.source_start,d.id").bind(ids).fetch_all(&self.pool).await?)
    }
}
