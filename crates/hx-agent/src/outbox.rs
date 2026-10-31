use anyhow::{Result, ensure};
use hx_schema::{kinds::*, *};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;

// Additional bounded headroom, unavailable to ordinary captures. It holds compact
// loss summaries and restart closures even when the configured event budget is full.
const RECOVERY_RESERVE: u64 = 64 * 1024;
#[derive(Debug)]
struct BudgetRejected(&'static str);
impl std::fmt::Display for BudgetRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for BudgetRejected {}

pub struct Outbox {
    db: Connection,
    event_budget: u64,
    media_budget: u64,
}
#[derive(Debug)]
pub struct Loss {
    pub sha256: String,
    pub bytes: u64,
}
impl Outbox {
    pub fn open(path: &Path, event_budget: u64, media_budget: u64) -> Result<Self> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, identity TEXT UNIQUE NOT NULL, json BLOB NOT NULL, bytes INTEGER NOT NULL, created INTEGER NOT NULL DEFAULT (unixepoch())); CREATE TABLE IF NOT EXISTS blobs (sha TEXT PRIMARY KEY, media_type TEXT NOT NULL, data BLOB NOT NULL, bytes INTEGER NOT NULL, created INTEGER NOT NULL DEFAULT(unixepoch())); CREATE TABLE IF NOT EXISTS refs (event INTEGER NOT NULL, sha TEXT NOT NULL, PRIMARY KEY(event,sha)); CREATE TABLE IF NOT EXISTS pending (singleton INTEGER PRIMARY KEY CHECK(singleton=1), json BLOB NOT NULL); CREATE TABLE IF NOT EXISTS losses (id INTEGER PRIMARY KEY, sha TEXT NOT NULL, bytes INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS sequences (connector TEXT PRIMARY KEY, value INTEGER NOT NULL);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS reserved_events (identity TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS open_intervals (connector TEXT NOT NULL, source_key TEXT NOT NULL, json BLOB NOT NULL, PRIMARY KEY(connector,source_key)); CREATE TABLE IF NOT EXISTS observed (connector TEXT PRIMARY KEY, json BLOB NOT NULL); CREATE TABLE IF NOT EXISTS checkpoint_version (version INTEGER PRIMARY KEY);")?;
        // Migrate the outstanding history once. New checkpoints survive acknowledgments.
        if db.query_row("SELECT COUNT(*) FROM checkpoint_version", [], |r| {
            r.get::<_, i64>(0)
        })? == 0
        {
            let tx = db.unchecked_transaction()?;
            let rows = tx
                .prepare("SELECT json FROM events ORDER BY id")?
                .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for json in rows {
                checkpoint(&tx, &serde_json::from_slice(&json)?)?;
            }
            tx.execute("INSERT INTO checkpoint_version VALUES(1)", [])?;
            tx.commit()?;
        }
        Ok(Self {
            db,
            event_budget,
            media_budget,
        })
    }
    pub fn enqueue(
        &mut self,
        envelope: &Envelope,
        blobs: &[(BlobRef, Vec<u8>)],
    ) -> Result<Vec<Loss>> {
        envelope.validate()?;
        let mut envelope = envelope.clone();
        let json = serde_json::to_vec(&envelope)?;
        let tx = self.db.transaction()?;
        if tx
            .query_row(
                "SELECT 1 FROM events WHERE identity=?",
                [envelope.observation_id.to_string()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
        {
            return Ok(vec![]);
        }
        let used: u64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM events WHERE identity NOT IN (SELECT identity FROM reserved_events)", [], |r| {
            r.get::<_, i64>(0)
        })? as u64;
        ensure!(
            used + json.len() as u64 <= self.event_budget,
            BudgetRejected("event_budget")
        );
        for (reference, data) in blobs {
            ensure!(envelope.blobs.contains(reference), "unreferenced blob");
            ensure!(
                reference.sha256 == hx_schema::sha256_hex(data)
                    && reference.bytes == data.len() as u64,
                "blob mismatch"
            );
            ensure!(
                reference.bytes <= self.media_budget,
                BudgetRejected("oversized_media")
            );
        }
        for reference in &envelope.blobs {
            ensure!(
                blobs.iter().any(|(b, _)| b == reference)
                    || tx
                        .query_row(
                            "SELECT 1 FROM blobs WHERE sha=?",
                            [&reference.sha256],
                            |r| r.get::<_, i64>(0)
                        )
                        .optional()?
                        .is_some(),
                "missing blob"
            );
        }
        // In-flight batches are immutable across retries, including their blob manifests.
        let pending: Option<Vec<u8>> = tx
            .query_row("SELECT json FROM pending", [], |r| r.get(0))
            .optional()?;
        let protected: Vec<String> = pending
            .map(|b| serde_json::from_slice::<UploadBatch>(&b))
            .transpose()?
            .into_iter()
            .flat_map(|b| b.envelopes)
            .flat_map(|e| e.blobs)
            .map(|b| b.sha256)
            .collect();
        let mut losses = vec![];
        for (reference, data) in blobs {
            tx.execute(
                "INSERT OR IGNORE INTO blobs(sha,media_type,data,bytes) VALUES(?,?,?,?)",
                params![
                    reference.sha256,
                    reference.media_type,
                    data,
                    reference.bytes as i64
                ],
            )?;
        }
        loop {
            let used: u64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM blobs", [], |r| {
                r.get::<_, i64>(0)
            })? as u64;
            if used <= self.media_budget {
                break;
            }
            let mut query = tx.prepare("SELECT sha,bytes FROM blobs ORDER BY created,rowid")?;
            let rows = query
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(query);
            let Some((sha, bytes)) = rows.into_iter().find(|(sha, _)| {
                !protected.contains(sha) && !envelope.blobs.iter().any(|b| &b.sha256 == sha)
            }) else {
                return Err(BudgetRejected("pinned_media").into());
            };
            tx.execute(
                "INSERT INTO losses(sha,bytes) VALUES(?,?)",
                params![sha, bytes as i64],
            )?;
            // Replace unuploaded media observations with an explicit loss event atomically. In-flight observations are protected above.
            let mut affected=tx.prepare("SELECT json FROM events WHERE id IN (SELECT event FROM refs WHERE sha=?) ORDER BY id")?;
            let originals = affected
                .query_map([&sha], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(affected);
            tx.execute(
                "DELETE FROM events WHERE id IN (SELECT event FROM refs WHERE sha=?)",
                [&sha],
            )?;
            tx.execute(
                "DELETE FROM refs WHERE event NOT IN (SELECT id FROM events)",
                [],
            )?;
            tx.execute("DELETE FROM blobs WHERE sha=?", [&sha])?;
            for json in originals {
                let old: Envelope = serde_json::from_slice(&json)?;
                let payload = hx_schema::kinds::CorePayload::CollectorHealth(
                    hx_schema::kinds::CollectorHealth {
                        collector: "media".into(),
                        status: hx_schema::kinds::CollectorStatus::Failed,
                        last_sequence: old.sequence,
                        last_observed_at: Some(old.time.observed_at),
                        queue_age_ms: None,
                        dropped: 1,
                        detail: Some(format!(
                            "media_evicted sha256={sha} bytes={bytes} original={}",
                            serde_json::to_string(&old)?
                        )),
                    },
                );
                let mut loss = Envelope::core(
                    old.connector_instance,
                    hx_schema::SourceKey::new(format!("loss:{}", ulid::Ulid::generate())),
                    hx_schema::Revision::append_only(),
                    hx_schema::Operation::Observe,
                    &payload,
                    old.time,
                );
                loss.device = old.device;
                loss.sensitivity = old.sensitivity;
                loss.coverage = Some(hx_schema::Coverage {
                    scope: "media_evicted".into(),
                    completeness: hx_schema::Completeness::Unknown,
                });
                loss.time.source_start = Some(chrono::Utc::now());
                loss.time.source_end = None;
                loss.time.observed_at = chrono::Utc::now();
                loss.time.mono_end = None;
                if let Some(stamp) = loss.time.mono_start.as_mut() {
                    stamp.nanos = hx_macos_sys::continuous_nanos();
                    stamp.boot_id = hx_schema::BootId::new(hx_macos_sys::boot_id()?);
                }
                allocate_sequence(&tx, &mut loss)?;
                let json = serde_json::to_vec(&loss)?;
                tx.execute(
                    "INSERT INTO events(identity,json,bytes) VALUES(?,?,?)",
                    params![loss.observation_id.to_string(), json, json.len() as i64],
                )?;
            }
            tx.execute("DELETE FROM losses WHERE id NOT IN (SELECT id FROM losses ORDER BY id DESC LIMIT 1024)",[])?;
            losses.push(Loss { sha256: sha, bytes });
        }
        allocate_sequence(&tx, &mut envelope)?;
        let json = serde_json::to_vec(&envelope)?;
        tx.execute(
            "INSERT INTO events(identity,json,bytes) VALUES(?,?,?)",
            params![envelope.observation_id.to_string(), json, json.len() as i64],
        )?;
        let id = tx.last_insert_rowid();
        for b in &envelope.blobs {
            tx.execute(
                "INSERT INTO refs(event,sha) VALUES(?,?)",
                params![id, b.sha256],
            )?;
        }
        let used: i64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM events WHERE identity NOT IN (SELECT identity FROM reserved_events)", [], |r| {
            r.get(0)
        })?;
        ensure!(
            used as u64 <= self.event_budget,
            BudgetRejected("eviction_loss_budget")
        );
        checkpoint(&tx, &envelope)?;
        tx.commit()?;
        Ok(losses)
    }
    /// Budget rejection is a durable, content-free loss, not a fatal collector error.
    /// Other storage/validation failures still propagate to stop unsafe capture.
    pub fn capture(&mut self, envelope: &Envelope, blobs: &[(BlobRef, Vec<u8>)]) -> Result<bool> {
        match self.enqueue(envelope, blobs) {
            Ok(_) => Ok(true),
            Err(error) => {
                let Some(reason) = error.downcast_ref::<BudgetRejected>() else {
                    return Err(error);
                };
                self.record_capture_loss(envelope, reason.0)?;
                Ok(false)
            }
        }
    }
    fn record_capture_loss(&mut self, original: &Envelope, reason: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        let pending: Option<Vec<u8>> = tx
            .query_row("SELECT json FROM pending", [], |r| r.get(0))
            .optional()?;
        let protected: Vec<_> = pending
            .map(|json| serde_json::from_slice::<UploadBatch>(&json))
            .transpose()?
            .into_iter()
            .flat_map(|batch| batch.envelopes)
            .map(|e| e.observation_id)
            .collect();
        let rows = tx.prepare("SELECT json FROM events WHERE identity IN (SELECT identity FROM reserved_events) ORDER BY id DESC")?
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut previous = None;
        for json in rows {
            let e: Envelope = serde_json::from_slice(&json)?;
            if e.connector_instance == original.connector_instance
                && !protected.contains(&e.observation_id)
                && e.coverage
                    .as_ref()
                    .is_some_and(|c| c.scope == "capture_loss")
                && e.payload["reason"] == reason
            {
                previous = Some(e);
                break;
            }
        }
        let count = previous
            .as_ref()
            .and_then(|e| e.payload["dropped"].as_u64())
            .unwrap_or(0)
            .saturating_add(1);
        let first = previous
            .as_ref()
            .and_then(|e| e.payload["first_observation_id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| original.observation_id.to_string());
        let payload = CorePayload::CollectorHealth(CollectorHealth {
            collector: original
                .kind
                .as_str()
                .split('.')
                .next()
                .filter(|name| {
                    ["focus", "presence", "input", "ui", "text", "media", "clock"].contains(name)
                })
                .unwrap_or("capture")
                .into(),
            status: CollectorStatus::Failed,
            last_sequence: original.sequence,
            last_observed_at: Some(original.time.observed_at),
            queue_age_ms: None,
            dropped: count,
            detail: Some(format!(
                "capture_rejected reason={reason} first={first} latest={}",
                original.observation_id
            )),
        });
        let mut loss = Envelope::core(
            original.connector_instance.clone(),
            SourceKey::new(format!("capture-loss:{}", ulid::Ulid::generate())),
            Revision::append_only(),
            Operation::Observe,
            &payload,
            ObservationTime::point(original.time.observed_at),
        );
        loss.device = original.device.clone();
        loss.sensitivity = original.sensitivity;
        loss.coverage = Some(Coverage {
            scope: "capture_loss".into(),
            completeness: Completeness::Unknown,
        });
        loss.payload["reason"] = serde_json::json!(reason);
        loss.payload["first_observation_id"] = serde_json::json!(first);
        if let Some(old) = previous {
            loss.time.source_start = old.time.source_start;
            loss.time.source_end = Some(
                original
                    .time
                    .observed_at
                    .max(old.time.source_start.unwrap_or(original.time.observed_at)),
            );
            tx.execute(
                "DELETE FROM events WHERE identity=?",
                [old.observation_id.to_string()],
            )?;
            tx.execute(
                "DELETE FROM reserved_events WHERE identity=?",
                [old.observation_id.to_string()],
            )?;
        }
        insert_reserved(&tx, &mut loss)?;
        tx.commit()?;
        Ok(())
    }
    /// Close the preceding run's intervals at its last durable observation, never
    /// at restart. The unknown downtime is an explicit, content-free gap.
    pub fn reconcile(&mut self, now: ObservationTime) -> Result<()> {
        let tx = self.db.transaction()?;
        let rows = tx
            .prepare("SELECT json FROM open_intervals ORDER BY connector,source_key")?
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut interrupted = std::collections::BTreeMap::new();
        for json in rows {
            let old: Envelope = serde_json::from_slice(&json)?;
            let json: Vec<u8> = tx.query_row(
                "SELECT json FROM observed WHERE connector=?",
                [old.connector_instance.as_str()],
                |r| r.get(0),
            )?;
            let last: ObservationTime = serde_json::from_slice(&json)?;
            let mut closed = old.clone();
            closed.observation_id = ObservationId::generate();
            closed.revision = Revision::new(
                old.revision
                    .as_str()
                    .parse::<u64>()?
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("interval revision exhausted"))?
                    .to_string(),
            );
            closed.time.source_end = last
                .source_start
                .map(|end| end.max(old.time.source_start.unwrap_or(end)));
            closed.time.mono_end = same_clock_end(&old.time, &last);
            closed.time.observed_at = now.observed_at;
            closed.coverage = Some(Coverage {
                scope: "capture_interrupted".into(),
                completeness: Completeness::Unknown,
            });
            if closed.kind.as_str() == "policy.gap" {
                closed.payload["open"] = serde_json::json!(false);
            }
            insert_reserved(&tx, &mut closed)?;
            interrupted.insert(old.connector_instance.to_string(), (old, last));
        }
        for (_, (old, last)) in interrupted {
            let mut gap = old;
            gap.observation_id = ObservationId::generate();
            gap.source_key = SourceKey::new(format!("interruption:{}", ulid::Ulid::generate()));
            gap.revision = Revision::new("0");
            gap.kind = Kind::new("policy.gap");
            gap.blobs.clear();
            gap.payload = serde_json::json!({"reason":"capture_interrupted","open":false});
            gap.time = last;
            gap.time.source_end = now
                .source_start
                .map(|end| end.max(gap.time.source_start.unwrap_or(end)));
            gap.time.mono_end = same_clock_end(&gap.time, &now);
            // A reboot changes monotonic domains; UTC alone bounds downtime.
            if gap.time.mono_end.is_none() {
                gap.time.mono_start = None;
            }
            gap.time.observed_at = now.observed_at;
            gap.coverage = Some(Coverage {
                scope: "capture_interrupted".into(),
                completeness: Completeness::Unknown,
            });
            insert_reserved(&tx, &mut gap)?;
        }
        tx.execute("DELETE FROM open_intervals", [])?;
        tx.commit()?;
        Ok(())
    }
    pub fn batch(&mut self) -> Result<Option<UploadBatch>> {
        if let Some(json) = self
            .db
            .query_row("SELECT json FROM pending", [], |r| r.get::<_, Vec<u8>>(0))
            .optional()?
        {
            return Ok(Some(serde_json::from_slice(&json)?));
        }
        let mut query = self
            .db
            .prepare("SELECT json FROM events ORDER BY id LIMIT 128")?;
        let rows = query
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        let envelopes = rows
            .iter()
            .map(|j| serde_json::from_slice(j))
            .collect::<Result<Vec<_>, _>>()?;
        if envelopes.is_empty() {
            return Ok(None);
        }
        let batch = UploadBatch {
            batch_id: ulid::Ulid::generate(),
            envelopes,
        };
        self.db.execute(
            "INSERT INTO pending VALUES(1,?)",
            [serde_json::to_vec(&batch)?],
        )?;
        Ok(Some(batch))
    }
    pub fn blob(&self, sha: &str) -> Result<Option<(String, Vec<u8>)>> {
        Ok(self
            .db
            .query_row(
                "SELECT media_type,data FROM blobs WHERE sha=?",
                [sha],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
    pub fn ack(&mut self, ack: &UploadAck) -> Result<()> {
        let Some(batch) = self.batch()? else {
            anyhow::bail!("no pending batch");
        };
        ensure!(batch.batch_id == ack.batch_id, "ack batch mismatch");
        ensure!(
            ack.accepted as usize + ack.duplicates as usize + ack.rejected.len()
                == batch.envelopes.len(),
            "incomplete ack"
        );
        ensure!(
            ack.rejected.is_empty(),
            "server rejected observations; retained for investigation"
        );
        let tx = self.db.transaction()?;
        for e in batch.envelopes {
            tx.execute(
                "DELETE FROM refs WHERE event IN (SELECT id FROM events WHERE identity=?)",
                [e.observation_id.to_string()],
            )?;
            tx.execute(
                "DELETE FROM events WHERE identity=?",
                [e.observation_id.to_string()],
            )?;
        }
        tx.execute(
            "DELETE FROM blobs WHERE sha NOT IN (SELECT sha FROM refs)",
            [],
        )?;
        tx.execute(
            "DELETE FROM reserved_events WHERE identity NOT IN (SELECT identity FROM events)",
            [],
        )?;
        tx.execute("DELETE FROM pending", [])?;
        tx.commit()?;
        Ok(())
    }
    pub fn stats(&self) -> Result<(i64, i64, i64)> {
        Ok((
            self.db
                .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?,
            self.db
                .query_row("SELECT COALESCE(SUM(bytes),0) FROM events", [], |r| {
                    r.get(0)
                })?,
            self.db
                .query_row("SELECT COALESCE(SUM(bytes),0) FROM blobs", [], |r| r.get(0))?,
        ))
    }
    pub fn queue_age_ms(&self) -> Result<i64> {
        Ok(self.db.query_row(
            "SELECT COALESCE(MAX(0,unixepoch()-MIN(created))*1000,0) FROM events",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn losses(&self) -> Result<Vec<Loss>> {
        Ok(self
            .db
            .prepare("SELECT sha,bytes FROM losses ORDER BY id")?
            .query_map([], |r| {
                Ok(Loss {
                    sha256: r.get(0)?,
                    bytes: r.get::<_, i64>(1)? as u64,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
}

fn allocate_sequence(tx: &Transaction<'_>, envelope: &mut Envelope) -> Result<()> {
    let last: Option<i64> = tx
        .query_row(
            "SELECT value FROM sequences WHERE connector=?",
            [envelope.connector_instance.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let next = last
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("sequence exhausted"))?
        .max(i64::try_from(envelope.sequence.unwrap_or(0))?);
    envelope.sequence = Some(next as u64);
    tx.execute("INSERT INTO sequences(connector,value) VALUES(?,?) ON CONFLICT(connector) DO UPDATE SET value=excluded.value",params![envelope.connector_instance.as_str(),next])?;
    Ok(())
}

fn same_clock_end(start: &ObservationTime, end: &ObservationTime) -> Option<MonoStamp> {
    let begin = start.mono_start.as_ref()?;
    let mut end = end.mono_start.clone()?;
    if begin.boot_id != end.boot_id || begin.domain != end.domain {
        return None;
    }
    end.nanos = end.nanos.max(begin.nanos);
    Some(end)
}
fn insert_reserved(tx: &Transaction<'_>, envelope: &mut Envelope) -> Result<()> {
    allocate_sequence(tx, envelope)?;
    envelope.validate()?;
    let json = serde_json::to_vec(envelope)?;
    let used = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM events WHERE identity IN (SELECT identity FROM reserved_events)", [], |r|r.get::<_, i64>(0))? as u64;
    ensure!(
        used + json.len() as u64 <= RECOVERY_RESERVE,
        "durable recovery reserve exhausted"
    );
    tx.execute(
        "INSERT INTO events(identity,json,bytes) VALUES(?,?,?)",
        params![envelope.observation_id.to_string(), json, json.len() as i64],
    )?;
    tx.execute(
        "INSERT INTO reserved_events VALUES(?)",
        [envelope.observation_id.to_string()],
    )?;
    Ok(())
}
fn checkpoint(tx: &Transaction<'_>, envelope: &Envelope) -> Result<()> {
    let previous: Option<Vec<u8>> = tx
        .query_row(
            "SELECT json FROM observed WHERE connector=?",
            [envelope.connector_instance.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    if previous
        .map(|json| serde_json::from_slice::<ObservationTime>(&json))
        .transpose()?
        .is_none_or(|last| envelope.time.observed_at >= last.observed_at)
    {
        let mut time = envelope.time.clone();
        time.source_start = Some(envelope.time.observed_at);
        time.source_end = None;
        time.mono_start = envelope.time.mono_end.clone().or(time.mono_start);
        time.mono_end = None;
        tx.execute("INSERT INTO observed(connector,json) VALUES(?,?) ON CONFLICT(connector) DO UPDATE SET json=excluded.json", params![envelope.connector_instance.as_str(),serde_json::to_vec(&time)?])?;
    }
    // Only this agent's interval protocol is restart-managed. Companion kinds and
    // revisions are open-ended; they must not be interpreted as native trackers.
    let native_span = envelope
        .connector_instance
        .as_str()
        .starts_with("macos-agent:")
        && ["focus.span", "presence.span"].contains(&envelope.kind.as_str());
    let native_gap = envelope.kind.as_str() == "policy.gap"
        && envelope.source_key.as_str().starts_with("policy-gap:")
        && envelope.revision.as_str().parse::<u64>().is_ok();
    if envelope.operation == Operation::Upsert && (native_span || native_gap) {
        let open = if envelope.kind.as_str() == "policy.gap" {
            envelope.payload["open"] == true
        } else {
            envelope.time.source_end.is_none()
        };
        if open {
            tx.execute("INSERT INTO open_intervals(connector,source_key,json) VALUES(?,?,?) ON CONFLICT(connector,source_key) DO UPDATE SET json=excluded.json", params![envelope.connector_instance.as_str(),envelope.source_key.as_str(),serde_json::to_vec(envelope)?])?;
        } else {
            tx.execute(
                "DELETE FROM open_intervals WHERE connector=? AND source_key=?",
                params![
                    envelope.connector_instance.as_str(),
                    envelope.source_key.as_str()
                ],
            )?;
        }
    }
    Ok(())
}
