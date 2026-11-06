use crate::postgres::Snapshot;
use anyhow::{Context, Result, ensure};
use futures_util::TryStreamExt;
use hx_store::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read, path::Path};

// Only the published interpretation's episode/claim fields are citations.
// agent_run, input_manifest, prior-context responses and rejected check_failures
// are immutable audit records, and may contain failed or hallucinated lookups.
fn citations(value: &Value, ids: &mut BTreeSet<i64>) -> Result<()> {
    fn add(value: Option<&Value>, ids: &mut BTreeSet<i64>) -> Result<()> {
        if let Some(value) = value {
            for id in value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("invalid published citation list"))?
            {
                ids.insert(
                    id.as_i64()
                        .ok_or_else(|| anyhow::anyhow!("invalid published citation ID"))?,
                );
            }
        }
        Ok(())
    }
    for episode in value
        .get("episodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        add(episode.get("evidence"), ids)?;
        for claim in episode
            .get("claims")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            add(claim.get("evidence"), ids)?;
            add(claim.get("outcome_evidence"), ids)?;
        }
    }
    Ok(())
}

pub async fn evidence(source: &mut Snapshot, store: &Store) -> Result<(u64, u64, u64)> {
    let mut citations_checked = 0;
    for sql in [
        "SELECT interpretation FROM revisions ORDER BY id",
        "SELECT evidence FROM entity_assertions ORDER BY id",
    ] {
        let mut stream = sqlx::query_scalar::<_, String>(sql).fetch(&store.pool);
        while let Some(text) = stream.try_next().await? {
            let value: Value = serde_json::from_str(&text)?;
            let mut ids = BTreeSet::new();
            if value.is_array() {
                for id in value.as_array().expect("array") {
                    ids.insert(
                        id.as_i64()
                            .ok_or_else(|| anyhow::anyhow!("invalid assertion citation"))?,
                    );
                }
            } else {
                citations(&value, &mut ids)?;
            }
            for id in ids {
                ensure!(store.evidence(id).await?.is_some(), "unresolved citation");
                citations_checked += 1;
            }
        }
    }
    // A fixed seed gives repeatable samples without depending on server RNG.
    let sampled: Vec<i64> = sqlx::query_scalar("WITH ids AS (SELECT id FROM events UNION ALL SELECT id FROM ocr_evidence), sample AS (SELECT id FROM ids ORDER BY md5(id::text || ':hx-move-v1') LIMIT 1000) SELECT id FROM sample UNION SELECT min(id) FROM ids HAVING min(id) IS NOT NULL UNION SELECT max(id) FROM ids HAVING max(id) IS NOT NULL ORDER BY id")
        .fetch_all(&mut source.conn).await?;
    let mut evidence_checked = 0;
    for id in sampled {
        compare_evidence(source, store, id).await?;
        evidence_checked += 1;
    }
    // Every OCR-derived ID, paged so verification memory is bounded.
    let mut cursor = i64::MIN;
    loop {
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM ocr_evidence WHERE id>$1 ORDER BY id LIMIT 128")
                .bind(cursor)
                .fetch_all(&mut source.conn)
                .await?;
        if ids.is_empty() {
            break;
        }
        for id in ids {
            compare_evidence(source, store, id).await?;
            evidence_checked += 1;
            cursor = id;
        }
    }
    // Probe only whole whitespace-delimited ASCII words from the original
    // search text. Never reparse PG lexemes: URL paths and Unicode case folding
    // differ between these tokenizers. FTS rank=1 integrity covers skipped rows.
    let searches: Vec<(i64,String)> = sqlx::query_as("SELECT event_id,search_text FROM normalized ORDER BY md5(event_id::text || ':hx-move-search-v1') LIMIT 1000")
        .fetch_all(&mut source.conn).await?;
    let mut search_checked = 0;
    for (id, text) in searches {
        let Some(term) = text
            .split_whitespace()
            .find(|word| word.bytes().all(|b| b.is_ascii_alphabetic()))
        else {
            continue;
        };
        let pg: bool = sqlx::query_scalar("SELECT search_vector @@ plainto_tsquery('simple',$2) FROM normalized WHERE event_id=$1").bind(id).bind(term).fetch_one(&mut source.conn).await?;
        let sqlite: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM normalized_fts WHERE rowid=$1 AND normalized_fts MATCH $2)").bind(id).bind(hx_store::sqlite::search_query(term)).fetch_one(&store.pool).await?;
        ensure!(pg && sqlite, "search sample did not resolve for event {id}");
        search_checked += 1;
    }
    Ok((citations_checked, evidence_checked, search_checked))
}
fn sort_blobs(value: &mut Value) {
    if let Some(blobs) = value.get_mut("blobs").and_then(Value::as_array_mut) {
        blobs.sort_by(|a, b| a["sha256"].as_str().cmp(&b["sha256"].as_str()));
    }
}
async fn compare_evidence(source: &mut Snapshot, store: &Store, id: i64) -> Result<()> {
    let event: Option<String> = sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'envelope',e.envelope,'sensitivity',e.sensitivity,'blobs',coalesce((SELECT jsonb_agg(jsonb_build_object('sha256',b.sha256,'evidence_expired',b.evidence_expired)) FROM event_blobs eb JOIN blobs b USING(sha256) WHERE eb.event_id=e.id),'[]'::jsonb))::text FROM events e WHERE id=$1")
        .bind(id).fetch_optional(&mut source.conn).await?;
    let mut old = if let Some(text) = event {
        let mut value: Value = serde_json::from_str(&text)?;
        let derived: Vec<String> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'envelope',envelope,'sensitivity',sensitivity)::text FROM ocr_derived WHERE source_event_id=$1 ORDER BY id")
            .bind(id).fetch_all(&mut source.conn).await?;
        value["derived_evidence"] = json!(
            derived
                .iter()
                .map(|s| serde_json::from_str::<Value>(s))
                .collect::<serde_json::Result<Vec<_>>>()?
        );
        value
    } else {
        let text: String = sqlx::query_scalar("SELECT jsonb_build_object('id',d.id,'envelope',d.envelope,'sensitivity',d.sensitivity,'source_event_id',d.source_event_id,'blobs',jsonb_build_array(jsonb_build_object('sha256',b.sha256,'evidence_expired',b.evidence_expired)))::text FROM ocr_derived d JOIN blobs b USING(sha256) WHERE d.id=$1")
            .bind(id).fetch_one(&mut source.conn).await?;
        serde_json::from_str(&text)?
    };
    let mut new = store
        .evidence(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("sampled evidence missing"))?;
    sort_blobs(&mut old);
    sort_blobs(&mut new);
    ensure!(old == new, "sampled evidence differs");
    Ok(())
}

