//! Opt-in only. Always creates and drops its own database; never accepts an
//! existing database as fixture data. No services are needed by cargo test.
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use hx_move::{
    Options, move_postgres,
    postgres::{FROZEN, Snapshot},
};
use hx_store::{Store, Ts};
use serde_json::{Value, json};
use sqlx::{Connection, Executor, PgConnection, postgres::PgConnectOptions};
use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

#[tokio::test]
#[ignore = "requires a scratch Postgres service; run in mover-postgres CI"]
async fn synthetic_postgres_cutover_preserves_identity_and_paid_budgets() -> Result<()> {
    let url = std::env::var("HX_MOVE_TEST_POSTGRES")
        .unwrap_or_else(|_| "postgres://hx:hx@localhost:55432/hx".into());
    let base = PgConnectOptions::from_str(&url)?;
    ensure!(
        base.get_database() == Some("hx"),
        "test admin database must be hx"
    );
    let mut admin = PgConnection::connect_with(&base).await?;
    let name = format!(
        "hx_move_{}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await?;
    let scratch_url = format!(
        "{}/{}",
        url.rsplit_once('/')
            .ok_or_else(|| anyhow::anyhow!("test URL"))?
            .0,
        name
    );
    let result = exercise(&scratch_url).await;
    // Cleanup also happens on a failed assertion or a refused move.
    admin
        .execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {name} WITH (FORCE)"
        )))
        .await?;
    admin.close().await?;
    result
}

