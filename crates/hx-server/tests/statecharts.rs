use chrono::{TimeZone, Utc};
use hx_schema::{
    ConnectorInstance, Envelope, Interval, Kind, ObservationId, ObservationTime, Operation,
    Revision, Sensitivity, SourceKey,
};
use hx_store::test_support::Fixture;
use hx_store::{Store, Token, UploadBatch};
use serde_json::json;
fn window() -> Interval {
    hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 8, 0, 0).unwrap())
}
#[tokio::test]
async fn exhausted_compile_is_revived_after_cooldown() {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    f.store.request_work("compile", w, g).await.unwrap();
    for _ in 0..5 {
        let j = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
        assert_eq!(j.kind, "compile");
        assert!(
            hx_jobs::finish(&f.store.pool, &j, Some("synthetic transient failure"))
                .await
                .unwrap()
        );
        f.store.make_jobs_due("ready", None).await.unwrap();
    }
    hx_server::processing::schedule(&f.store).await.unwrap();
    let state = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.state == "failed")
        .unwrap()
        .state;
    assert_eq!(state, "failed");
    f.store.make_jobs_due("failed", None).await.unwrap();
    for _ in 0..3 {
        hx_server::processing::schedule(&f.store).await.unwrap();
    }
    let r = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.output_key == format!("compile:{}", w.start))
        .unwrap();
    assert_eq!(r.state, "ready");
    assert_eq!(r.attempts, 0);
    let dirty = f.store.window_state(w).await.unwrap().dirty;
    assert!(dirty);
    f.close().await;
}
struct Fails;
impl hx_interpret::Provider for Fails {
    fn name(&self) -> String {
        "synthetic failure".into()
    }
    async fn complete(&self, _: &str, _: &str) -> Result<String, hx_interpret::InterpretError> {
        Err(hx_interpret::InterpretError::Provider(
            "synthetic temporary outage".into(),
        ))
    }
}
#[tokio::test]
async fn exhausted_interpretations_recover_without_new_input() {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    f.store.request_work("compile", w, g).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Fails,
    };
    assert!(hx_jobs::run_one(&f.store.pool, &runner).await.unwrap());
    f.store.settle_inputs().await.unwrap();
    for _ in 0..10 {
        f.store
            .make_jobs_due("ready", Some("interpret"))
            .await
            .unwrap();
        assert!(hx_jobs::run_one(&f.store.pool, &runner).await.is_err());
    }
    hx_server::processing::schedule(&f.store).await.unwrap();
    let state = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.state == "failed")
        .unwrap()
        .state;
    assert_eq!(state, "failed");
    f.store.make_jobs_due("failed", None).await.unwrap();
    for _ in 0..3 {
        hx_server::processing::schedule(&f.store).await.unwrap();
    }
    let failed = f
        .store
        .job_states()
        .await
        .unwrap()
        .iter()
        .filter(|j| j.kind == "interpret" && j.state == "failed" && j.attempts == 5)
        .count();
    assert_eq!(failed, 0);
    let compile = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.output_key == format!("compile:{}", w.start))
        .unwrap()
        .state;
    assert_eq!(compile, "done");
    let r = f.store.window_state(w).await.unwrap();
    assert!(!r.dirty);
    assert!(r.canonical_revision.is_none());
    let recovered = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    let mut canonical: Option<i64> = None;
    // The scheduler also creates the previous bucket; allow its compile to run.
    for _ in 0..4 {
        assert!(hx_jobs::run_one(&f.store.pool, &recovered).await.unwrap());
        canonical = f.store.window_state(w).await.unwrap().canonical_revision;
        if canonical.is_some() {
            break;
        }
    }
    assert!(canonical.is_some());
    f.close().await;
}
#[tokio::test]
async fn workers_serialize_jobs_for_the_same_window() {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    f.store.request_work("compile", w, g).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    assert!(hx_jobs::run_one(&f.store.pool, &runner).await.unwrap());
    // Make both cadences eligible; this test isolates per-window claim fencing.
    f.store
        .make_jobs_due("ready", Some("interpret"))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        hx_jobs::claim(&f.store.pool, 120),
        hx_jobs::claim(&f.store.pool, 120)
    );
    let claimed: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(claimed.len(), 1);
    let first = &claimed[0];
    assert_eq!(first.kind, "interpret");
    assert!(hx_jobs::claim(&f.store.pool, 120).await.unwrap().is_none());
    // A different window can still run while this window's lease is live.
    let other = hx_store::window_at(w.end);
    let generation = f.store.generation(other).await.unwrap();
    f.store
        .request_work("compile", other, generation)
        .await
        .unwrap();
    let parallel = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert_eq!(parallel.payload["window"], json!(other));
    assert!(
        hx_jobs::finish(&f.store.pool, &parallel, None)
            .await
            .unwrap()
    );
    assert!(hx_jobs::finish(&f.store.pool, first, None).await.unwrap());
    let next = hx_jobs::claim(&f.store.pool, 120).await.unwrap();
    let debug = f.store.job_states().await.unwrap();
    assert!(next.is_some(), "{debug:?}");
    f.close().await;
}
#[tokio::test]
async fn windows_controls_stale_defer_and_restart_fencing() {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    let (_, ok) = f
        .store
        .publish_revision_key(w, g, json!({"tasks":[]}), "first")
        .await
        .unwrap();
    assert!(ok);
    f.store.change_input(w).await.unwrap();
    let (new, ok) = f
        .store
        .publish_revision_key(w, g + 1, json!({"tasks":[]}), "new")
        .await
        .unwrap();
    assert!(ok);
    let (_, ok) = f
        .store
        .publish_revision_key(w, g, json!({"tasks":[]}), "stale")
        .await
        .unwrap();
    assert!(!ok);
    let canonical: Option<i64> = f.store.window_state(w).await.unwrap().canonical_revision;
    assert_eq!(canonical, Some(new));
    f.store.request_work("compile", w, g + 1).await.unwrap();
    let first = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert!(
        hx_jobs::defer(
            &f.store.pool,
            &first,
            &hx_jobs::Deferred::NormalizationPending
        )
        .await
        .unwrap()
    );
    let r = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id == first.id)
        .unwrap();
    assert_eq!(r.state, "ready");
    assert_eq!(r.attempts, 0);
    f.store.make_jobs_due("ready", None).await.unwrap();
    let old = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    f.store.request_work("compile", w, g + 2).await.unwrap();
    assert!(hx_jobs::heartbeat(&f.store.pool, &old, 120).await.unwrap());
    f.store.expire_lease(old.id, Some(1)).await.unwrap();
    let fresh = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert_eq!(fresh.id, old.id);
    assert_ne!(fresh.owner, old.owner);
    assert_eq!(fresh.attempts, 1);
    assert!(!hx_jobs::finish(&f.store.pool, &old, None).await.unwrap());
    assert!(hx_jobs::finish(&f.store.pool, &fresh, None).await.unwrap());
    f.close().await;
}