pub async fn blobs(store: &Store, old_dir: &Path, _all: bool) -> Result<(u64, u64)> {
    let mut stream = sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT sha256,bytes FROM blobs WHERE published ORDER BY sha256",
    )
    .fetch(&store.pool);
    let mut checked = 0;
    let mut problems = Vec::new();
    while let Some((sha, bytes)) = stream.try_next().await? {
        ensure!(hx_store::valid_hash(&sha), "invalid blob identity");
        if let Err(error) = copy_verify_blob(store, old_dir, &sha, bytes) {
            problems.push(format!(
                "{sha}: {}",
                crate::diagnostics::diagnostic(&error, "")
            ));
        }
        checked += 1;
    }
    ensure!(
        problems.is_empty(),
        "published blob verification failed ({} problem(s)): {}",
        problems.len(),
        problems.join("; ")
    );
    std::fs::File::open(store.data_dir.join("blobs"))?.sync_all()?;
    // Every published blob is now rehashed, including without the legacy flag:
    // the owner must get the complete list of missing/size/hash failures at once.
    Ok((checked, checked))
}
fn copy_verify_blob(store: &Store, old_dir: &Path, sha: &str, bytes: Option<i64>) -> Result<()> {
    let old = old_dir.join("blobs").join(&sha[..2]).join(sha);
    let destination = store.blob_path(sha)?;
    let metadata = std::fs::symlink_metadata(&old).context("published blob file unavailable")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "blob must be a regular file"
    );
    ensure!(
        bytes.is_some_and(|b| b >= 0 && b as u64 == metadata.len()),
        "size mismatch (recorded {:?}, found {})",
        bytes,
        metadata.len()
    );
    std::fs::create_dir_all(destination.parent().expect("blob parent"))
        .context("create destination blob directory")?;
    match std::fs::hard_link(&old, &destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            std::fs::copy(&old, &destination).context("copy blob across volumes")?;
        }
        Err(error) => return Err(error).context("link blob"),
    }
    ensure!(
        std::fs::metadata(&destination)?.len() == metadata.len(),
        "copied blob size mismatch"
    );
    let mut file = std::fs::File::open(&destination).context("open blob for hashing")?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).context("read blob for hashing")?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    ensure!(hex::encode(hash.finalize()) == sha, "SHA-256 mismatch");
    std::fs::File::open(&destination)?
        .sync_all()
        .context("sync blob")?;
    std::fs::File::open(destination.parent().expect("blob parent"))?
        .sync_all()
        .context("sync blob directory")?;
    Ok(())
}
