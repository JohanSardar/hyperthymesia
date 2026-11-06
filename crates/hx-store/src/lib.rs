//! Durable evidence, derived generations, credentials and content-addressed files.
pub mod backup;
pub mod sqlite;
pub use sqlite::{Store, Ts, WriteTransaction};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use hx_compile::{Observation, Timeline};
use hx_schema::{Envelope, Interval, Sensitivity, kinds::CorePayload};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

#[derive(Debug, sqlx::FromRow)]
pub struct InterpretationState {
    pub generation: i64,
    pub canonical_revision: Option<i64>,
    #[sqlx(try_from = "Ts")]
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
        self.prefixes.iter().any(|prefix| {
            connector.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.is_empty() || prefix.ends_with(':') || suffix.starts_with(':')
            })
        })
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
fn random_secret() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0_u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex::encode(bytes))
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
        let secret = random_secret()?;
        sqlx::query("INSERT INTO device_tokens(hash,name,prefixes,scopes) VALUES($1,$2,$3,$4)")
            .bind(hash(secret.as_bytes()))
            .bind(name)
            .bind(sqlx::types::Json(prefixes))
            .bind(sqlx::types::Json(scopes))
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
            prefixes: r.get::<sqlx::types::Json<Vec<String>>, _>("prefixes").0,
            scopes: r.get::<sqlx::types::Json<Vec<String>>, _>("scopes").0,
        }))
    }
    pub async fn ingest(&self, batch: UploadBatch, token: &Token) -> Result<UploadAck> {
        let mut ack = UploadAck {
            batch_id: batch.batch_id,
            accepted: 0,
            duplicates: 0,
            rejected: vec![],
        };
        let mut validated = Vec::new();
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
            validated.push(e);
        }
        let mut tx = self.begin_write().await?;
        let now = Ts::now();
        let mut affected = std::collections::BTreeMap::new();
        let mut normalize_windows = std::collections::BTreeMap::new();
        let mut watermarks =
            std::collections::BTreeMap::<String, (DateTime<Utc>, Option<u64>)>::new();
        for e in validated {
            let start = e.time.source_start.unwrap_or(e.time.observed_at);
            let sensitivity = match hx_schema::kinds::stored_class(e.kind.as_str(), e.sensitivity) {
                Sensitivity::Normal => "normal",
                Sensitivity::Sensitive => "sensitive",
                Sensitivity::Restricted => "restricted",
            };
            // RETURNING subqueries see the new row in SQLite, so old ranges
            // must be read separately. The shared allocator advances on success only.
            let inserted = tx.insert_evidence(sqlx::query_scalar("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,source_end,observed_at,device,sensitivity,envelope,received_at) VALUES((SELECT next_id FROM evidence_seq),$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(connector_instance,source_key,revision) DO NOTHING RETURNING id")
                .bind(e.observation_id.to_string()).bind(e.connector_instance.as_str()).bind(e.source_key.as_str()).bind(e.revision.as_str()).bind(e.kind.as_str())
                .bind(Ts::from(start)).bind(e.time.source_end.map(Ts::from)).bind(Ts::from(e.time.observed_at)).bind(e.device.as_ref().map(|d| d.as_str())).bind(sensitivity).bind(stored_json(serde_json::to_value(&e)?)).bind(now)).await?;
            let Some(id) = inserted else {
                ack.duplicates += 1;
                continue;
            };
            let ranges: Vec<(Ts, Option<Ts>)> = sqlx::query_as("SELECT source_start,source_end FROM events WHERE connector_instance=$1 AND source_key=$2 AND id<>$3")
                .bind(e.connector_instance.as_str()).bind(e.source_key.as_str()).bind(id).fetch_all(&mut *tx).await?;
            ack.accepted += 1;
            for b in &e.blobs {
                sqlx::query("INSERT INTO blobs(sha256,retention_class,created_at) VALUES($1,$2,$3) ON CONFLICT(sha256) DO UPDATE SET retention_class=CASE WHEN EXCLUDED.retention_class='keep' THEN 'keep' ELSE blobs.retention_class END")
                    .bind(&b.sha256).bind(if e.kind.as_str()=="media.chunk" {"chunk"} else {"keep"}).bind(now).execute(&mut *tx).await?;
                let added =
                    sqlx::query("INSERT INTO event_blobs VALUES($1,$2) ON CONFLICT DO NOTHING")
                        .bind(id)
                        .bind(&b.sha256)
                        .execute(&mut *tx)
                        .await?;
                if added.rows_affected() == 1 {
                    sqlx::query("UPDATE blobs SET refcount=refcount+1 WHERE sha256=$1")
                        .bind(&b.sha256)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            normalize_windows.insert(window_at(start).start, window_at(start));
            // Invalidate all immutable ranges of a corrected identity, including overlap.
            for (start, source_end) in ranges
                .into_iter()
                .map(|(s, e)| (DateTime::<Utc>::from(s), e.map(DateTime::<Utc>::from)))
                .chain([(start, e.time.source_end)])
            {
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
            sqlx::query("INSERT INTO watermarks(connector_instance,observed_through,sequence) VALUES($1,$2,$3) ON CONFLICT(connector_instance) DO UPDATE SET observed_through=max(watermarks.observed_through,EXCLUDED.observed_through),sequence=max(coalesce(watermarks.sequence,EXCLUDED.sequence),coalesce(EXCLUDED.sequence,watermarks.sequence))")
                .bind(connector).bind(Ts::from(observed_at)).bind(sequence.map(|n| format!("{n:020}"))).execute(&mut *tx).await?;
        }
        for window in affected.values() {
            let generation: i64 = sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET generation=windows.generation+1,dirty=true RETURNING generation")
                .bind(Ts::from(window.start)).bind(Ts::from(window.end)).fetch_one(&mut *tx).await?;
            request_window_job(&mut tx, "compile", *window, generation, now).await?;
            if normalize_windows.contains_key(&window.start) {
                request_window_job(&mut tx, "normalize", *window, generation, now).await?;
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
                .bind(Ts::from(window.start)).bind(Ts::from(window.end)).fetch_all(&self.pool).await?;
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
        let mut tx = self.begin_write().await?;
        for row in normalized {
            sqlx::query("INSERT INTO normalized(event_id,normalizer_version,payload,search_text) VALUES($1,'normalize/v1',$2,$3) ON CONFLICT DO NOTHING")
                .bind(row["event_id"].as_i64()).bind(stored_json(row["payload"].clone())).bind(row["search_text"].as_str()).execute(&mut *tx).await?;
        }
        let now = Ts::now();
        for row in quarantined {
            sqlx::query("INSERT INTO quarantine(event_id,reason,created_at) VALUES($1,$2,$3) ON CONFLICT(event_id) DO UPDATE SET reason=EXCLUDED.reason")
                .bind(row["event_id"].as_i64()).bind(row["reason"].as_str()).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Revisions outside the interval may supersede evidence inside it. Wait for
    /// their classification too, so an old normalized revision cannot win early.
    pub async fn pending_normalization(&self, window: Interval) -> Result<i64> {
        Ok(sqlx::query_scalar("WITH fam AS (SELECT DISTINCT connector_instance,source_key FROM events INDEXED BY events_time WHERE source_start<$2 AND coalesce(source_end,source_start)>=$1) SELECT count(*) FROM fam CROSS JOIN events e ON e.connector_instance=fam.connector_instance AND e.source_key=fam.source_key AND NOT EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id) AND NOT EXISTS(SELECT 1 FROM quarantine q WHERE q.event_id=e.id)")
            .bind(Ts::from(window.start)).bind(Ts::from(window.end)).fetch_one(&self.pool).await?)
    }

    /// Return the complete revision family for overlapping identities. The pure
    /// compiler resolves revision order before clipping the winning revision.
    pub async fn observations(
        &self,
        window: Interval,
        normalized_only: bool,
    ) -> Result<Vec<Observation>> {
        let rows = sqlx::query("WITH fam AS (SELECT DISTINCT connector_instance,source_key FROM events INDEXED BY events_time WHERE source_start<$2 AND (source_end>$1 OR (source_end IS NULL AND source_start>=$1) OR (source_end=source_start AND source_start>=$1))) SELECT e.id,e.received_at,e.envelope FROM fam CROSS JOIN events e ON e.connector_instance=fam.connector_instance AND e.source_key=fam.source_key AND (NOT $3 OR EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id)) ORDER BY source_start,id")
            .bind(Ts::from(window.start)).bind(Ts::from(window.end)).bind(normalized_only).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(Observation {
                    id: r.get("id"),
                    received_at: DateTime::<Utc>::from(r.get::<Ts, _>("received_at")),
                    envelope: serde_json::from_value(r.get("envelope"))?,
                })
            })
            .collect()
    }
    pub async fn generation(&self, w: Interval) -> Result<i64> {
        Ok(sqlx::query_scalar("INSERT INTO windows(start_at,end_at) VALUES($1,$2) ON CONFLICT(start_at) DO UPDATE SET start_at=EXCLUDED.start_at RETURNING generation").bind(Ts::from(w.start)).bind(Ts::from(w.end)).fetch_one(&self.pool).await?)
    }
    /// Read-only fence for a run; unlike generation(), this never creates work.
    pub async fn is_current_generation(&self, w: Interval, generation: i64) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM windows WHERE start_at=$1 AND generation=$2)",
        )
        .bind(Ts::from(w.start))
        .bind(generation)
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn save_timeline(&self, timeline: &Timeline, generation: i64) -> Result<bool> {
        // OCR results are immutable per source/blob/engine lineage. Include the
        // lead-in used by both providers; an OCR completion can change that input
        // without changing any event or compiled lane.
        let lineage: Vec<Value> = sqlx::query_scalar("SELECT json_object('id',id,'source_event_id',source_event_id,'sha256',sha256,'engine_version',engine_version) FROM ocr_derived WHERE source_start >= $1 AND source_start < $2 ORDER BY id")
            .bind(Ts::from(timeline.window.start - Duration::minutes(30))).bind(Ts::from(timeline.window.end))
            .fetch_all(&self.pool).await?;
        // Keep the full timeline for display, but exclude admission-ineligible
        // housekeeping from paid identity, including its lanes/coverage effects.
        let observations: Vec<_> = self
            .observations(timeline.window, true)
            .await?
            .into_iter()
            .filter(|o| {
                !matches!(
                    o.envelope.kind.as_str(),
                    "collector.health" | "clock.calibration" | "presence.span"
                )
            })
            .collect();
        let paid_timeline = hx_compile::compile(&observations, timeline.window);
        let identity = hash(&serde_json::to_vec(&(paid_timeline, lineage))?);
        Ok(sqlx::query("UPDATE windows SET timeline=$3,compiler_version=$4,compiled_generation=$2,dirty=false,timeline_hash=$5 WHERE start_at=$1 AND generation=$2")
            .bind(Ts::from(timeline.window.start)).bind(generation).bind(stored_json(serde_json::to_value(timeline)?)).bind(&timeline.compiler_version).bind(identity).execute(&self.pool).await?.rows_affected()==1)
    }
    /// Read the exact identity saved for this compiled generation. A concurrent
    /// input change must not attach a newer hash to an older interpretation.
    pub async fn compiled_timeline_hash(
        &self,
        w: Interval,
        generation: i64,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT timeline_hash FROM windows WHERE start_at=$1 AND generation=$2 AND compiled_generation=$2")
            .bind(Ts::from(w.start)).bind(generation).fetch_optional(&self.pool).await?.flatten())
    }
    pub async fn interpretation_state(&self, w: Interval) -> Result<InterpretationState> {
        Ok(sqlx::query_as("SELECT w.generation,w.canonical_revision,w.input_changed_at,w.revision24_finished, (r.id IS NULL OR (r.input_generation<>w.generation AND w.timeline_hash IS NOT NULL AND r.interpretation->>'$.input_manifest.timeline_hash' IS NOT NULL AND w.timeline_hash IS DISTINCT FROM r.interpretation->>'$.input_manifest.timeline_hash')) AS timeline_changed FROM windows w LEFT JOIN revisions r ON r.id=w.canonical_revision WHERE w.start_at=$1")
            .bind(Ts::from(w.start)).fetch_one(&self.pool).await?)
    }
    pub async fn finish_revision24(&self, w: Interval, generation: i64) -> Result<()> {
        sqlx::query(
            "UPDATE windows SET revision24_finished=true WHERE start_at=$1 AND generation=$2",
        )
        .bind(Ts::from(w.start))
        .bind(generation)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Reserve only one stale window per UTC day, including across scheduler restarts.
    /// Unchanged failed/deferred input cools down for seven UTC days.
    /// Historical first runs and exhausted 24h passes share this same cap.
    pub async fn schedule_daily_sweep(&self, now: DateTime<Utc>) -> Result<()> {
        let candidate_sql =
            "SELECT w.start_at,w.end_at,w.generation
             FROM windows w LEFT JOIN revisions r ON r.id=w.canonical_revision
             WHERE w.empty_generation IS DISTINCT FROM w.generation
               AND w.end_at+86400000000<=$1
               AND NOT w.dirty AND w.compiled_generation=w.generation
               AND (
                 w.canonical_revision IS NULL
                 OR (w.timeline_hash IS NOT NULL
                   AND r.interpretation->>'$.input_manifest.timeline_hash' IS NOT NULL
                   AND w.timeline_hash IS DISTINCT FROM r.interpretation->>'$.input_manifest.timeline_hash' AND (
                   w.end_at+172800000000<=$1
                   OR w.revision24_finished OR EXISTS (
                     SELECT 1 FROM jobs exhausted
                     WHERE exhausted.kind='interpret' AND exhausted.state='failed'
                       AND exhausted.revivals>=3
                       AND exhausted.payload->>'revision24'=1
                       AND exhausted.window_start=w.start_at
                   )
                 ))
               )
               AND NOT EXISTS (
                 SELECT 1 FROM daily_sweeps spent
                 WHERE spent.start_at=w.start_at AND spent.generation=w.generation
                   AND spent.day > $2
               )
               AND NOT EXISTS (
                 SELECT 1 FROM jobs j WHERE j.kind='interpret'
                   AND j.window_start=w.start_at
                   AND (j.state IN ('ready','running') OR (
                     j.state='failed' AND j.revivals<3
                     AND j.payload->>'revision24'=1
                     AND NOT w.revision24_finished
                     AND NOT (json_type(j.payload,'$.sweep_day') IS NOT NULL)
                   ))
               )
             ORDER BY w.end_at DESC LIMIT 1";
        let candidate = sqlx::query(candidate_sql)
            .bind(Ts::from(now))
            .bind((now.date_naive() - Duration::days(7)).to_string())
            .fetch_optional(&self.pool)
            .await?;
        let Some(selected) = candidate else {
            return Ok(());
        };
        let start: Ts = selected.get("start_at");
        let mut tx = self.begin_write().await?;
        // Recheck just the selected primary key under the writer. Reservations
        // still arbitrate the global daily cap across concurrent schedulers.
        let recheck = candidate_sql.replace(
            "WHERE w.empty_generation",
            "WHERE w.start_at=$3 AND w.empty_generation",
        );
        let candidate = sqlx::query(sqlx::AssertSqlSafe(recheck))
            .bind(Ts::from(now))
            .bind((now.date_naive() - Duration::days(7)).to_string())
            .bind(start)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(row) = candidate {
            let w = Interval {
                start: DateTime::<Utc>::from(row.get::<Ts, _>("start_at")),
                end: DateTime::<Utc>::from(row.get::<Ts, _>("end_at")),
            };
            let generation: i64 = row.get("generation");
            let reserved: Option<String> = sqlx::query_scalar("INSERT INTO daily_sweeps(day,start_at,generation) VALUES($1,$2,$3) ON CONFLICT DO NOTHING RETURNING day")
                .bind(now.date_naive().to_string()).bind(Ts::from(w.start)).bind(generation).fetch_optional(&mut *tx).await?;
            if reserved.is_some() {
                // Reservation takes over the one automatic catch-up opportunity.
                // Retire it under the window lock, before any provider work or
                // concurrent generation change can expose a fresh 24h slot.
                sqlx::query("UPDATE windows SET revision24_finished=true WHERE start_at=$1")
                    .bind(Ts::from(w.start))
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("INSERT INTO jobs(output_key,kind,payload,queue,available_at,window_start,generation) VALUES($1,'interpret',$2,'backfill',$3,$4,$5) ON CONFLICT(output_key) DO UPDATE SET payload=EXCLUDED.payload,revivals=jobs.revivals,request_version=jobs.request_version+1,state='ready',attempts=0,available_at=EXCLUDED.available_at,window_start=EXCLUDED.window_start,generation=EXCLUDED.generation,last_error=NULL WHERE jobs.state IN ('done','failed')")
                    .bind(format!("interpret:{}", w.start)).bind(json!({"window":w,"generation":generation,"sweep_day":now.date_naive()})).bind(Ts::now()).bind(Ts::from(w.start)).bind(generation).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn start_daily_sweep(&self, w: Interval, day: chrono::NaiveDate) -> Result<bool> {
        // A crash or failure after admission cannot purchase another run that day.
        Ok(sqlx::query("UPDATE daily_sweeps SET started=true WHERE day=$1 AND start_at=$2 AND NOT started AND day=$3")
            .bind(day.to_string()).bind(Ts::from(w.start)).bind(Utc::now().date_naive().to_string()).execute(&self.pool).await?.rows_affected()==1)
    }
    pub async fn release_daily_sweep(&self, w: Interval, day: chrono::NaiveDate) -> Result<()> {
        sqlx::query("UPDATE daily_sweeps SET started=false WHERE day=$1 AND start_at=$2")
            .bind(day.to_string())
            .bind(Ts::from(w.start))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Empty admission did no provider work; let the next candidate use this day.
    pub async fn skip_empty_daily_sweep(&self, w: Interval, day: chrono::NaiveDate) -> Result<()> {
        sqlx::query("DELETE FROM daily_sweeps WHERE day=$1 AND start_at=$2 AND NOT started")
            .bind(day.to_string())
            .bind(Ts::from(w.start))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn finish_empty(&self, w: Interval, generation: i64) -> Result<bool> {
        Ok(sqlx::query("UPDATE windows SET empty_generation=$2,dirty=false WHERE start_at=$1 AND generation=$2")
            .bind(Ts::from(w.start)).bind(generation).execute(&self.pool).await?.rows_affected() == 1)
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
            .bind(Ts::from(w.start-Duration::minutes(30))).bind(Ts::from(w.end)).fetch_all(&self.pool).await?;
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
        retry_busy("publish revision", |_| {
            self.publish_revision_attempt(w, generation, &value, output_key, namespaces)
        })
        .await
    }
    async fn publish_revision_attempt(
        &self,
        w: Interval,
        generation: i64,
        value: &Value,
        output_key: &str,
        namespaces: &[String],
    ) -> Result<(i64, bool)> {
        let mut content = value.clone();
        let agent_run = content.as_object_mut().and_then(|v| v.remove("agent_run"));
        let dependencies = revision_dependency_ids(&content, agent_run.as_ref());
        let content = stored_json(content);
        let agent_run = agent_run.map(stored_json);
        let mut tx = self.begin_write().await?;
        let window =
            sqlx::query("SELECT generation,canonical_revision FROM windows WHERE start_at=$1")
                .bind(Ts::from(w.start))
                .fetch_one(&mut *tx)
                .await?;
        let current: i64 = window.get("generation");
        let canonical: Option<i64> = window.get("canonical_revision");
        if value.get("agent_run").is_some() && current != generation {
            return Ok((canonical.unwrap_or_default(), false));
        }
        let inserted = sqlx::query("INSERT INTO revisions(start_at,input_generation,interpretation,output_key,input_namespaces,agent_run,created_at,has_dependency_record) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(output_key) DO NOTHING").bind(Ts::from(w.start)).bind(generation).bind(content).bind(output_key).bind(sqlx::types::Json(namespaces)).bind(agent_run).bind(Ts::now()).bind(dependencies.is_some()).execute(&mut *tx).await?;
        let revision = sqlx::query(
            "SELECT id,input_generation,start_at,interpretation FROM revisions WHERE output_key=$1",
        )
        .bind(output_key)
        .fetch_one(&mut *tx)
        .await?;
        let id: i64 = revision.get("id");
        if inserted.rows_affected() == 1 {
            record_revision_dependencies(&mut tx, id, dependencies.as_deref()).await?;
        }
        let publish = canonical.is_none_or(|canonical| canonical <= id)
            && current == revision.get::<i64, _>("input_generation")
            && w.start == DateTime::<Utc>::from(revision.get::<Ts, _>("start_at"));
        if publish {
            let previous:Option<Value>=sqlx::query_scalar("SELECT coalesce(interpretation->'tasks','null') FROM revisions WHERE id=(SELECT canonical_revision FROM windows WHERE start_at=$1)")
                .bind(Ts::from(w.start)).fetch_optional(&mut *tx).await?;
            let stored: Value = revision.get("interpretation");
            let open_tasks = |tasks: &Value| -> Vec<Value> {
                tasks
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|task| task["open"].as_bool() == Some(true))
                    .cloned()
                    .collect()
            };
            if open_tasks(previous.as_ref().unwrap_or(&Value::Null))
                != open_tasks(stored.get("tasks").unwrap_or(&Value::Null))
            {
                // A consumer with no prior open tasks records no dependency ID.
                // The same 16h lookback as prior_context must still become stale
                // when new tasks appear. Recorded dependencies use the indexed table;
                // legacy runs retain the lookback fallback. Mark only; payment stays cadence-gated.
                sqlx::query("UPDATE windows SET generation=generation+1,dirty=true WHERE start_at >= $1 AND start_at < $1+57600000000 AND (canonical_revision IN (SELECT revision_id FROM revision_dependencies WHERE prior_revision_id IN (SELECT id FROM revisions WHERE start_at=$2)) OR $3 OR canonical_revision IS NULL OR canonical_revision IN (SELECT id FROM revisions WHERE NOT has_dependency_record))")
                    .bind(Ts::from(w.end)).bind(Ts::from(w.start)).bind(!open_tasks(stored.get("tasks").unwrap_or(&Value::Null)).is_empty()).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE windows SET canonical_revision=$2 WHERE start_at=$1")
                .bind(Ts::from(w.start))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok((id, publish))
    }
    pub async fn search(&self, q: &str, raw: bool, prefixes: &[String]) -> Result<Vec<Value>> {
        let query = sqlite::search_query(q);
        // Small posting lists are cheaper to sort; common terms should walk the
        // time index and stop at 100 permitted hits instead of sorting history.
        let hits: Vec<i64> = sqlx::query_scalar(
            "SELECT rowid FROM normalized_fts WHERE normalized_fts MATCH $1 LIMIT 1001",
        )
        .bind(&query)
        .fetch_all(&self.pool)
        .await?;
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let rows = if hits.len() <= 1000 {
            sqlx::query("SELECT e.id,n.search_text,e.source_start FROM events e JOIN normalized n ON n.event_id=e.id WHERE e.id IN (SELECT value FROM json_each($1)) AND ($2 OR e.sensitivity='normal') AND EXISTS(SELECT 1 FROM json_each($3) p WHERE substr(e.connector_instance,1,length(p.value))=p.value) ORDER BY e.source_start DESC LIMIT 100")
                .bind(sqlx::types::Json(&hits)).bind(raw).bind(sqlx::types::Json(prefixes)).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT e.id,n.search_text,e.source_start FROM events e INDEXED BY events_time CROSS JOIN normalized n ON n.event_id=e.id WHERE ($2 OR e.sensitivity='normal') AND EXISTS(SELECT 1 FROM json_each($3) p WHERE substr(e.connector_instance,1,length(p.value))=p.value) AND EXISTS(SELECT 1 FROM normalized_fts WHERE normalized_fts MATCH $1 AND rowid=e.id) ORDER BY e.source_start DESC LIMIT 100")
                .bind(query).bind(raw).bind(sqlx::types::Json(prefixes)).fetch_all(&self.pool).await?
        };
        Ok(rows.into_iter().map(|r| json!({"id":r.get::<i64,_>("id"),"text":r.get::<String,_>("search_text"),"at":DateTime::<Utc>::from(r.get::<Ts, _>("source_start"))})).collect())
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
        // Stage and sync the contents before acquiring any database lock. Only
        // the atomic publication and its directory durability need the writer lock.
        let temp = tempfile::NamedTempFile::new_in(path.parent().context("blob parent")?)?;
        let mut file = tokio::fs::File::from_std(temp.reopen()?);
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        let mut tx = self.begin_write().await?;
        sqlx::query("INSERT INTO blobs(sha256,created_at) VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(sha)
            .bind(Ts::now())
            .execute(&mut *tx)
            .await?;
        let published: bool = sqlx::query_scalar("SELECT published FROM blobs WHERE sha256=$1")
            .bind(sha)
            .fetch_one(&mut *tx)
            .await?;
        // A gc crash can leave a published row with no file, so the row alone
        // doesn't prove the bytes are there.
        if published && path.try_exists()? {
            // The client may now discard its copy, so retention restarts here.
            sqlx::query("UPDATE blobs SET created_at=$2 WHERE sha256=$1")
                .bind(sha)
                .bind(Ts::now())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(false);
        }
        std::fs::rename(temp.path(), &path)?;
        std::fs::File::open(path.parent().context("blob parent")?)?.sync_all()?;
        sqlx::query(
            "UPDATE blobs SET published=true,evidence_expired=false,bytes=$2,created_at=$3 WHERE sha256=$1",
        )
        .bind(sha)
        .bind(bytes.len() as i64)
        .bind(Ts::now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
    pub async fn gc(&self) -> Result<usize> {
        let hashes:Vec<String>=sqlx::query_scalar("SELECT sha256 FROM blobs WHERE retention_class='chunk' AND pins=0 AND published AND created_at < $1 ORDER BY sha256").bind(Ts::from(Utc::now()-Duration::days(30))).fetch_all(&self.pool).await?;
        let mut expired = 0;
        for sha in &hashes {
            let mut tx = self.begin_write().await?;
            #[cfg(test)]
            if let Some(boundary) = &self.gc_boundary {
                tx.observe_gc_commit(boundary.clone());
            }
            let eligible: Option<String> = sqlx::query_scalar("SELECT sha256 FROM blobs WHERE sha256=$1 AND retention_class='chunk' AND pins=0 AND published AND created_at < $2")
                .bind(sha).bind(Ts::from(Utc::now()-Duration::days(30))).fetch_optional(&mut *tx).await?;
            if eligible.is_none() {
                tx.commit().await?;
                continue;
            }
            // Unlink under the same writer lock used by upload's rename. Keep this
            // synchronous so cancellation cannot leave an unlink in flight after
            // releasing the transaction and race a later upload.
            #[cfg(test)]
            if let Some(boundary) = &self.gc_boundary {
                boundary.before_unlink().await;
            }
            match std::fs::remove_file(self.blob_path(sha)?) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            sqlx::query("UPDATE blobs SET published=false,evidence_expired=true WHERE sha256=$1")
                .bind(sha)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            expired += 1;
        }
        Ok(expired)
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
        let sensitivity:String=sqlx::query_scalar("SELECT CASE max(CASE sensitivity WHEN 'restricted' THEN 2 WHEN 'sensitive' THEN 1 ELSE 0 END) WHEN 0 THEN 'normal' WHEN 1 THEN 'sensitive' ELSE 'restricted' END FROM events WHERE id IN (SELECT value FROM json_each($1))")
            .bind(sqlx::types::Json(evidence)).fetch_one(&self.pool).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT id) FROM events WHERE id IN (SELECT value FROM json_each($1))",
        )
        .bind(sqlx::types::Json(evidence))
        .fetch_one(&self.pool)
        .await?;
        let unique: std::collections::HashSet<_> = evidence.iter().collect();
        if count as usize != unique.len() {
            bail!("unknown evidence");
        }
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE windows AS w SET generation=generation+1,dirty=true WHERE EXISTS (SELECT 1 FROM events e WHERE e.id IN (SELECT value FROM json_each($1)) AND e.source_start<w.end_at AND CASE WHEN e.source_end>e.source_start THEN e.source_end>w.start_at ELSE e.source_start>=w.start_at END)")
            .bind(sqlx::types::Json(evidence))
            .execute(&mut *tx)
            .await?;
        let id=sqlx::query_scalar("INSERT INTO entity_assertions(entity,assertion,evidence,sensitivity) VALUES($1,$2,$3,$4) RETURNING id").bind(entity).bind(stored_json(assertion)).bind(sqlx::types::Json(evidence)).bind(sensitivity).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn evidence(&self, id: i64) -> Result<Option<Value>> {
        let value:Option<Value>=sqlx::query_scalar("SELECT json_object('id',e.id,'envelope',json(e.envelope),'sensitivity',e.sensitivity,'blobs',coalesce((SELECT json_group_array(json_object('sha256',b.sha256,'evidence_expired',json(CASE WHEN b.evidence_expired THEN 'true' ELSE 'false' END))) FROM event_blobs eb JOIN blobs b USING(sha256) WHERE eb.event_id=e.id),'[]')) FROM events e WHERE id=$1").bind(id).fetch_optional(&self.pool).await?;
        if let Some(mut value) = value {
            let derived: Vec<Value> = sqlx::query_scalar("SELECT json_object('id',id,'envelope',json(envelope),'sensitivity',sensitivity) FROM ocr_derived WHERE source_event_id=$1 ORDER BY id")
                .bind(id).fetch_all(&self.pool).await?;
            value["derived_evidence"] = json!(derived);
            return Ok(Some(value));
        }
        Ok(sqlx::query_scalar("SELECT json_object('id',d.id,'envelope',json(d.envelope),'sensitivity',d.sensitivity,'source_event_id',d.source_event_id,'blobs',json_array(json_object('sha256',b.sha256,'evidence_expired',json(CASE WHEN b.evidence_expired THEN 'true' ELSE 'false' END)))) FROM ocr_derived d JOIN blobs b USING(sha256) WHERE d.id=$1")
            .bind(id).fetch_optional(&self.pool).await?)
    }
}

/// Update a single durable work slot without stealing an in-flight lease.
/// A completion observes the request counter and requeues if input changed under it.
pub async fn request_window_job(
    tx: &mut WriteTransaction,
    kind: &str,
    window: Interval,
    generation: i64,
    now: Ts,
) -> Result<()> {
    sqlx::query("INSERT INTO jobs(output_key,kind,payload,available_at,window_start,generation) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(output_key) DO UPDATE SET payload=EXCLUDED.payload,revivals=0,request_version=jobs.request_version+1,state=CASE WHEN jobs.state='running' THEN 'running' ELSE 'ready' END,attempts=CASE WHEN jobs.state='running' THEN jobs.attempts ELSE 0 END,available_at=EXCLUDED.available_at,window_start=EXCLUDED.window_start,generation=EXCLUDED.generation,last_error=NULL WHERE jobs.generation < EXCLUDED.generation")
        .bind(format!("{kind}:{}",window.start)).bind(kind).bind(json!({"window":window,"generation":generation})).bind(now).bind(Ts::from(window.start)).bind(generation).execute(&mut **tx).await?;
    Ok(())
}

/// Refresh automatic input without refilling paid attempts or revival rounds.
/// Done/failed slots stay terminal; an input generation is not a paid request.
pub async fn request_interpret_job(
    tx: &mut WriteTransaction,
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
    sqlx::query("INSERT INTO jobs(output_key,kind,payload,queue,available_at,window_start,generation) VALUES($1,'interpret',$2,$3,$4,$5,$6) ON CONFLICT(output_key) DO UPDATE SET payload=EXCLUDED.payload,window_start=EXCLUDED.window_start,generation=EXCLUDED.generation,request_version=jobs.request_version+1,available_at=CASE WHEN jobs.state='ready' THEN EXCLUDED.available_at ELSE jobs.available_at END WHERE coalesce(jobs.generation,0)<EXCLUDED.generation")
        .bind(format!("{slot}:{}", window.start))
        .bind(json!({"window":window,"generation":generation,"revision24":revision24}))
        .bind(if revision24 { "backfill" } else { "interactive" })
        .bind(Ts::from(at)).bind(Ts::from(window.start)).bind(generation).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(test)]
mod concurrency_tests;

mod reads;
pub use reads::*;

mod pairing;
pub use pairing::{PairCodeInfo, TokenInfo};

mod processing;
pub use processing::*;

mod locks;

mod transaction;
pub use transaction::*;

mod ocr;
pub use ocr::*;

mod persistence;
pub use persistence::{
    job_window_start, record_revision_dependencies, revision_dependency_ids, stored_json,
};

mod contention;
pub use contention::{is_busy, retry_busy};