fn token() -> Token {
    Token {
        prefixes: vec!["test:".into()],
        scopes: vec!["ingest".into(), "read_raw".into(), "read_l3".into()],
    }
}
fn event(key: &str, hour: u32) -> Envelope {
    let at = Utc.with_ymd_and_hms(2026, 1, 1, hour, 0, 0).unwrap();
    Envelope {
        envelope_version: 1,
        observation_id: ObservationId::generate(),
        connector_instance: ConnectorInstance::new("test:device"),
        source_key: SourceKey::new(key),
        revision: Revision::append_only(),
        operation: Operation::Observe,
        sequence: None,
        kind: Kind::new("web.visit"),
        schema_version: 1,
        time: ObservationTime::point(at),
        device: None,
        actor: None,
        subject: None,
        resource: None,
        links: vec![],
        coverage: None,
        sensitivity: Sensitivity::Normal,
        payload: json!({"url":"https://example.invalid","title":"synthetic nebula"}),
        blobs: vec![],
    }
}
async fn ingest(store: &Store, envelopes: Vec<Envelope>) -> hx_store::UploadAck {
    store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes,
            },
            &token(),
        )
        .await
        .unwrap()
}
async fn count(store: &Store, table: &str) -> i64 {
    store.record_count(table).await.unwrap()
}

