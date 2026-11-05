//! Reads used by the server. Access policy and presentation stay with callers.
use super::*;
use sqlx::Result;
#[derive(Debug, sqlx::FromRow)]
pub struct CanonicalRevision {
    pub id: i64,
    pub input_generation: i64,
    pub interpretation: Value,
    pub has_agent_run: bool,
    pub end_at: DateTime<Utc>,
}
#[derive(Debug, sqlx::FromRow)]
pub struct RevisionRun {
    pub agent_run: Value,
    pub input_namespaces: Vec<String>,
    pub interpretation: Value,
}
#[derive(Debug, sqlx::FromRow)]
pub struct SourceRecord {
    pub id: i64,
    pub envelope: Value,
}
#[derive(Debug, sqlx::FromRow)]
pub struct PriorRevision {
    pub id: i64,
    pub interpretation: Value,
}
#[derive(Debug, sqlx::FromRow)]
pub struct EntityAssertion {
    pub entity: String,
    pub assertion: Value,
    pub evidence: Vec<i64>,
}
#[derive(Debug, sqlx::FromRow)]
pub struct EventRecord {
    pub id: i64,
    pub received_at: DateTime<Utc>,
    pub envelope: Value,
}
impl Store {
    pub async fn digest_records(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<EventRecord>> {
        sqlx::query_as("WITH candidates AS (SELECT connector_instance,source_key FROM events WHERE source_start<$2 AND coalesce(source_end,source_start)>=$1) SELECT e.id,e.received_at,e.envelope FROM events e JOIN (SELECT DISTINCT connector_instance,source_key FROM candidates) c USING (connector_instance,source_key) ORDER BY e.source_start,e.id").bind(start).bind(end).fetch_all(&self.pool).await
    }
    pub async fn source_envelope(&self, id: i64) -> Result<Option<Value>> {
        sqlx::query_scalar("SELECT envelope FROM events WHERE id=$1 UNION ALL SELECT e.envelope FROM ocr_evidence d JOIN events e ON e.id=d.source_event_id WHERE d.id=$1").bind(id).fetch_optional(&self.pool).await
    }
    pub async fn attention_records(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<EventRecord>> {
        sqlx::query_as("SELECT id,received_at,envelope FROM events WHERE source_start<$2 AND coalesce(source_end,source_start)>=$1 ORDER BY source_start,id LIMIT 2001").bind(start).bind(end).fetch_all(&self.pool).await
    }
    pub async fn exploration_records(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<EventRecord>> {
        sqlx::query_as("SELECT id,received_at,envelope FROM events WHERE source_start<$2 AND coalesce(source_end,source_start)>=$1 ORDER BY source_start,id LIMIT 2000").bind(start).bind(end).fetch_all(&self.pool).await
    }
    pub async fn records_by_ids(&self, ids: &[i64]) -> Result<Vec<EventRecord>> {
        sqlx::query_as(
            "SELECT id,received_at,envelope FROM events WHERE id = ANY($1) ORDER BY source_start,id",
        ).bind(ids).fetch_all(&self.pool).await
    }
    pub async fn recent_entity_assertions(&self) -> Result<Vec<EntityAssertion>> {
        sqlx::query_as(
            "SELECT entity,assertion,evidence FROM entity_assertions ORDER BY id DESC LIMIT 100",
        )
        .fetch_all(&self.pool)
        .await
    }
    pub async fn latest_source_start(&self, ids: &[i64]) -> Result<Option<DateTime<Utc>>> {
        sqlx::query_scalar("SELECT max(source_start) FROM events WHERE id=ANY($1)")
            .bind(ids)
            .fetch_one(&self.pool)
            .await
    }
    pub async fn prior_revisions(&self, before: DateTime<Utc>) -> Result<Vec<PriorRevision>> {
        sqlx::query_as("SELECT r.id,r.interpretation FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE w.end_at<=$1 AND w.end_at>$1-interval '16 hours' ORDER BY w.end_at DESC LIMIT 4").bind(before).fetch_all(&self.pool).await
    }
    pub async fn source_namespace(&self, id: i64) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT connector_instance FROM events WHERE id=$1 UNION ALL SELECT e.connector_instance FROM ocr_evidence d JOIN events e ON e.id=d.source_event_id WHERE d.id=$1").bind(id).fetch_optional(&self.pool).await
    }
    pub async fn recent_source_records(&self) -> Result<Vec<SourceRecord>> {
        sqlx::query_as("SELECT DISTINCT ON(connector_instance,kind) id,envelope FROM events ORDER BY connector_instance,kind,source_start DESC LIMIT 2000").fetch_all(&self.pool).await
    }
    pub async fn owner_utc_offset(&self) -> Result<Option<i32>> {
        sqlx::query_scalar(
        "SELECT (envelope->'time'->>'utc_offset_min')::int FROM events WHERE envelope->'time'->>'utc_offset_min' IS NOT NULL ORDER BY id DESC LIMIT 1",
    ).fetch_optional(&self.pool).await
    }
    pub async fn revision_run(&self, revision: i64) -> Result<Option<RevisionRun>> {
        sqlx::query_as("SELECT agent_run,input_namespaces,interpretation FROM revisions WHERE id=$1 AND agent_run IS NOT NULL").bind(revision).fetch_optional(&self.pool).await
    }
    pub async fn blob_allowed(&self, sha: &str, prefixes: &[String]) -> Result<bool> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM event_blobs eb JOIN events e ON e.id=eb.event_id WHERE eb.sha256=$1 AND EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE starts_with(e.connector_instance,p)))").bind(sha).bind(prefixes).fetch_one(&self.pool).await
    }
    pub async fn blob_published(&self, sha: &str) -> Result<bool> {
        sqlx::query_scalar("SELECT published FROM blobs WHERE sha256=$1")
            .bind(sha)
            .fetch_one(&self.pool)
            .await
    }
    pub async fn canonical_revisions(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        prefixes: &[String],
    ) -> Result<Vec<CanonicalRevision>> {
        sqlx::query_as("SELECT r.id,r.input_generation,r.interpretation,r.agent_run IS NOT NULL AS has_agent_run,w.end_at FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE w.start_at<$2 AND w.end_at>$1 AND NOT EXISTS(SELECT 1 FROM unnest(r.input_namespaces) n WHERE NOT EXISTS(SELECT 1 FROM unnest($3::text[]) p WHERE starts_with(n,p))) ORDER BY w.start_at").bind(start).bind(end).bind(prefixes).fetch_all(&self.pool).await
    }
    pub async fn visible_entities(&self, raw: bool, prefixes: &[String]) -> Result<Vec<Value>> {
        sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'entity',a.entity,'assertion',a.assertion,'evidence',a.evidence) FROM entity_assertions a WHERE ($1 OR sensitivity='normal') AND NOT EXISTS(SELECT 1 FROM events e WHERE e.id=ANY(a.evidence) AND NOT EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE starts_with(e.connector_instance,p)))").bind(raw).bind(prefixes).fetch_all(&self.pool).await
    }
}
