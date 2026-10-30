use anyhow::{Result, ensure};
use hx_schema::{BlobRef, Envelope, UploadAck, UploadBatch};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;

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
        let used: u64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM events", [], |r| {
            r.get::<_, i64>(0)
        })? as u64;
        ensure!(
            used + json.len() as u64 <= self.event_budget,
            "event budget exhausted; capture must pause (no unacknowledged event removed)"
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
                "blob exceeds media budget"
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
                anyhow::bail!("media budget pinned by pending uploads");
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
                        collector: "outbox.media".into(),
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
        let used: i64 = tx.query_row("SELECT COALESCE(SUM(bytes),0) FROM events", [], |r| {
            r.get(0)
        })?;
        ensure!(
            used as u64 <= self.event_budget,
            "loss event exceeds event budget"
        );
        tx.commit()?;
        Ok(losses)
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