#[tokio::test]
async fn shortened_revision_invalidates_tail_and_suppresses_old_winner() {
    let f = Fixture::new().await;
    let mut old = event("span", 0);
    old.operation = Operation::Upsert;
    old.sequence = Some(1);
    old.time.source_end = Some(Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap());
    old.revision = Revision::new("old");
    ingest(&f.store, vec![old.clone()]).await;
    f.store
        .normalize_window(hx_store::window_at(old.time.observed_at))
        .await
        .unwrap();
    let tail = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 4, 0, 0).unwrap());
    let (before, g) = hx_server::processing::compile_window(&f.store, tail)
        .await
        .unwrap();
    assert!(!before.lanes.is_empty());
    let mut new = old.clone();
    new.observation_id = ObservationId::generate();
    new.sequence = Some(2);
    new.revision = Revision::new("new");
    new.time.source_end = Some(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    ingest(&f.store, vec![new.clone()]).await;
    let pending = hx_server::processing::compile_window(&f.store, tail)
        .await
        .unwrap_err();
    assert!(matches!(
        pending.downcast_ref::<hx_jobs::Deferred>(),
        Some(hx_jobs::Deferred::NormalizationPending)
    ));
    f.store
        .normalize_window(hx_store::window_at(new.time.observed_at))
        .await
        .unwrap();
    let row = f.store.window_state(tail).await.unwrap();
    assert_eq!(row.generation, g + 1, "tail generation must advance");
    assert!(row.dirty, "tail must be dirty");
    let obs = f.store.observations(tail, true).await.unwrap();
    assert_eq!(obs.len(), 2);
    let actual = hx_compile::compile(&obs, tail);
    assert!(actual.lanes.is_empty());
    let (recompiled, generation) = hx_server::processing::compile_window(&f.store, tail)
        .await
        .unwrap();
    assert_eq!(generation, g + 1);
    assert!(recompiled.lanes.is_empty());
    let mut all = obs.clone();
    all.push(hx_compile::Observation {
        id: 9999,
        received_at: Utc::now(),
        envelope: new,
    });
    let expected = hx_compile::compile(&all, tail);
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap(),
        "revision resolution must precede overlap filtering"
    );
    f.close().await;
}
#[tokio::test]
async fn five_crashes_quarantine_pending_normalization() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("pending", 0)]).await;
    for _ in 0..5 {
        let job = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
        assert_eq!(job.kind, "normalize");
        // Crash after claim: no handler writes, lease expires. Synthetic time advance.
        f.store.expire_lease(job.id, None).await.unwrap();
    }
    let _ = hx_jobs::claim(&f.store.pool, 120).await.unwrap();
    hx_server::processing::schedule(&f.store).await.unwrap();
    let state = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.kind == "normalize")
        .unwrap()
        .state;
    assert_eq!(state, "failed");
    assert_eq!(count(&f.store, "normalized").await, 0);
    assert_eq!(count(&f.store, "quarantine").await, 1);
    let reason = f.store.quarantine_reasons().await.unwrap().remove(0);
    assert!(reason.contains("exhaust"));
    ingest(&f.store, vec![event("pending", 0)]).await; // same dedup key cannot revive work
    let state = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.kind == "normalize")
        .unwrap()
        .state;
    assert_eq!(state, "failed");
    f.close().await;
}
#[tokio::test]
async fn reverse_arrival_same_window_preserves_newer_sequence() {
    let f = Fixture::new().await;
    let mut newer = event("reverse", 0);
    newer.sequence = Some(2);
    newer.operation = Operation::Upsert;
    newer.revision = Revision::new("newer");
    newer.time.source_end = Some(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    let ack = ingest(&f.store, vec![newer.clone()]).await;
    assert_eq!(ack.accepted, 1);
    let mut older = newer.clone();
    older.observation_id = ObservationId::generate();
    older.sequence = Some(1);
    older.revision = Revision::new("older");
    let ack = ingest(&f.store, vec![older]).await;
    assert_eq!(ack.accepted, 1);
    let w = hx_store::window_at(newer.time.observed_at);
    f.store.normalize_window(w).await.unwrap();
    let obs = f.store.observations(w, true).await.unwrap();
    assert_eq!(obs.len(), 2);
    let newer_id = obs
        .iter()
        .find(|o| o.envelope.sequence == Some(2))
        .unwrap()
        .id;
    let t = hx_compile::compile(&obs, w);
    assert!(!t.lanes.is_empty());
    assert!(
        t.lanes
            .iter()
            .flat_map(|l| &l.intervals)
            .all(|i| i.evidence == vec![newer_id])
    );
    f.close().await;
}

#[tokio::test]
async fn latest_revision_is_resolved_before_window_overlap() {
    let f = Fixture::new().await;
    let mut old = event("latest", 0);
    old.operation = Operation::Upsert;
    old.sequence = Some(1);
    old.revision = Revision::new("long");
    old.time.source_end = Some(Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap());
    let mut new = old.clone();
    new.observation_id = ObservationId::generate();
    new.sequence = Some(2);
    new.revision = Revision::new("short");
    new.time.source_end = Some(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    ingest(&f.store, vec![old, new]).await;
    f.store
        .normalize_window(hx_store::window_at(event("latest", 0).time.observed_at))
        .await
        .unwrap();
    let tail = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 4, 0, 0).unwrap());
    let (timeline, _) = hx_server::processing::compile_window(&f.store, tail)
        .await
        .unwrap();
    assert!(
        timeline.lanes.is_empty(),
        "superseded long span must not become current"
    );
    assert_eq!(count(&f.store, "events").await, 2, "L0 history is retained");
    f.close().await;
}

