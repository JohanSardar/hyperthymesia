//! Durable evidence, derived generations, credentials and content-addressed files.
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use hx_compile::{Observation, Timeline};
use hx_schema::{Envelope, Interval, Sensitivity, kinds::CorePayload};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    pub data_dir: PathBuf,
}

#[derive(Debug, sqlx::FromRow)]
pub struct InterpretationState {
    pub generation: i64,
    pub canonical_revision: Option<i64>,
    pub input_changed_at: DateTime<Utc>,
    pub revision24_finished: bool,
    pub timeline_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub prefixes: Vec<String>,
    pub scopes: Vec<String>,
}
impl Token {
    pub fn has(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
    pub fn allows(&self, connector: &str) -> bool {
        self.prefixes.iter().any(|p| connector.starts_with(p))
    }
}

pub use hx_schema::{Rejected, UploadAck, UploadBatch};

pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn window_at(t: DateTime<Utc>) -> Interval {
    let start = DateTime::from_timestamp(t.timestamp().div_euclid(14400) * 14400, 0)
        .expect("valid timestamp");
    Interval {
        start,
        end: start + Duration::hours(4),
    }
}
impl Store {
    pub fn new(pool: PgPool, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            pool,
            data_dir: data_dir.into(),
        }
    }
    pub async fn connect(url: &str, dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::new(PgPool::connect(url).await?, dir))
    }
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }
    pub async fn create_token(
        &self,
        name: &str,
        prefixes: Vec<String>,
        scopes: Vec<String>,
    ) -> Result<String> {
        if prefixes.is_empty() || prefixes.iter().any(String::is_empty) {
            bail!("nonempty namespace prefixes required");
        }
        if scopes
            .iter()
            .any(|s| !["ingest", "read_l3", "read_raw"].contains(&s.as_str()))
        {
            bail!("unknown scope");
        }
        let mut bytes = [0_u8; 32];
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let secret = hex::encode(bytes);
        sqlx::query("INSERT INTO device_tokens(hash,name,prefixes,scopes) VALUES($1,$2,$3,$4)")
            .bind(hash(secret.as_bytes()))
            .bind(name)
            .bind(prefixes)
            .bind(scopes)
            .execute(&self.pool)
            .await?;
        Ok(secret)
    }
    pub async fn authenticate(&self, secret: &str) -> Result<Option<Token>> {
        let r =
            sqlx::query("SELECT prefixes,scopes FROM device_tokens WHERE hash=$1 AND NOT revoked")
                .bind(hash(secret.as_bytes()))
                .fetch_optional(&self.pool)
                .await?;
        Ok(r.map(|r| Token {
            prefixes: r.get("prefixes"),
            scopes: r.get("scopes"),
        }))
    }
    pub async fn ingest(&self, batch: UploadBatch, token: &Token) -> Result<UploadAck> {
        let mut ack = UploadAck {
            batch_id: batch.batch_id,
            accepted: 0,
            duplicates: 0,
            rejected: vec![],
        };
        let mut tx = self.pool.begin().await?;
        let mut affected = std::collections::BTreeMap::new();
        let mut normalize_windows = std::collections::BTreeMap::new();
        let mut watermarks =
            std::collections::BTreeMap::<String, (DateTime<Utc>, Option<u64>)>::new();
        for e in batch.envelopes {
            let error = if !token.has("ingest") {
                Some("ingest scope required".into())
            } else if !token.allows(e.connector_instance.as_str()) {
                Some("connector namespace forbidden".into())
            } else {
                e.validate().err().map(|e| e.to_string())
            };
            if let Some(reason) = error {
                ack.rejected.push(Rejected {
                    observation_id: e.observation_id,
                    reason,
                });
                continue;
            }
            let start = e.time.source_start.unwrap_or(e.time.observed_at);
            let sensitivity = match hx_schema::kinds::stored_class(e.kind.as_str(), e.sensitivity) {
                Sensitivity::Normal => "normal",
                Sensitivity::Sensitive => "sensitive",
                Sensitivity::Restricted => "restricted",
            };
            let inserted = sqlx::query("INSERT INTO events(observation_id,connector_instance,source_key,revision,kind,source_start,source_end,observed_at,device,sensitivity,envelope) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT(connector_instance,source_key,revision) DO NOTHING RETURNING id,(SELECT coalesce(jsonb_agg(jsonb_build_array(prior.source_start,prior.source_end)),'[]'::jsonb) FROM events prior WHERE prior.connector_instance=$2 AND prior.source_key=$3) AS previous_ranges")
                .bind(e.observation_id.to_string()).bind(e.connector_instance.as_str()).bind(e.source_key.as_str()).bind(e.revision.as_str()).bind(e.kind.as_str())
                .bind(start).bind(e.time.source_end).bind(e.time.observed_at).bind(e.device.as_ref().map(|d| d.as_str())).bind(sensitivity).bind(serde_json::to_value(&e)?)
                .fetch_optional(&mut *tx).await?;
            let Some(inserted) = inserted else {
                ack.duplicates += 1;
                continue;
            };
            let id: i64 = inserted.get("id");
            ack.accepted += 1;
            for b in &e.blobs {
                sqlx::query("INSERT INTO blobs(sha256,retention_class) VALUES($1,$2) ON CONFLICT(sha256) DO UPDATE SET retention_class=CASE WHEN EXCLUDED.retention_class='keep' THEN 'keep' ELSE blobs.retention_class END")
                    .bind(&b.sha256).bind(if e.kind.as_str()=="media.chunk" {"chunk"} else {"keep"}).execute(&mut *tx).await?;
                sqlx::query("WITH added AS (INSERT INTO event_blobs VALUES($1,$2) ON CONFLICT DO NOTHING RETURNING sha256) UPDATE blobs SET refcount=refcount+1 WHERE sha256 IN (SELECT sha256 FROM added)")
                    .bind(id).bind(&b.sha256).execute(&mut *tx).await?;
            }
            normalize_windows.insert(window_at(start).start, window_at(start));
            // Corrections can shrink or move a span. Invalidate the union of every
            // immutable revision's range, including the following context overlap.
            // Fetch old ranges in INSERT's RETURNING expression, avoiding another
            // database round trip for each event in large synthetic/live batches.
            let ranges: Vec<(DateTime<Utc>, Option<DateTime<Utc>>)> =
                serde_json::from_value(inserted.get("previous_ranges"))?;
            for (start, source_end) in ranges.into_iter().chain([(start, e.time.source_end)]) {
                let end = source_end.unwrap_or(start) + Duration::minutes(30);
                let mut window = window_at(start);
                loop {
                    affected.insert(window.start, window);
                    window = window_at(window.end);
                    if window.start >= end {
                        break;
                    }
                }
            }
            let watermark = watermarks
                .entry(e.connector_instance.as_str().to_owned())
                .or_insert((e.time.observed_at, e.sequence));
            watermark.0 = watermark.0.max(e.time.observed_at);
            watermark.1 = watermark.1.max(e.sequence);
        }
        // Only accepted observations advance watermarks. Coalesce independent
        // timestamp/sequence maxima once per connector without per-event I/O.
        for (connector, (observed_at, sequence)) in watermarks {
            sqlx::query("INSERT INTO watermarks(connector_instance,observed_through,sequence) VALUES($1,$2,$3::text::numeric) ON CONFLICT(connector_instance) DO UPDATE SET observed_through=greatest(watermarks.observed_through,EXCLUDED.observed_through),sequence=greatest(watermarks.sequence,EXCLUDED.sequence)")
                .bind(connector).bind(observed_at).bind(sequence.map(|n| n.to_string())).execute(&mut *tx).await?;
        }
        for window in affected.values() {
            let generation: i64 = sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET generation=windows.generation+1,dirty=true RETURNING generation")
                .bind(window.start).bind(window.end).fetch_one(&mut *tx).await?;
            request_window_job(&mut tx, "compile", *window, generation).await?;
            if normalize_windows.contains_key(&window.start) {
                request_window_job(&mut tx, "normalize", *window, generation).await?;
            }
        }
        tx.commit().await?;
        Ok(ack)
    }
    pub async fn normalize(&self, id: i64) -> Result<()> {
        let v: Value = sqlx::query_scalar("SELECT envelope FROM events WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        self.normalize_values(vec![(id, v)]).await
    }
    /// Bounded chunks keep retries idempotent and avoid one database round trip per event.
    pub async fn normalize_window(&self, window: Interval) -> Result<()> {
        loop {
            let rows = sqlx::query("SELECT id,envelope FROM events e WHERE source_start >= $1 AND source_start < $2 AND NOT EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id) AND NOT EXISTS(SELECT 1 FROM quarantine q WHERE q.event_id=e.id) ORDER BY id LIMIT 512")
                .bind(window.start).bind(window.end).fetch_all(&self.pool).await?;
            if rows.is_empty() {
                return Ok(());
            }
            self.normalize_values(
                rows.into_iter()
                    .map(|r| (r.get("id"), r.get("envelope")))
                    .collect(),
            )
            .await?;
        }
    }
    async fn normalize_values(&self, values: Vec<(i64, Value)>) -> Result<()> {
        let mut normalized = Vec::new();
        let mut quarantined = Vec::new();
        for (id, v) in values {
            let e: Envelope = serde_json::from_value(v)?;
            let failure = if hx_schema::kinds::is_core(e.kind.as_str()) && e.schema_version != 1 {
                Some(format!(
                    "unsupported core schema_version {}",
                    e.schema_version
                ))
            } else {
                CorePayload::decode(e.kind.as_str(), &e.payload)
                    .and_then(Result::err)
                    .map(|e| e.to_string())
            };
            if failure.is_some() {
                quarantined
                    .push(json!({"event_id":id,"reason":"invalid or unsupported core payload"}));
                continue;
            }
            let text = match e.kind.as_str() {
                "text.snapshot" | "agent.message" => {
                    e.payload["text"].as_str().unwrap_or("").to_owned()
                }
                "web.visit" => format!(
                    "{} {}",
                    e.payload["title"].as_str().unwrap_or(""),
                    e.payload["url"].as_str().unwrap_or("")
                ),
                _ => String::new(),
            };
            normalized.push(json!({"event_id":id,"payload":e.payload,"search_text":text}));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO normalized(event_id,normalizer_version,payload,search_text) SELECT event_id,'normalize/v1',payload,search_text FROM jsonb_to_recordset($1) AS x(event_id bigint,payload jsonb,search_text text) ON CONFLICT DO NOTHING")
            .bind(json!(normalized)).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO quarantine(event_id,reason) SELECT event_id,reason FROM jsonb_to_recordset($1) AS x(event_id bigint,reason text) ON CONFLICT(event_id) DO UPDATE SET reason=EXCLUDED.reason")
            .bind(json!(quarantined)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Revisions outside the interval may supersede evidence inside it. Wait for
    /// their classification too, so an old normalized revision cannot win early.
    pub async fn pending_normalization(&self, window: Interval) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM events e WHERE EXISTS (SELECT 1 FROM events related WHERE related.connector_instance=e.connector_instance AND related.source_key=e.source_key AND related.source_start<$2 AND coalesce(related.source_end,related.source_start)>=$1) AND NOT EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id) AND NOT EXISTS(SELECT 1 FROM quarantine q WHERE q.event_id=e.id)")
            .bind(window.start).bind(window.end).fetch_one(&self.pool).await?)
    }

    /// Return the complete revision family for overlapping identities. The pure
    /// compiler resolves revision order before clipping the winning revision.
    pub async fn observations(
        &self,
        window: Interval,
        normalized_only: bool,
    ) -> Result<Vec<Observation>> {
        let rows = sqlx::query("SELECT e.id,e.received_at,e.envelope FROM events e WHERE EXISTS (SELECT 1 FROM events related WHERE related.connector_instance=e.connector_instance AND related.source_key=e.source_key AND related.source_start < $2 AND (related.source_end > $1 OR (related.source_end IS NULL AND related.source_start >= $1) OR (related.source_end=related.source_start AND related.source_start >= $1))) AND (NOT $3 OR EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id)) ORDER BY source_start,id")
            .bind(window.start).bind(window.end).bind(normalized_only).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(Observation {
                    id: r.get("id"),
                    received_at: r.get("received_at"),
                    envelope: serde_json::from_value(r.get("envelope"))?,
                })
            })
            .collect()
    }
    pub async fn generation(&self, w: Interval) -> Result<i64> {
        Ok(sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET start_at=EXCLUDED.start_at RETURNING generation").bind(w.start).bind(w.end).fetch_one(&self.pool).await?)
    }
    /// Read-only fence for a run; unlike generation(), this never creates work.
    pub async fn is_current_generation(&self, w: Interval, generation: i64) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM windows WHERE start_at=$1 AND generation=$2)",
        )
        .bind(w.start)
        .bind(generation)
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn save_timeline(&self, timeline: &Timeline, generation: i64) -> Result<bool> {
        Ok(sqlx::query("UPDATE windows SET timeline=$3,compiler_version=$4,compiled_generation=$2,dirty=false,timeline_hash=$5 WHERE start_at=$1 AND generation=$2")
            .bind(timeline.window.start).bind(generation).bind(serde_json::to_value(timeline)?).bind(&timeline.compiler_version).bind(hash(&serde_json::to_vec(timeline)?)).execute(&self.pool).await?.rows_affected()==1)
    }
    pub async fn interpretation_state(&self, w: Interval) -> Result<InterpretationState> {
        Ok(sqlx::query_as("SELECT w.generation,w.canonical_revision,w.input_changed_at,w.revision24_finished, (r.id IS NULL OR (r.input_generation<>w.generation AND w.timeline_hash IS DISTINCT FROM r.interpretation#>>'{input_manifest,timeline_hash}')) AS timeline_changed FROM windows w LEFT JOIN revisions r ON r.id=w.canonical_revision WHERE w.start_at=$1")
            .bind(w.start).fetch_one(&self.pool).await?)
    }
    pub async fn finish_revision24(&self, w: Interval, generation: i64) -> Result<()> {
        sqlx::query(
            "UPDATE windows SET revision24_finished=true WHERE start_at=$1 AND generation=$2",
        )
        .bind(w.start)
        .bind(generation)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Reserve only one stale window per UTC day, including across scheduler restarts.
    /// A failed current-version job keeps its retry cap; the sweep cannot bypass it.
    pub async fn schedule_daily_sweep(&self, now: DateTime<Utc>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let candidate = sqlx::query("SELECT w.start_at,w.end_at,w.generation FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE w.revision24_finished AND w.empty_generation IS DISTINCT FROM w.generation AND w.end_at+interval '24 hours'<=$1 AND NOT w.dirty AND w.compiled_generation=w.generation AND r.input_generation<w.generation AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind='interpret' AND (j.state IN ('ready','running') OR (j.state='failed' AND (j.payload->>'generation')::bigint>=w.generation)) AND (j.payload->'window'->>'start')::timestamptz=w.start_at) ORDER BY w.end_at LIMIT 1 FOR UPDATE OF w SKIP LOCKED")
            .bind(now).fetch_optional(&mut *tx).await?;
        if let Some(row) = candidate {
            let w = Interval {
                start: row.get("start_at"),
                end: row.get("end_at"),
            };
            let generation: i64 = row.get("generation");
            let reserved: Option<chrono::NaiveDate> = sqlx::query_scalar("INSERT INTO daily_sweeps(day,start_at,generation) VALUES($1,$2,$3) ON CONFLICT DO NOTHING RETURNING day")
                .bind(now.date_naive()).bind(w.start).bind(generation).fetch_optional(&mut *tx).await?;
            if reserved.is_some() {
                sqlx::query("INSERT INTO jobs(output_key,kind,payload,queue) VALUES($1,'interpret',$2,'backfill') ON CONFLICT(output_key) DO UPDATE SET payload=EXCLUDED.payload,revivals=CASE WHEN jobs.payload->'generation'=EXCLUDED.payload->'generation' THEN jobs.revivals ELSE 0 END,request_version=jobs.request_version+1,state='ready',attempts=0,available_at=now(),last_error=NULL WHERE jobs.state='done' OR (jobs.state='failed' AND (jobs.payload->>'generation')::bigint<(EXCLUDED.payload->>'generation')::bigint)")
                    .bind(format!("interpret:{}", w.start)).bind(json!({"window":w,"generation":generation,"sweep_day":now.date_naive()})).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn start_daily_sweep(&self, w: Interval, day: chrono::NaiveDate) -> Result<bool> {
        // A crash or failure after admission cannot purchase another run that day.
        Ok(sqlx::query("UPDATE daily_sweeps SET started=true WHERE day=$1 AND start_at=$2 AND NOT started AND day=(now() AT TIME ZONE 'UTC')::date")
            .bind(day).bind(w.start).execute(&self.pool).await?.rows_affected()==1)
    }
    pub async fn release_daily_sweep(&self, w: Interval, day: chrono::NaiveDate) -> Result<()> {
        sqlx::query("UPDATE daily_sweeps SET started=false WHERE day=$1 AND start_at=$2")
            .bind(day)
            .bind(w.start)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn finish_empty(&self, w: Interval, generation: i64) -> Result<bool> {
        Ok(sqlx::query("UPDATE windows SET empty_generation=$2,dirty=false WHERE start_at=$1 AND generation=$2")
            .bind(w.start).bind(generation).execute(&self.pool).await?.rows_affected() == 1)
    }
    pub async fn publish_revision(
        &self,
        w: Interval,
        generation: i64,
        value: Value,
    ) -> Result<(i64, bool)> {
        self.publish_revision_key(w, generation, value, &ulid::Ulid::generate().to_string())
            .await
    }
    pub async fn publish_revision_key(
        &self,
        w: Interval,
        generation: i64,
        value: Value,
        output_key: &str,
    ) -> Result<(i64, bool)> {
        let namespaces:Vec<String>=sqlx::query_scalar("SELECT DISTINCT connector_instance FROM events WHERE source_start<$2 AND coalesce(source_end,source_start)>=$1")
            .bind(w.start-Duration::minutes(30)).bind(w.end).fetch_all(&self.pool).await?;
        self.publish_revision_manifest(w, generation, value, output_key, &namespaces)
            .await
    }
    pub async fn publish_revision_manifest(
        &self,
        w: Interval,
        generation: i64,
        value: Value,
        output_key: &str,
        namespaces: &[String],
    ) -> Result<(i64, bool)> {
        let mut tx = self.pool.begin().await?;
        let window = sqlx::query(
            "SELECT generation,canonical_revision FROM windows WHERE start_at=$1 FOR UPDATE",
        )
        .bind(w.start)
        .fetch_one(&mut *tx)
        .await?;
        let current: i64 = window.get("generation");
        let canonical: Option<i64> = window.get("canonical_revision");
        if value.get("agent_run").is_some() && current != generation {
            return Ok((canonical.unwrap_or_default(), false));
        }
        let mut content = value.clone();
        let agent_run = content.as_object_mut().and_then(|v| v.remove("agent_run"));
        sqlx::query("INSERT INTO revisions(start_at,input_generation,interpretation,output_key,input_namespaces,agent_run) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(output_key) DO NOTHING").bind(w.start).bind(generation).bind(&content).bind(output_key).bind(namespaces).bind(agent_run).execute(&mut *tx).await?;
        let revision = sqlx::query(
            "SELECT id,input_generation,start_at,interpretation FROM revisions WHERE output_key=$1",
        )
        .bind(output_key)
        .fetch_one(&mut *tx)
        .await?;
        let id: i64 = revision.get("id");
        let publish = canonical.is_none_or(|canonical| canonical <= id)
            && current == revision.get::<i64, _>("input_generation")
            && w.start == revision.get::<DateTime<Utc>, _>("start_at");
        if publish {
            let previous:Option<Value>=sqlx::query_scalar("SELECT coalesce(interpretation->'tasks','null'::jsonb) FROM revisions WHERE id=(SELECT canonical_revision FROM windows WHERE start_at=$1)")
                .bind(w.start).fetch_optional(&mut *tx).await?;
            let stored: Value = revision.get("interpretation");
            if previous.as_ref().unwrap_or(&Value::Null)
                != stored.get("tasks").unwrap_or(&Value::Null)
            {
                sqlx::query(
                    "UPDATE windows later SET generation=later.generation+1,dirty=true WHERE later.start_at >= $2 AND (EXISTS (SELECT 1 FROM revisions run WHERE run.id=later.canonical_revision AND (EXISTS (SELECT 1 FROM revisions prior WHERE prior.start_at=$1 AND (run.interpretation->'input_manifest'->'prior_revision_ids' @> jsonb_build_array(prior.id) OR EXISTS (SELECT 1 FROM jsonb_array_elements(coalesce(run.agent_run->'calls','[]'::jsonb)) call WHERE call->>'tool'='prior_context' AND call->'result'->'revisions' @> jsonb_build_array(jsonb_build_object('revision_id',prior.id))))))) OR (later.start_at=$2 AND NOT EXISTS (SELECT 1 FROM revisions run WHERE run.id=later.canonical_revision AND (run.agent_run IS NOT NULL OR run.interpretation->'input_manifest' ? 'prior_revision_ids'))))",
                )
                .bind(w.start)
                .bind(w.end)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("UPDATE windows SET canonical_revision=$2 WHERE start_at=$1")
                .bind(w.start)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok((id, publish))
    }
    pub async fn search(&self, q: &str, raw: bool, prefixes: &[String]) -> Result<Vec<Value>> {
        let rows=sqlx::query("SELECT e.id,n.search_text,e.source_start FROM normalized n JOIN events e ON e.id=n.event_id WHERE search_vector @@ plainto_tsquery('simple',$1) AND ($2 OR e.sensitivity='normal') AND EXISTS(SELECT 1 FROM unnest($3::text[]) p WHERE starts_with(e.connector_instance,p)) ORDER BY e.source_start DESC LIMIT 100")
            .bind(q).bind(raw).bind(prefixes).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| json!({"id":r.get::<i64,_>("id"),"text":r.get::<String,_>("search_text"),"at":r.get::<DateTime<Utc>,_>("source_start")})).collect())
    }
    pub fn blob_path(&self, sha: &str) -> Result<PathBuf> {
        if !valid_hash(sha) {
            bail!("invalid sha256");
        }
        Ok(self.data_dir.join("blobs").join(&sha[..2]).join(sha))
    }
    pub async fn upload_blob(&self, sha: &str, bytes: &[u8]) -> Result<bool> {
        let path = self.blob_path(sha)?;
        if hash(bytes) != sha {
            bail!("sha256 mismatch");
        }
        tokio::fs::create_dir_all(path.parent().context("blob parent")?).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO blobs(sha256) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(sha)
            .execute(&mut *tx)
            .await?;
        let published: bool =
            sqlx::query_scalar("SELECT published FROM blobs WHERE sha256=$1 FOR UPDATE")
                .bind(sha)
                .fetch_one(&mut *tx)
                .await?;
        if published {
            tx.commit().await?;
            return Ok(false);
        }
        let temp = tempfile::NamedTempFile::new_in(path.parent().context("blob parent")?)?;
        let mut file = tokio::fs::File::from_std(temp.reopen()?);
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(temp.path(), &path).await?;
        std::fs::File::open(path.parent().context("blob parent")?)?.sync_all()?;
        sqlx::query(
            "UPDATE blobs SET published=true,evidence_expired=false,bytes=$2 WHERE sha256=$1",
        )
        .bind(sha)
        .bind(bytes.len() as i64)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
    pub async fn gc(&self) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        let hashes:Vec<String>=sqlx::query_scalar("SELECT sha256 FROM blobs WHERE retention_class='chunk' AND pins=0 AND published AND created_at < now()-interval '30 days' FOR UPDATE SKIP LOCKED").fetch_all(&mut *tx).await?;
        for sha in &hashes {
            match tokio::fs::remove_file(self.blob_path(sha)?).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            sqlx::query("UPDATE blobs SET published=false,evidence_expired=true WHERE sha256=$1")
                .bind(sha)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(hashes.len())
    }
    pub async fn pin(&self, sha: &str, pinned: bool) -> Result<()> {
        sqlx::query("UPDATE blobs SET pins=pins+$2 WHERE sha256=$1")
            .bind(sha)
            .bind(if pinned { 1_i64 } else { -1_i64 })
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn assert_entity(
        &self,
        entity: &str,
        assertion: Value,
        evidence: &[i64],
    ) -> Result<i64> {
        if entity.is_empty() || evidence.is_empty() {
            bail!("entity and evidence are required");
        }
        let sensitivity:String=sqlx::query_scalar("SELECT CASE max(CASE sensitivity WHEN 'restricted' THEN 2 WHEN 'sensitive' THEN 1 ELSE 0 END) WHEN 0 THEN 'normal' WHEN 1 THEN 'sensitive' ELSE 'restricted' END FROM events WHERE id=ANY($1)")
            .bind(evidence).fetch_one(&self.pool).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(DISTINCT id) FROM events WHERE id=ANY($1)")
                .bind(evidence)
                .fetch_one(&self.pool)
                .await?;
        let unique: std::collections::HashSet<_> = evidence.iter().collect();
        if count as usize != unique.len() {
            bail!("unknown evidence");
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE windows w SET generation=generation+1,dirty=true WHERE EXISTS (SELECT 1 FROM events e WHERE e.id=ANY($1) AND e.source_start<w.end_at AND CASE WHEN e.source_end>e.source_start THEN e.source_end>w.start_at ELSE e.source_start>=w.start_at END)")
            .bind(evidence)
            .execute(&mut *tx)
            .await?;
        let id=sqlx::query_scalar("INSERT INTO entity_assertions(entity,assertion,evidence,sensitivity) VALUES($1,$2,$3,$4) RETURNING id").bind(entity).bind(assertion).bind(evidence).bind(sensitivity).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn evidence(&self, id: i64) -> Result<Option<Value>> {
        let value:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'envelope',e.envelope,'sensitivity',e.sensitivity,'blobs',coalesce((SELECT jsonb_agg(jsonb_build_object('sha256',b.sha256,'evidence_expired',b.evidence_expired)) FROM event_blobs eb JOIN blobs b USING(sha256) WHERE eb.event_id=e.id),'[]'::jsonb)) FROM events e WHERE id=$1").bind(id).fetch_optional(&self.pool).await?;
        if let Some(mut value) = value {
            let derived: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'envelope',envelope,'sensitivity',sensitivity) FROM ocr_derived WHERE source_event_id=$1 ORDER BY id")
                .bind(id).fetch_all(&self.pool).await?;
            value["derived_evidence"] = json!(derived);
            return Ok(Some(value));
        }
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',d.id,'envelope',d.envelope,'sensitivity',d.sensitivity,'source_event_id',d.source_event_id,'blobs',jsonb_build_array(jsonb_build_object('sha256',b.sha256,'evidence_expired',b.evidence_expired))) FROM ocr_derived d JOIN blobs b USING(sha256) WHERE d.id=$1")
            .bind(id).fetch_optional(&self.pool).await?)
    }
}

/// Update a single durable work slot without stealing an in-flight lease.
/// A completion observes the request counter and requeues if input changed under it.
pub async fn request_window_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    window: Interval,
    generation: i64,
) -> Result<()> {
    sqlx::query("INSERT INTO jobs(output_key,kind,payload) VALUES($1,$2,$3) ON CONFLICT(output_key) DO UPDATE SET payload=EXCLUDED.payload,revivals=0,request_version=jobs.request_version+1,state=CASE WHEN jobs.state='running' THEN 'running' ELSE 'ready' END,attempts=CASE WHEN jobs.state='running' THEN jobs.attempts ELSE 0 END,available_at=now(),last_error=NULL WHERE (jobs.payload->>'generation')::bigint < (EXCLUDED.payload->>'generation')::bigint")
        .bind(format!("{kind}:{}",window.start)).bind(kind).bind(json!({"window":window,"generation":generation})).execute(&mut **tx).await?;
    Ok(())
}

/// Coalesce interpretation requests while preserving an active worker's fencing token.
pub async fn request_interpret_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    window: Interval,
    generation: i64,
    revision24: bool,
    at: DateTime<Utc>,
) -> Result<()> {
    let slot = if revision24 {
        "revision24"
    } else {
        "interpret"
    };
    sqlx::query("INSERT INTO jobs(output_key,kind,payload,queue,available_at) VALUES($1,'interpret',$2,$3,$4) ON CONFLICT(output_key) DO UPDATE SET payload=EXCLUDED.payload,revivals=0,request_version=jobs.request_version+1,state=CASE WHEN jobs.state='running' THEN 'running' ELSE 'ready' END,attempts=CASE WHEN jobs.state='running' THEN jobs.attempts ELSE 0 END,available_at=EXCLUDED.available_at,last_error=NULL WHERE coalesce((jobs.payload->>'generation')::bigint,0)<(EXCLUDED.payload->>'generation')::bigint")
        .bind(format!("{slot}:{}", window.start))
        .bind(json!({"window":window,"generation":generation,"revision24":revision24}))
        .bind(if revision24 { "backfill" } else { "interactive" })
        .bind(at).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(feature = "test-support")]
pub mod test_support;