async fn exercise(url: &str) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let legacy = dir.path().join("legacy");
    let target = dir.path().join("moved");
    let mut pg = PgConnection::connect(url).await?;
    FROZEN.run(&mut pg).await?;
    let sha = hx_store::hash(b"synthetic image bytes");
    let waiting_sha = hx_store::hash(b"late synthetic bytes");
    std::fs::create_dir_all(legacy.join("blobs").join(&sha[..2]))?;
    let blob = legacy.join("blobs").join(&sha[..2]).join(&sha);
    std::fs::write(&blob, b"synthetic image bytes")?;
    for (hash, published, bytes) in [(&sha, true, 21_i64), (&waiting_sha, false, 20_i64)] {
        sqlx::query("INSERT INTO blobs(sha256,bytes,published,pins) VALUES($1,$2,$3,1)")
            .bind(hash)
            .bind(bytes)
            .bind(published)
            .execute(&mut pg)
            .await?;
    }
    let w = hx_store::window_at("2026-01-02T04:00:00Z".parse()?);
    let mut originals = Vec::new();
    for (id, at, kind, hash, sensitivity) in [
        (
            40,
            "2026-01-02T03:59:59.999999Z",
            "synthetic.point",
            None,
            "normal",
        ),
        (
            41,
            "2026-01-02T04:00:00Z",
            "media.still",
            Some(&sha),
            "normal",
        ),
        (42, "2026-01-02T05:00:00Z", "focus.span", None, "normal"),
        (
            43,
            "2026-01-02T07:59:59.999999Z",
            "media.still",
            Some(&sha),
            "restricted",
        ),
        (
            44,
            "2026-01-02T08:00:00Z",
            "synthetic.point",
            None,
            "normal",
        ),
        (
            1001,
            "2026-01-02T06:00:00.123456Z",
            "media.still",
            Some(&waiting_sha),
            "normal",
        ),
    ] {
        let text = format!(
            r#"{{"envelope_version":1,"observation_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","connector_instance":"synthetic:test","source_key":"event-{id}","revision":"0","operation":"observe","kind":"{kind}","schema_version":1,"time":{{"source_start":"{at}","observed_at":"{at}","resolution_ms":1}},"sensitivity":"{sensitivity}","payload":{{"z":1.9783974156629658e-165,"a":-0.0,"mixed":{{"z":1E+24,"a":1e-8}},"small":5e-324,"app_bundle_id":"synthetic.app","app_name":"Synthetic"}},"blobs":[]}}"#
        );
        sqlx::query("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES($1,'01ARZ3NDEKTSV4RRFFQ69G5FAV','synthetic:test',$2,'0',$3,$4::text::timestamptz,$4::text::timestamptz,$5,$6::text::jsonb,$4::text::timestamptz)")
            .bind(id).bind(format!("event-{id}")).bind(kind).bind(at).bind(sensitivity).bind(text).execute(&mut pg).await?;
        let stored: String = sqlx::query_scalar("SELECT envelope::text FROM events WHERE id=$1")
            .bind(id)
            .fetch_one(&mut pg)
            .await?;
        originals.push((id, stored));
        if let Some(hash) = hash {
            sqlx::query("INSERT INTO event_blobs VALUES($1,$2)")
                .bind(id)
                .bind(hash)
                .execute(&mut pg)
                .await?;
        }
        if id == 44 {
            sqlx::query(
                "INSERT INTO quarantine(event_id,reason) VALUES($1,'synthetic classification')",
            )
            .bind(id)
            .execute(&mut pg)
            .await?;
        } else {
            sqlx::query("INSERT INTO normalized(event_id,normalizer_version,payload,search_text) VALUES($1,'normalize/v1','{\"z\":1e24,\"a\":-0.0}','syntheticneedle')").bind(id).execute(&mut pg).await?;
        }
    }
    sqlx::query("INSERT INTO ocr_jobs(sha256,engine_version,source_event_id,status,result) VALUES($1,'synthetic-ocr/v1',41,'complete','{\"lines\":[{\"text\":\"syntheticneedle\",\"confidence\":0.9,\"bbox\":{\"x\":0,\"y\":0,\"width\":1,\"height\":1}}],\"engine_name\":\"synthetic\",\"engine_version\":\"v1\",\"coverage\":\"partial\",\"n\":1.9783974156629658e-165}')").bind(&sha).execute(&mut pg).await?;
    for (id, source) in [(91, 41), (92, 43)] {
        sqlx::query("INSERT INTO ocr_evidence(id,observation_id,source_event_id,sha256,engine_version) VALUES($1,$2,$3,$4,'synthetic-ocr/v1')").bind(id).bind(format!("01ARZ3NDEKTSV4RRFFQ69G5F{:02}",id)).bind(source).bind(&sha).execute(&mut pg).await?;
    }
    // Compute legacy paid identity from the exact JSON text the Postgres reader
    // would hand serde, rather than from the pre-storage exponent notation.
    let observations = originals
        .iter()
        .filter(|(id, _)| [41, 42, 43, 1001].contains(id))
        .map(|(id, text)| {
            let envelope: hx_schema::Envelope = serde_json::from_str(text)?;
            Ok(hx_compile::Observation {
                id: *id,
                received_at: envelope.time.observed_at,
                envelope,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let timeline = hx_compile::compile(&observations, w);
    let lineage = [
        json!({"id":91,"source_event_id":41,"sha256":sha,"engine_version":"synthetic-ocr/v1"}),
        json!({"id":92,"source_event_id":43,"sha256":sha,"engine_version":"synthetic-ocr/v1"}),
    ];
    let timeline_hash = hx_store::hash(&serde_json::to_vec(&(&timeline, &lineage))?);
    // A migration-10 window already has its paid-budget metadata. Reapplying
    // the old repair here would destroy these deliberately retained counters.
    sqlx::query("INSERT INTO windows(start_at,end_at,generation,dirty,compiled_generation,timeline,timeline_hash,revision24_finished) VALUES($1::text::timestamptz,$2::text::timestamptz,7,false,7,$3::text::jsonb,$4,true)")
        .bind(w.start.to_rfc3339()).bind(w.end.to_rfc3339()).bind(serde_json::to_string(&timeline)?).bind(&timeline_hash).execute(&mut pg).await?;
    let content_text = format!(
        r#"{{"z":1e+24,"a":-0.0,"episodes":[{{"evidence":[41,91],"claims":[{{"outcome_evidence":[92]}}]}}],"input_manifest":{{"timeline_hash":"{timeline_hash}","input_generation":7,"prior_revision_ids":[201],"prompt_version":"synthetic","provider":"synthetic","sensitive_egress":"redact","disclosure_policy":{{"digest":"synthetic"}},"n":1.9783974156629658e-165}}}}"#
    );
    let content: String = sqlx::query_scalar("SELECT $1::text::jsonb::text")
        .bind(content_text)
        .fetch_one(&mut pg)
        .await?;
    let manifest = serde_json::from_str::<Value>(&content)?["input_manifest"].clone();
    let output_key = hx_store::hash(&serde_json::to_vec(&manifest)?);
    for (id, interpretation, run, key) in [
        (
            201,
            "{\"evidence\":[41]}".to_owned(),
            None,
            "legacy-key".to_owned(),
        ),
        (
            202,
            content.clone(),
            Some(
                "{\"calls\":[{\"tool\":\"prior_context\",\"result\":{\"revisions\":[{\"revision_id\":201}]}}]}",
            ),
            output_key.clone(),
        ),
        (
            203,
            "{\"input_manifest\":{\"prior_revision_ids\":[]}}".to_owned(),
            None,
            "empty-dependencies".to_owned(),
        ),
    ] {
        sqlx::query("INSERT INTO revisions(id,start_at,input_generation,interpretation,output_key,input_namespaces,agent_run) VALUES($1,$2::text::timestamptz,7,$3::text::jsonb,$4,ARRAY['synthetic:'],$5::text::jsonb)").bind(id).bind(w.start.to_rfc3339()).bind(interpretation).bind(key).bind(run).execute(&mut pg).await?;
    }
    sqlx::query("UPDATE windows SET canonical_revision=202")
        .execute(&mut pg)
        .await?;
    sqlx::query("INSERT INTO watermarks VALUES('synthetic:test','2026-01-02T08:00:00Z',18446744073709551615)").execute(&mut pg).await?;
    sqlx::query("INSERT INTO device_tokens VALUES('synthetic-hash','synthetic-token',ARRAY['synthetic:'],ARRAY['ingest'],false)").execute(&mut pg).await?;
    sqlx::query("INSERT INTO entity_assertions(id,entity,assertion,evidence,sensitivity) VALUES(301,'synthetic-entity','{\"z\":1e-8,\"a\":-0.0}',ARRAY[41,91]::bigint[],'restricted')").execute(&mut pg).await?;
    for (id, slot, state, revivals) in [
        (401, "interpret", "done", 1),
        (402, "revision24", "failed", 3),
        (403, "compile", "done", 0),
    ] {
        let key = format!("{slot}:{}", w.start);
        // Deliberately use the old +00:00 JSON format, not serde's Z.
        let payload = format!(
            r#"{{"window":{{"start":"2026-01-02T04:00:00+00:00","end":"2026-01-02T08:00:00+00:00"}},"generation":7,"revision24":{}}}"#,
            slot == "revision24"
        );
        sqlx::query("INSERT INTO jobs(id,output_key,kind,payload,state,revivals,attempts,request_version,leased_version) VALUES($1,$2,$3,$4::text::jsonb,$5,$6,5,9,9)").bind(id).bind(key).bind(if slot=="compile" {"compile"} else {"interpret"}).bind(payload).bind(state).bind(revivals).execute(&mut pg).await?;
    }
    sqlx::query("INSERT INTO daily_sweeps VALUES('2026-01-03','2026-01-02T04:00:00Z',7,true)")
        .execute(&mut pg)
        .await?;
    for (name, seq) in [
        ("events", 5000_i64),
        ("revisions", 8000),
        ("jobs", 9000),
        ("entity_assertions", 10000),
    ] {
        sqlx::query("SELECT setval($1::regclass,$2)")
            .bind(format!("{name}_id_seq"))
            .bind(seq)
            .execute(&mut pg)
            .await?;
    }
    let mut source = Snapshot::open(url).await?;
    let before = source.hashes().await?;
    source.close().await?;
    let options = Options {
        postgres_url: url.to_owned(),
        source_data_dir: legacy.clone(),
        state_dir: target.clone(),
        verify_all_blobs: true,
    };
    // Fail after rows have loaded: the staged blob has the right size but the
    // wrong bytes. A retry must succeed without deleting anything by hand.
    std::fs::write(&blob, b"synthetic image byteX")?;
    ensure!(move_postgres(&options).await.is_err(), "bad blob must fail");
    ensure!(!target.exists(), "failed move left target artifacts");
    std::fs::write(&blob, b"synthetic image bytes")?;
    std::fs::create_dir(&target)?;
    // Exercise the owner's actual CLI, including successful exit and a
    // content-free JSON report, at the highest interface available in CI.
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_hx-move"))
        .env("HX_MOVE_POSTGRES_URL", url)
        .arg("--source-data-dir")
        .arg(&legacy)
        .arg("--state-dir")
        .arg(&target)
        .arg("--verify-all-blobs")
        .output()
        .await?;
    ensure!(output.status.success(), "synthetic mover CLI failed");
    ensure!(output.stderr.is_empty(), "unexpected CLI stderr");
    ensure!(
        !String::from_utf8_lossy(&output.stdout).contains("syntheticneedle"),
        "CLI printed content"
    );
    let report: hx_move::Report = serde_json::from_slice(&output.stdout)?;
    ensure!(
        serde_json::from_slice::<Value>(&output.stdout)? == serde_json::to_value(&report)?,
        "report contains unexpected fields"
    );
    ensure!(
        report.blobs_checked == 1 && report.blobs_rehashed == 1 && report.citations_checked >= 5
    );
    ensure!(
        move_postgres(&options).await.is_err(),
        "must refuse an existing core"
    );
    let store = Store::open(&target).await?;
    hx_move::core::verify(&store, &report.tables).await?;
    for (id, original) in &originals {
        let new: String = sqlx::query_scalar("SELECT envelope FROM events WHERE id=$1")
            .bind(id)
            .fetch_one(&store.pool)
            .await?;
        ensure!(&new == original, "legacy JSON text was re-encoded");
        let converted: Ts = sqlx::query_scalar("SELECT source_start FROM events WHERE id=$1")
            .bind(id)
            .fetch_one(&store.pool)
            .await?;
        let value: hx_schema::Envelope = serde_json::from_str(original)?;
        ensure!(
            hx_store::window_at(DateTime::<Utc>::from(converted))
                == hx_store::window_at(value.time.source_start.unwrap()),
            "timestamp crossed a window boundary"
        );
    }
    let moved: (String, String) =
        sqlx::query_as("SELECT interpretation,output_key FROM revisions WHERE id=202")
            .fetch_one(&store.pool)
            .await?;
    ensure!(
        moved == (content, output_key),
        "manifest or output key changed"
    );
    let moved_manifest = serde_json::from_str::<Value>(&moved.0)?["input_manifest"].clone();
    ensure!(
        moved_manifest == manifest
            && hx_store::hash(&serde_json::to_vec(&moved_manifest)?) == moved.1,
        "manifest identity changed"
    );
    let dependencies: Vec<(i64, i64)> =
        sqlx::query_as("SELECT revision_id,prior_revision_id FROM revision_dependencies")
            .fetch_all(&store.pool)
            .await?;
    ensure!(dependencies == [(202, 201)]);
    let flags: Vec<(i64, bool)> =
        sqlx::query_as("SELECT id,has_dependency_record FROM revisions ORDER BY id")
            .fetch_all(&store.pool)
            .await?;
    ensure!(flags == [(201, false), (202, true), (203, true)]);
    let seq: i64 = sqlx::query_scalar("SELECT next_id FROM evidence_seq")
        .fetch_one(&store.pool)
        .await?;
    ensure!(seq == 5001);
    let sequences: Vec<(String, i64)> =
        sqlx::query_as("SELECT name,seq FROM sqlite_sequence ORDER BY name")
            .fetch_all(&store.pool)
            .await?;
    ensure!(
        sequences
            == [
                ("entity_assertions".into(), 10000),
                ("jobs".into(), 9000),
                ("revisions".into(), 8000)
            ]
    );
    let water: String = sqlx::query_scalar("SELECT sequence FROM watermarks")
        .fetch_one(&store.pool)
        .await?;
    ensure!(water == "18446744073709551615");
    let jobs: Vec<(String, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT state,attempts,revivals,window_start,generation FROM jobs ORDER BY id",
    )
    .fetch_all(&store.pool)
    .await?;
    ensure!(
        jobs == [
            ("done".into(), 5, 1, Ts::from(w.start).0, 7),
            ("failed".into(), 5, 3, Ts::from(w.start).0, 7),
            ("done".into(), 5, 0, Ts::from(w.start).0, 7)
        ]
    );
    // Recompute via the runtime interface to catch float/JSON drift that table
    // hashes alone cannot detect. No provider or paid process is invoked.
    ensure!(store.save_timeline(&timeline, 7).await?);
    ensure!(
        store.compiled_timeline_hash(w, 7).await? == Some(timeline_hash),
        "timeline hash changed after runtime read"
    );
    let prepared = hx_server::processing::prepare_input_with_policy(
        &store,
        w,
        "synthetic",
        hx_server::processing::SensitiveEgress::Redact,
        &hx_server::disclosure::DisclosurePolicy::default(),
    )
    .await?;
    ensure!(
        prepared.packet.evidence.iter().any(|e| e.id == 91),
        "visible OCR citation missing"
    );
    ensure!(
        !prepared
            .packet
            .evidence
            .iter()
            .any(|e| [43, 92].contains(&e.id)),
        "restricted evidence was disclosed"
    );
    let search = store
        .search("syntheticneedle", false, &["synthetic:".into()])
        .await?;
    ensure!(
        search.iter().all(|row| row["id"] != 43),
        "restricted search hit was disclosed"
    );
    ensure!(prepared.manifest["timeline_hash"] == manifest["timeline_hash"]);
    let paid_before: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT id,state,request_version,revivals FROM jobs WHERE kind='interpret' ORDER BY id",
    )
    .fetch_all(&store.pool)
    .await?;
    hx_server::processing::schedule(&store).await?;
    let paid_after: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT id,state,request_version,revivals FROM jobs WHERE kind='interpret' ORDER BY id",
    )
    .fetch_all(&store.pool)
    .await?;
    ensure!(
        paid_before == paid_after,
        "schedule enqueued or revived paid work"
    );
    store.discover_ocr("synthetic-ocr/v1").await?;
    let evidence: i64 = sqlx::query_scalar("SELECT count(*) FROM ocr_evidence")
        .fetch_one(&store.pool)
        .await?;
    let ocr_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM ocr_jobs")
        .fetch_one(&store.pool)
        .await?;
    ensure!(evidence == 2 && ocr_jobs == 1, "moved stills were re-OCRed");
    ensure!(
        store
            .upload_blob(&waiting_sha, b"late synthetic bytes")
            .await?
    );
    store.discover_ocr("synthetic-ocr/v1").await?;
    let waiting: i64 = sqlx::query_scalar("SELECT source_event_id FROM ocr_jobs WHERE sha256=$1")
        .bind(&waiting_sha)
        .fetch_one(&store.pool)
        .await?;
    ensure!(waiting == 1001, "unpublished moved still was missed");
    // New IDs continue beyond source sequence state, even gaps above row max.
    let mut tx = store.begin_write().await?;
    let new_id = tx.insert_evidence(sqlx::query_scalar("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope,received_at) VALUES((SELECT next_id FROM evidence_seq),'synthetic-next','synthetic:test','next','0','synthetic.point',0,0,'normal','{}',0) RETURNING id")).await?;
    ensure!(new_id == Some(5001));
    tx.rollback().await?;
    ensure!(
        sqlx::query("UPDATE events SET source_key='changed' WHERE id=41")
            .execute(&store.pool)
            .await
            .is_err(),
        "immutability weakened"
    );
    let mut source = Snapshot::open(url).await?;
    ensure!(
        source.hashes().await? == before,
        "mover changed Postgres rows"
    );
    source.close().await?;
    store.pool.close().await;
    // A live lease must fail before any target files are created.
    sqlx::query("UPDATE jobs SET state='running',lease_until=now()+interval '1 hour' WHERE id=401")
        .execute(&mut pg)
        .await?;
    let lease_options = Options {
        state_dir: dir.path().join("live-lease"),
        ..options
    };
    ensure!(move_postgres(&lease_options).await.is_err() && !lease_options.state_dir.exists());
    sqlx::query("UPDATE jobs SET state='done',lease_until=NULL WHERE id=401")
        .execute(&mut pg)
        .await?;
    sqlx::query("INSERT INTO events(id,observation_id,connector_instance,source_key,revision,kind,source_start,observed_at,sensitivity,envelope) SELECT 91,observation_id,connector_instance,'collision','0',kind,source_start,observed_at,sensitivity,envelope FROM events WHERE id=41").execute(&mut pg).await?;
    ensure!(
        move_postgres(&lease_options).await.is_err() && !lease_options.state_dir.exists(),
        "overlapping IDs were accepted"
    );
    pg.close().await?;
    Ok(())
}