#[tokio::test]
async fn normalization_errors_quarantine_only_pending_events() {
    let f = Fixture::new().await;
    let normalized = event("classified", 0);
    ingest(&f.store, vec![normalized.clone()]).await;
    f.store
        .normalize_window(hx_store::window_at(normalized.time.observed_at))
        .await
        .unwrap();
    ingest(&f.store, vec![event("crashing", 0)]).await;
    for _ in 0..5 {
        let job = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
        assert_eq!(job.kind, "normalize");
        assert!(
            hx_jobs::finish(&f.store.pool, &job, Some("synthetic normalization failure"))
                .await
                .unwrap()
        );
        f.store.make_jobs_due("ready", None).await.unwrap();
    }
    assert_eq!(count(&f.store, "normalized").await, 1);
    assert_eq!(count(&f.store, "quarantine").await, 1);
    assert_eq!(
        f.store
            .pending_normalization(hx_store::window_at(normalized.time.observed_at))
            .await
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
async fn claim_observes_jobs_that_become_due_while_waiting_for_the_gate() {
    let f = Fixture::new().await;
    let gate = f.store.hold_claim_gate().await.unwrap();
    let pool = f.store.pool.clone();
    let worker = tokio::spawn(async move { hx_jobs::claim(&pool, 120).await });
    let mut waiting = false;
    for _ in 0..100 {
        waiting = f.store.claim_is_waiting().await.unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        waiting,
        "claim must have begun its transaction before the job becomes due"
    );
    f.store.enqueue_due_probe().await.unwrap();
    gate.release().await.unwrap();
    assert!(worker.await.unwrap().unwrap().is_some());
    f.close().await;
}

#[tokio::test]
async fn batch_watermarks_keep_independent_maxima_and_ignore_duplicates() {
    let f = Fixture::new().await;
    let mut first = event("first", 0);
    first.sequence = Some(3);
    let mut second = event("second", 4);
    second.sequence = Some(1);
    let mut third = event("third", 8);
    third.sequence = Some(2);
    let at = third.time.observed_at;
    assert_eq!(
        ingest(&f.store, vec![first.clone(), second, third])
            .await
            .accepted,
        3
    );
    first.time.observed_at = at + chrono::Duration::hours(4);
    first.sequence = Some(99);
    assert_eq!(ingest(&f.store, vec![first]).await.duplicates, 1);
    let (through, sequence) = f.store.watermark("test:device").await.unwrap();
    assert_eq!(through, at);
    assert_eq!(sequence, "3");
    f.close().await;
}
