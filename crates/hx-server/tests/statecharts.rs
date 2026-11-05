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
async fn exhaust_slot(store: &Store, key: &str) {
    let mut failures = 0;
    while failures < 5 {
        store.make_jobs_due("ready", None).await.unwrap();
        let j = hx_jobs::claim(store, 120).await.unwrap().unwrap();
        let target = store
            .job_states()
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == j.id && s.output_key == key);
        hx_jobs::finish(store, &j, target.then_some("synthetic failure"))
            .await
            .unwrap();
        if target {
            failures += 1;
        }
    }
}
async fn assert_revival_cap(kind: &str) {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    if kind == "interpret" {
        f.store.request_interpretation(w, g, false).await.unwrap();
    } else {
        f.store.request_work(kind, w, g).await.unwrap();
    }
    let key = format!("{kind}:{}", w.start);
    for revival in 0..=3 {
        exhaust_slot(&f.store, &key).await;
        f.store.make_jobs_due("failed", None).await.unwrap();
        for _ in 0..3 {
            hx_server::processing::schedule(&f.store).await.unwrap();
        }
        let j = f
            .store
            .job_states()
            .await
            .unwrap()
            .into_iter()
            .find(|j| j.output_key == key)
            .unwrap();
        assert_eq!(
            j.state,
            if revival < 3 { "ready" } else { "failed" },
            "revival {revival}"
        );
    }
    // Only free compile work gets a fresh budget when the data version changes.
    f.store.change_input(w).await.unwrap();
    if kind == "interpret" {
        f.store
            .request_interpretation(w, g + 1, false)
            .await
            .unwrap();
    } else {
        f.store.request_work(kind, w, g + 1).await.unwrap();
    }
    let reset = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.output_key == key)
        .unwrap();
    if kind == "interpret" {
        assert_eq!(
            (reset.state.as_str(), reset.attempts, reset.revivals),
            ("failed", 5, 3),
            "automatic input refreshes preserve the paid budget"
        );
        f.close().await;
        return;
    }
    assert_eq!(reset.revivals, 0, "free compile work resets revivals");
    for _ in 0..8 {
        f.store.make_jobs_due("ready", None).await.unwrap();
        let j = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
        if j.kind != kind {
            hx_jobs::finish(&f.store, &j, None).await.unwrap();
            continue;
        }
        hx_jobs::defer(&f.store, &j, &hx_jobs::Deferred::ProviderBusy)
            .await
            .unwrap();
        let state = f
            .store
            .job_states()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == j.id)
            .unwrap();
        assert_eq!(state.attempts, 0);
    }
    exhaust_slot(&f.store, &key).await;
    f.store.make_jobs_due("failed", None).await.unwrap();
    hx_server::processing::schedule(&f.store).await.unwrap();
    assert_eq!(
        f.store
            .job_states()
            .await
            .unwrap()
            .into_iter()
            .find(|j| j.output_key == key)
            .unwrap()
            .state,
        "ready"
    );
    f.close().await;
}
#[tokio::test]
async fn exhausted_compile_stops_after_three_revivals_per_generation() {
    assert_revival_cap("compile").await;
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
async fn exhausted_interpretations_do_not_revive_on_generation_changes() {
    assert_revival_cap("interpret").await;
}
#[tokio::test]
async fn workers_serialize_jobs_for_the_same_window() {
    let f = Fixture::new().await;
    let w = window();
    ingest(&f.store, vec![event("synthetic-input", 8)]).await;
    f.store.normalize_window(w).await.unwrap();
    // Complete the ingest-created normalization slot before testing other jobs.
    let n = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    hx_jobs::finish(&f.store, &n, None).await.unwrap();
    let g = f.store.generation(w).await.unwrap();
    f.store.request_work("compile", w, g).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    // Explicit synthetic slots isolate claim fencing from automatic age admission.
    for revision24 in [false, true] {
        f.store
            .request_interpretation(w, g, revision24)
            .await
            .unwrap();
    }
    // Make both cadences eligible; this test isolates per-window claim fencing.
    f.store
        .make_jobs_due("ready", Some("interpret"))
        .await
        .unwrap();
    let (a, b) = tokio::join!(hx_jobs::claim(&f.store, 120), hx_jobs::claim(&f.store, 120));
    let claimed: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(claimed.len(), 1);
    let first = &claimed[0];
    assert_eq!(first.kind, "interpret");
    assert!(hx_jobs::claim(&f.store, 120).await.unwrap().is_none());
    // A different window can still run while this window's lease is live.
    let other = hx_store::window_at(w.end);
    let generation = f.store.generation(other).await.unwrap();
    f.store
        .request_work("compile", other, generation)
        .await
        .unwrap();
    let parallel = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(parallel.payload["window"], json!(other));
    assert!(hx_jobs::finish(&f.store, &parallel, None).await.unwrap());
    assert!(hx_jobs::finish(&f.store, first, None).await.unwrap());
    let next = hx_jobs::claim(&f.store, 120).await.unwrap();
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
    let first = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert!(
        hx_jobs::defer(&f.store, &first, &hx_jobs::Deferred::NormalizationPending)
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
    let old = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    f.store.request_work("compile", w, g + 2).await.unwrap();
    assert!(hx_jobs::heartbeat(&f.store, &old, 120).await.unwrap());
    f.store.expire_lease(old.id, Some(1)).await.unwrap();
    let fresh = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(fresh.id, old.id);
    assert_ne!(fresh.owner, old.owner);
    assert_eq!(fresh.attempts, 1);
    assert!(!hx_jobs::finish(&f.store, &old, None).await.unwrap());
    assert!(hx_jobs::finish(&f.store, &fresh, None).await.unwrap());
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
        let job = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
        assert_eq!(job.kind, "normalize");
        // Crash after claim: no handler writes, lease expires. Synthetic time advance.
        f.store.expire_lease(job.id, None).await.unwrap();
    }
    let _ = hx_jobs::claim(&f.store, 120).await.unwrap();
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
        let job = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
        assert_eq!(job.kind, "normalize");
        assert!(
            hx_jobs::finish(&f.store, &job, Some("synthetic normalization failure"))
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
    let pool = f.store.clone();
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

#[tokio::test]
async fn empty_windows_finish_without_provider_and_reopen_on_data() {
    let f = Fixture::new().await;
    let w = window();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Fails,
    };
    let g = f.store.generation(w).await.unwrap();
    f.store.request_work("compile", w, g).await.unwrap();
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert!(
        !hx_server::processing::interpret_window(&f.store, w, &Fails)
            .await
            .unwrap()
    );
    let invalid_fx = hx_server::fx::Config {
        binary: "/nonexistent/synthetic-fx".into(),
        credential: "/nonexistent/synthetic-auth".into(),
        timeout: std::time::Duration::from_secs(1),
        max_steps: 2,
    };
    assert!(
        !hx_server::fx::interpret_window(&f.store, w, "empty-fx", &invalid_fx)
            .await
            .unwrap()
    );
    for _ in 0..3 {
        hx_server::processing::schedule(&f.store).await.unwrap();
    }
    assert!(
        f.store
            .job_states()
            .await
            .unwrap()
            .iter()
            .all(|j| j.kind != "interpret")
    );
    assert!(!f.store.window_state(w).await.unwrap().dirty);
    let mut hidden = event("hidden", 8);
    hidden.sensitivity = Sensitivity::Restricted;
    ingest(&f.store, vec![hidden]).await;
    f.store.normalize_window(w).await.unwrap();
    assert!(
        !hx_server::processing::interpret_window(&f.store, w, &Fails)
            .await
            .unwrap()
    );
    ingest(&f.store, vec![event("arrived", 8)]).await;
    f.store.normalize_window(w).await.unwrap();
    assert!(f.store.window_state(w).await.unwrap().generation > g);
    assert!(
        hx_server::processing::interpret_window(&f.store, w, &hx_server::processing::StubProvider)
            .await
            .unwrap()
    );
    // An older canonical entry must not make an empty current version eligible
    // for the daily sweep after the ordinary scheduler has finished it.
    let emptied = hx_store::window_at(w.end + chrono::Duration::days(2));
    let old = f.store.generation(emptied).await.unwrap();
    f.store
        .publish_revision(emptied, old, json!({"tasks":[]}))
        .await
        .unwrap();
    f.store.change_input(emptied).await.unwrap();
    assert!(
        !hx_server::processing::interpret_window(&f.store, emptied, &Fails)
            .await
            .unwrap()
    );
    f.store.finish_revision24(emptied, old + 1).await.unwrap();
    f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
    assert!(
        f.store.sweep_reservations().await.unwrap().is_empty(),
        "an empty completed version must not be scheduled by the daily sweep"
    );
    f.close().await;
}

#[tokio::test]
async fn task_changes_invalidate_windows_within_the_prior_context_lookback() {
    let f = Fixture::new().await;
    let w = window();
    let next = hx_store::window_at(w.end);
    let used = hx_store::window_at(next.end);
    let fx_used = hx_store::window_at(used.end);
    let unrelated = hx_store::window_at(fx_used.end);
    let distant = hx_store::window_at(w.end + chrono::Duration::days(30));
    let g = f.store.generation(w).await.unwrap();
    let (prior, _) = f
        .store
        .publish_revision_key(
            w,
            g,
            json!({"tasks":[{"title":"old","open":true}]}),
            "prior",
        )
        .await
        .unwrap();
    for (win, key, manifest, agent) in [
        (next, "legacy", json!({}), serde_json::Value::Null),
        (
            used,
            "used",
            json!({"prior_revision_ids":[prior]}),
            serde_json::Value::Null,
        ),
        (
            fx_used,
            "fx-used",
            json!({}),
            json!({"calls":[{"tool":"prior_context","result":{"revisions":[{"revision_id":prior}]}}]}),
        ),
        (
            unrelated,
            "unrelated",
            json!({"prior_revision_ids":[]}),
            serde_json::Value::Null,
        ),
        (
            distant,
            "distant",
            json!({"prior_revision_ids":[]}),
            serde_json::Value::Null,
        ),
    ] {
        let generation = f.store.generation(win).await.unwrap();
        let mut value = json!({"tasks":[],"input_manifest":manifest});
        if !agent.is_null() {
            value["agent_run"] = agent;
        }
        f.store
            .publish_revision_key(win, generation, value, key)
            .await
            .unwrap();
    }
    let before = [next, used, fx_used, unrelated, distant];
    let mut generations = vec![];
    for win in before {
        generations.push(f.store.generation(win).await.unwrap());
    }
    f.store
        .publish_revision_key(
            w,
            g,
            json!({"tasks":[{"title":"changed","open":true}]}),
            "changed",
        )
        .await
        .unwrap();
    for (i, win) in before.into_iter().enumerate() {
        assert_eq!(
            f.store.generation(win).await.unwrap(),
            generations[i] + i64::from(i < 4),
            "{win:?}"
        );
    }
    f.close().await;
}

#[tokio::test]
async fn entity_changes_invalidate_only_evidence_windows() {
    let f = Fixture::new().await;
    let w = window();
    let unrelated = hx_store::window_at(w.end);
    let mut span = event("entity-evidence", 8);
    span.time.source_end = Some(w.end);
    ingest(&f.store, vec![span]).await;
    let evidence = f.store.observations(w, false).await.unwrap()[0].id;
    let old = f.store.generation(w).await.unwrap();
    let unrelated_g = f.store.generation(unrelated).await.unwrap();
    f.store
        .assert_entity(
            "Synthetic project",
            json!({"name":"synthetic"}),
            &[evidence],
        )
        .await
        .unwrap();
    assert_eq!(f.store.generation(w).await.unwrap(), old + 1);
    assert_eq!(f.store.generation(unrelated).await.unwrap(), unrelated_g);
    f.close().await;
}

#[tokio::test]
async fn packet_tracks_only_prior_tasks_it_sent() {
    let f = Fixture::new().await;
    let w = window();
    let mut ids = vec![];
    for hours in [12, 8, 4] {
        let prior = hx_store::window_at(w.start - chrono::Duration::hours(hours));
        let mut e = event(&format!("prior-evidence-{hours}"), 8);
        e.time = ObservationTime::point(prior.start);
        ingest(&f.store, vec![e]).await;
        let evidence = f.store.observations(prior, false).await.unwrap()[0].id;
        let g = f.store.generation(prior).await.unwrap();
        let value = json!({"episodes":[{"evidence":[evidence]}],"summary":"Synthetic prior", "tasks":[{
            "id":format!("synthetic-{hours}"),"title":"Synthetic task","description":"Synthetic",
            "intervals":[],"episode_ids":[],"confidence":0.5,"open":true
        }],"input_manifest":{"disclosure_policy":{"digest":hx_server::disclosure::DisclosurePolicy::default().digest()},"sensitive_egress":"full"}});
        ids.push(f.store.publish_revision(prior, g, value).await.unwrap().0);
    }
    let prepared = hx_server::processing::prepare_input_with_policy(
        &f.store,
        w,
        "stub",
        hx_server::processing::SensitiveEgress::Full,
        &hx_server::disclosure::DisclosurePolicy::default(),
    )
    .await
    .unwrap();
    assert_eq!(prepared.packet.open_tasks.len(), 1);
    assert_eq!(prepared.manifest["prior_revision_ids"], json!([ids[2]]));
    f.close().await;
}

#[derive(Default)]
struct Counts(std::sync::atomic::AtomicUsize);
impl hx_interpret::Provider for Counts {
    fn name(&self) -> String {
        "synthetic-counting".into()
    }
    async fn complete(&self, _: &str, _: &str) -> Result<String, hx_interpret::InterpretError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!({"episodes":[],"tasks":[],"summary":"Synthetic"}).to_string())
    }
}
async fn drain(store: &Store, runner: &hx_server::processing::Runner<Counts>) {
    store.settle_inputs().await.unwrap();
    for _ in 0..30 {
        if !hx_jobs::run_one(store, runner).await.unwrap() {
            return;
        }
    }
    panic!("synthetic queue did not drain");
}
#[tokio::test]
async fn late_data_recompiles_without_unbounded_paid_reruns() {
    let f = Fixture::new().await;
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    for hour in [8, 12] {
        ingest(&f.store, vec![event(&format!("first-{hour}"), hour)]).await;
        let w = hx_store::window_at(event("at", hour).time.observed_at);
        f.store.normalize_window(w).await.unwrap();
        // Historical windows need explicit initial runs; late data is still automatic.
        hx_server::processing::interpret_window(&f.store, w, &runner.provider)
            .await
            .unwrap();
    }
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    for hour in [8, 12] {
        ingest(&f.store, vec![event(&format!("late-{hour}"), hour)]).await;
    }
    drain(&f.store, &runner).await;
    assert_eq!(
        f.store
            .search("nebula", true, &["test:".into()])
            .await
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "late data compiles cheaply, without primary reruns"
    );
    let (a, b) = tokio::join!(
        hx_server::processing::schedule(&f.store),
        hx_server::processing::schedule(&f.store)
    );
    a.unwrap();
    b.unwrap();
    let pending = f.store.job_states().await.unwrap();
    assert!(
        pending
            .iter()
            .filter(|j| j.kind == "interpret" && j.state == "ready")
            .count()
            <= 1,
        "{pending:?}, sweeps: {:?}",
        f.store.sweep_reservations().await.unwrap()
    );
    for _ in 0..3 {
        hx_server::processing::schedule(&f.store).await.unwrap();
        drain(&f.store, &runner).await;
    }
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "daily sweep admits at most one stale window globally"
    );
    // The unswept window remains available for an explicit owner request.
    let mut owner_ran = false;
    for hour in [8, 12] {
        owner_ran |= hx_server::processing::interpret_window(
            &f.store,
            hx_store::window_at(event("at", hour).time.observed_at),
            &runner.provider,
        )
        .await
        .unwrap();
    }
    assert!(owner_ran);
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        4
    );
    f.close().await;
}
#[tokio::test]
async fn revision24_compares_timeline_hash_instead_of_generation() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(Utc::now() - chrono::Duration::hours(40));
    let at = |key: &str, start| {
        let mut e = event(key, 8);
        e.time = ObservationTime::point(start);
        e
    };
    ingest(&f.store, vec![at("first", w.start)]).await;
    f.store.normalize_window(w).await.unwrap();
    let p = Counts::default();
    assert!(
        hx_server::processing::interpret_window(&f.store, w, &p)
            .await
            .unwrap()
    );
    f.store.change_input(w).await.unwrap(); // Context changed; timeline did not.
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: p,
    };
    let g = f.store.generation(w).await.unwrap();
    f.store.request_work("compile", w, g).await.unwrap();
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "unchanged timeline must not buy the 24h pass"
    );
    let other = hx_store::window_at(w.end);
    ingest(&f.store, vec![at("other-first", other.start)]).await;
    f.store.normalize_window(other).await.unwrap();
    assert!(
        hx_server::processing::interpret_window(&f.store, other, &runner.provider)
            .await
            .unwrap()
    );
    ingest(&f.store, vec![at("other-late-before-pass", other.start)]).await;
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "changed timeline gets its one 24h pass"
    );
    hx_server::processing::schedule(&f.store).await.unwrap();
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "context-only refreshes cannot buy a sweep when the timeline is unchanged"
    );
    f.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn fx_stops_on_changed_input_at_tool_boundary_and_live_lookup_is_local() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let w = window();
    ingest(&f.store, vec![event("before-run", 8)]).await;
    f.store.normalize_window(w).await.unwrap();
    let g = f.store.generation(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let binary = scratch.path().join("synthetic-fx");
    std::fs::write(&binary, r#"#!/usr/bin/python3
import sys, os, json, pathlib, urllib.request, time
if '--version' in sys.argv:
 print('0.0.16'); sys.exit(0)
sys.stdin.read()
root=pathlib.Path(__file__).parent
server=json.loads((pathlib.Path(os.environ['HOME'])/'.fx/mcp.json').read_text())['mcpServers']['hyperthymesia']
def call(name, args):
 request=urllib.request.Request(server['url'], data=json.dumps({'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':name,'arguments':args}}).encode(), headers={'Content-Type':'application/json','Authorization':'Bearer '+os.environ['HX_MCP_TOKEN']})
 return json.load(urllib.request.urlopen(request))['result']
assert not call('live_lookup', {'source':'synthetic','request':'synthetic lookup'})['isError']
(root/'ready').touch()
while not (root/'changed').exists(): time.sleep(0.005)
call('list_sources', {})
time.sleep(10)
(root/'finished-doomed-run').touch()
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let credential = scratch.path().join("synthetic-auth.json");
    std::fs::write(&credential, "{}").unwrap();
    let config = hx_server::fx::Config {
        binary,
        credential,
        timeout: std::time::Duration::from_secs(8),
        max_steps: 10,
    };
    let store = f.store.clone();
    let mut task = tokio::spawn(async move {
        hx_server::fx::interpret_window(&store, w, "synthetic-cancelled", &config).await
    });
    for _ in 0..1000 {
        if scratch.path().join("ready").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        scratch.path().join("ready").exists(),
        "scripted fx did not reach live lookup"
    );
    assert_eq!(
        f.store.generation(w).await.unwrap(),
        g,
        "live lookup must not mutate outside input"
    );
    ingest(&f.store, vec![event("outside-change", 8)]).await;
    std::fs::write(scratch.path().join("changed"), "").unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await;
    if outcome.is_err() {
        task.abort();
        let _ = task.await;
        panic!("fx did not stop at the tool boundary after outside input changed");
    }
    let error = outcome.unwrap().unwrap().unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<hx_jobs::Deferred>(),
            Some(hx_jobs::Deferred::InputChanged)
        ),
        "{error:?}"
    );
    assert_eq!(f.store.record_count("revisions").await.unwrap(), 0);
    assert!(!scratch.path().join("finished-doomed-run").exists());
    f.close().await;
}

// PR #44 review regressions. All data is synthetic and providers are local fakes.
#[tokio::test]
async fn review_legacy_windows_do_not_buy_a_24h_rerun() {
    let f = Fixture::through_migration(9).await;
    let old = window();
    let recent = hx_store::window_at(Utc::now() - chrono::Duration::hours(4));
    for w in [old, recent] {
        let g = f.store.generation(w).await.unwrap();
        f.store
            .publish_revision(w, g, json!({"tasks":[]}))
            .await
            .unwrap();
    }
    f.store.migrate().await.unwrap();
    assert!(
        f.store
            .interpretation_state(old)
            .await
            .unwrap()
            .revision24_finished,
        "migration must retire the 24h pass for old windows"
    );
    assert!(
        !f.store
            .interpretation_state(recent)
            .await
            .unwrap()
            .revision24_finished
    );
    for w in [old, recent] {
        f.store.change_input(w).await.unwrap();
        hx_server::processing::compile_window(&f.store, w)
            .await
            .unwrap();
        assert!(
            !f.store
                .interpretation_state(w)
                .await
                .unwrap()
                .timeline_changed,
            "a missing legacy hash is unknown, not changed"
        );
    }
    f.close().await;
}

#[tokio::test]
async fn review_revision24_waits_for_upload_backlog_to_settle() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(Utc::now() - chrono::Duration::hours(32));
    let at = |key: &str| {
        let mut e = event(key, 8);
        e.time = ObservationTime::point(w.start);
        e
    };
    ingest(&f.store, vec![at("before-backlog")]).await;
    f.store.normalize_window(w).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    hx_server::processing::interpret_window(&f.store, w, &runner.provider)
        .await
        .unwrap();
    // Three upload batches, with no wall-clock sleeps or process-global env changes.
    for batch in 0..3 {
        ingest(&f.store, vec![at(&format!("backlog-{batch}"))]).await;
        for _ in 0..10 {
            if !hx_jobs::run_one(&f.store, &runner).await.unwrap() {
                break;
            }
        }
        assert_eq!(
            runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "unsettled 24h passes must not start a provider"
        );
        assert!(
            !f.store
                .interpretation_state(w)
                .await
                .unwrap()
                .revision24_finished
        );
        let jobs = f.store.job_states().await.unwrap();
        let pass = jobs
            .iter()
            .find(|j| j.output_key.starts_with("revision24:"))
            .unwrap();
        assert_eq!(pass.state, "ready");
        assert_eq!(
            pass.attempts, 0,
            "readiness deferrals do not spend attempts"
        );
    }
    f.store.settle_inputs().await.unwrap();
    f.store
        .make_jobs_due("ready", Some("interpret"))
        .await
        .unwrap();
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(
        f.store
            .interpretation_state(w)
            .await
            .unwrap()
            .revision24_finished
    );
    f.close().await;
}

struct ReviewOcr;
impl hx_visual::OcrEngine for ReviewOcr {
    fn ocr(&self, _: &[u8], _: &str) -> anyhow::Result<hx_visual::OcrResult> {
        Ok(hx_visual::OcrResult {
            engine_name: "synthetic".into(),
            engine_version: "1".into(),
            coverage: "partial".into(),
            lines: vec![hx_visual::OcrLine {
                text: "Synthetic late OCR".into(),
                confidence: 1.0,
                bbox: hx_visual::BoundingBox {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
            }],
        })
    }
}
#[tokio::test]
async fn review_late_ocr_changes_the_24h_timeline_identity() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(Utc::now() - chrono::Duration::hours(32));
    let at = |key: &str| {
        let mut e = event(key, 8);
        e.time = ObservationTime::point(w.start);
        e
    };
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image::GrayImage::new(8, 8))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let bytes = png.into_inner();
    let sha = hx_store::hash(&bytes);
    let mut still = at("late-ocr-still");
    still.kind = Kind::new("media.still");
    still.payload = json!({"display":"synthetic","trigger":"focus_change","width":8,"height":8,"app_bundle_id":"synthetic.app"});
    still.blobs.push(hx_schema::BlobRef {
        sha256: sha.clone(),
        media_type: "image/png".into(),
        bytes: bytes.len() as u64,
    });
    ingest(&f.store, vec![at("before-ocr"), still]).await;
    f.store.upload_blob(&sha, &bytes).await.unwrap();
    f.store.normalize_window(w).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    hx_server::processing::interpret_window(&f.store, w, &runner.provider)
        .await
        .unwrap();
    hx_server::ocr::discover(&f.store, "synthetic-ocr-v1")
        .await
        .unwrap();
    assert!(
        hx_server::ocr::run_one(&f.store, std::sync::Arc::new(ReviewOcr), "synthetic-ocr-v1")
            .await
            .unwrap()
    );
    hx_server::processing::compile_window(&f.store, w)
        .await
        .unwrap();
    assert!(
        f.store
            .interpretation_state(w)
            .await
            .unwrap()
            .timeline_changed,
        "new OCR lineage must change the identity even when events are unchanged"
    );
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(
        f.store
            .interpretation_state(w)
            .await
            .unwrap()
            .revision24_finished
    );
    f.close().await;
}

async fn stale_sweep_window(store: &Store, hour: u32) -> Interval {
    let w = hx_store::window_at(event("at", hour).time.observed_at);
    ingest(store, vec![event(&format!("sweep-{hour}"), hour)]).await;
    store.normalize_window(w).await.unwrap();
    hx_server::processing::interpret_window(store, w, &hx_server::processing::StubProvider)
        .await
        .unwrap();
    let g = store.generation(w).await.unwrap();
    store.finish_revision24(w, g).await.unwrap();
    ingest(store, vec![event(&format!("sweep-changed-{hour}"), hour)]).await;
    store.normalize_window(w).await.unwrap();
    hx_server::processing::compile_window(store, w)
        .await
        .unwrap();
    // Retire upload-created jobs so the next claim isolates the sweep.
    while let Some(j) = hx_jobs::claim(store, 120).await.unwrap() {
        hx_jobs::finish(store, &j, None).await.unwrap();
    }
    store.settle_inputs().await.unwrap();
    w
}
struct CountsFail(std::sync::atomic::AtomicUsize);
impl hx_interpret::Provider for CountsFail {
    fn name(&self) -> String {
        "synthetic-counting-failure".into()
    }
    async fn complete(&self, _: &str, _: &str) -> Result<String, hx_interpret::InterpretError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(hx_interpret::InterpretError::Provider(
            "synthetic failure".into(),
        ))
    }
}
#[tokio::test]
async fn review_failed_sweep_does_not_hold_future_days() {
    let f = Fixture::new().await;
    let a = stale_sweep_window(&f.store, 8).await;
    let b = stale_sweep_window(&f.store, 12).await;
    let now = Utc::now();
    f.store.schedule_daily_sweep(now).await.unwrap();
    let chosen = f.store.sweep_reservations().await.unwrap()[0].1;
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: CountsFail(std::sync::atomic::AtomicUsize::new(0)),
    };
    assert!(hx_jobs::run_one(&f.store, &runner).await.is_err());
    f.store.make_jobs_due("ready", None).await.unwrap();
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "retrying queue bookkeeping must not retry a paid sweep attempt"
    );
    f.store.make_jobs_due("failed", None).await.unwrap();
    hx_jobs::revive_window_jobs(&f.store).await.unwrap();
    f.store
        .schedule_daily_sweep(now + chrono::Duration::days(1))
        .await
        .unwrap();
    let reservations = f.store.sweep_reservations().await.unwrap();
    assert_eq!(reservations.len(), 2);
    assert_eq!(
        reservations[1].1,
        if chosen == a.start { b.start } else { a.start },
        "failed input must not win another day's sweep"
    );
    f.close().await;
}

#[tokio::test]
async fn review_heartbeat_only_windows_buy_nothing() {
    let f = Fixture::new().await;
    let w = window();
    let mut heartbeat = event("heartbeat", 8);
    heartbeat.kind = Kind::new("collector.health");
    heartbeat.payload = json!({"collector":"synthetic","status":"running","dropped":0});
    let mut clock = event("clock", 8);
    clock.kind = Kind::new("clock.calibration");
    clock.payload = json!({"offset_ms":0,"error_ms":0,"method":"synthetic"});
    let mut presence = event("presence", 8);
    presence.kind = Kind::new("presence.span");
    presence.time.source_end = Some(w.end);
    presence.payload = json!({"state":"active"});
    ingest(&f.store, vec![heartbeat, clock, presence]).await;
    f.store.normalize_window(w).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "housekeeping alone cannot buy an interpretation"
    );
    assert!(
        f.store
            .job_states()
            .await
            .unwrap()
            .iter()
            .all(|j| j.kind != "interpret")
    );
    ingest(&f.store, vec![event("actual-activity", 8)]).await;
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "visible historical activity waits for the capped sweep"
    );
    hx_server::processing::schedule(&f.store).await.unwrap();
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn review_sweep_rescues_newest_exhausted_window_without_a_revision() {
    let f = Fixture::new().await;
    let now = Utc::now();
    for hour in [8, 12] {
        let w = hx_store::window_at(event("at", hour).time.observed_at);
        ingest(&f.store, vec![event(&format!("exhausted-{hour}"), hour)]).await;
        f.store.normalize_window(w).await.unwrap();
        hx_server::processing::compile_window(&f.store, w)
            .await
            .unwrap();
        let g = f.store.generation(w).await.unwrap();
        f.store.request_interpretation(w, g, false).await.unwrap();
        for revival in 0..=3 {
            exhaust_slot(&f.store, &format!("interpret:{}", w.start)).await;
            if revival < 3 {
                f.store.make_jobs_due("failed", None).await.unwrap();
                hx_jobs::revive_window_jobs(&f.store).await.unwrap();
            }
        }
    }
    f.store.settle_inputs().await.unwrap();
    f.store.schedule_daily_sweep(now).await.unwrap();
    let reservations = f.store.sweep_reservations().await.unwrap();
    assert_eq!(
        reservations.len(),
        1,
        "exhausted windows without a revision need the sweep"
    );
    assert_eq!(
        reservations[0].1,
        event("at", 12).time.observed_at,
        "newest candidate first"
    );
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(
        f.store.sweep_reservations().await.unwrap()[0].2,
        "rescue also consumes the daily start"
    );
    f.store.schedule_daily_sweep(now).await.unwrap();
    assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
async fn review_new_open_tasks_stale_later_windows_with_empty_dependencies() {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    f.store
        .publish_revision(w, g, json!({"tasks":[]}))
        .await
        .unwrap();
    let later = hx_store::window_at(w.end + chrono::Duration::hours(8));
    let outside = hx_store::window_at(w.end + chrono::Duration::hours(16));
    for win in [later, outside] {
        let v = f.store.generation(win).await.unwrap();
        f.store
            .publish_revision(
                win,
                v,
                json!({"tasks":[],"input_manifest":{"prior_revision_ids":[]}}),
            )
            .await
            .unwrap();
    }
    let before = f.store.generation(later).await.unwrap();
    let outside_g = f.store.generation(outside).await.unwrap();
    f.store
        .publish_revision(
            w,
            g,
            json!({"tasks":[{"title":"Synthetic new task","open":true}]}),
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.generation(later).await.unwrap(),
        before + 1,
        "empty dependency lists cannot hide newly opened earlier tasks"
    );
    assert_eq!(
        f.store.generation(outside).await.unwrap(),
        outside_g,
        "exclusive 16h lookback"
    );
    assert!(f.store.window_state(later).await.unwrap().dirty);
    assert!(
        f.store.job_states().await.unwrap().is_empty(),
        "invalidation only marks; it must not buy runs"
    );
    f.close().await;
}

#[tokio::test]
async fn review_deferred_sweep_releases_its_unspent_start_but_not_future_days() {
    for retry_today in [true, false] {
        let f = Fixture::new().await;
        let older = stale_sweep_window(&f.store, 8).await;
        let newest = stale_sweep_window(&f.store, 12).await;
        let now = Utc::now();
        f.store.schedule_daily_sweep(now).await.unwrap();
        let day = now.date_naive();
        assert_eq!(
            f.store.sweep_reservations().await.unwrap(),
            vec![(day, newest.start, false)]
        );
        let gate = f.store.hold_interpretation_gate(newest).await.unwrap();
        let runner = hx_server::processing::Runner {
            store: f.store.clone(),
            provider: Counts::default(),
        };
        assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
        assert_eq!(
            runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            f.store.sweep_reservations().await.unwrap(),
            vec![(day, newest.start, false)],
            "provider-busy deferrals must release the unspent start for today's retry"
        );
        let deferred = f
            .store
            .job_states()
            .await
            .unwrap()
            .into_iter()
            .find(|j| j.payload.get("sweep_day").is_some())
            .unwrap();
        assert_eq!(deferred.state, "ready");
        assert_eq!(deferred.attempts, 0);
        gate.release().await.unwrap();
        if !retry_today {
            // Advance the durable reservation and request together, so the real
            // worker takes its expired-day path without sleeps or a fake handler.
            f.store.expire_sweep_day().await.unwrap();
        }
        f.store.make_jobs_due("ready", None).await.unwrap();
        assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
        assert_eq!(
            runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(retry_today)
        );
        assert_eq!(
            f.store.sweep_reservations().await.unwrap()[0].2,
            retry_today
        );
        if !retry_today {
            f.store.schedule_daily_sweep(now).await.unwrap();
            assert_eq!(
                f.store.sweep_reservations().await.unwrap()[1].1,
                older.start,
                "even an unspent reservation cannot win again on unchanged input"
            );
        }
        // New input permits this window to become a candidate again.
        ingest(&f.store, vec![event("new-visible-after-sweep", 12)]).await;
        f.store.normalize_window(newest).await.unwrap();
        hx_server::processing::compile_window(&f.store, newest)
            .await
            .unwrap();
        f.store
            .schedule_daily_sweep(now + chrono::Duration::days(1))
            .await
            .unwrap();
        assert_eq!(
            f.store
                .sweep_reservations()
                .await
                .unwrap()
                .last()
                .unwrap()
                .1,
            newest.start
        );
        f.close().await;
    }
}

#[tokio::test]
async fn cap_upgrade_never_interpreted_backlog_uses_only_the_sweep() {
    let f = Fixture::through_migration(9).await;
    for hour in [0, 8, 16] {
        let w = hx_store::window_at(event("at", hour).time.observed_at);
        ingest(&f.store, vec![event(&format!("legacy-{hour}"), hour)]).await;
        f.store.normalize_window(w).await.unwrap();
        if hour == 8 {
            let g = f.store.generation(w).await.unwrap();
            f.store.request_interpretation(w, g, false).await.unwrap();
            // A pre-upgrade failure has no recorded revival history. Leave the
            // compile backlog intact so all three windows can try to buy runs.
            f.store
                .fail_legacy_job(format!("interpret:{}", w.start))
                .await
                .unwrap();
        }
    }
    f.store.migrate().await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    for _ in 0..3 {
        hx_server::processing::schedule(&f.store).await.unwrap();
        drain(&f.store, &runner).await;
    }
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "all never-interpreted historical windows, including failed legacy jobs, share the daily cap"
    );
    assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 1);
    f.close().await;

    // The cutoff preserves the normal first run for a recently closed window.
    let f = Fixture::new().await;
    let recent = hx_store::window_at(Utc::now() - chrono::Duration::hours(4));
    let mut e = event("recent-first-run", 8);
    e.time.observed_at = recent.start;
    e.time.source_start = Some(recent.start);
    ingest(&f.store, vec![e]).await;
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the primary run remains automatic inside the first 24 hours"
    );
    assert!(f.store.sweep_reservations().await.unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn cap_generation_changes_preserve_paid_attempts_and_revivals() {
    let f = Fixture::new().await;
    // Refreshing a live lease must preserve attempts through both completion
    // and crash recovery, not just while the slot is already terminal.
    let live = hx_store::window_at(window().end);
    let g = f.store.generation(live).await.unwrap();
    f.store
        .request_interpretation(live, g, false)
        .await
        .unwrap();
    let j = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    f.store.change_input(live).await.unwrap();
    f.store
        .request_interpretation(live, g + 1, false)
        .await
        .unwrap();
    hx_jobs::finish(&f.store, &j, Some("synthetic failure"))
        .await
        .unwrap();
    assert_eq!(
        f.store.job_states().await.unwrap()[0].attempts,
        1,
        "finishing an older lease cannot refund paid attempts on a generation bump"
    );
    let j = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    f.store.change_input(live).await.unwrap();
    f.store
        .request_interpretation(live, g + 2, false)
        .await
        .unwrap();
    f.store.expire_lease(j.id, None).await.unwrap();
    let recovered = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(
        recovered.attempts, 3,
        "crash recovery cannot refill the paid budget"
    );
    hx_jobs::finish(&f.store, &recovered, None).await.unwrap();
    f.store.change_input(live).await.unwrap();
    f.store
        .request_interpretation(live, g + 3, false)
        .await
        .unwrap();
    assert_eq!(
        f.store.job_states().await.unwrap()[0].state,
        "done",
        "completed paid slots do not reopen just because input changes"
    );
    let w = window();
    ingest(&f.store, vec![event("budget", 8)]).await;
    f.store.normalize_window(w).await.unwrap();
    let g = f.store.generation(w).await.unwrap();
    for revision24 in [false, true] {
        f.store
            .request_interpretation(w, g, revision24)
            .await
            .unwrap();
        let slot = if revision24 {
            "revision24"
        } else {
            "interpret"
        };
        let key = format!("{slot}:{}", w.start);
        // Exhaust the full budget without invoking the age-based scheduler.
        for revival in 0..=3 {
            exhaust_slot(&f.store, &key).await;
            if revival < 3 {
                f.store.make_jobs_due("failed", None).await.unwrap();
                hx_jobs::revive_window_jobs(&f.store).await.unwrap();
            }
        }
    }
    // Task invalidation and OCR lineage publication both reach this same generation path.
    for _ in 0..3 {
        f.store.change_input(w).await.unwrap();
        let g = f.store.generation(w).await.unwrap();
        for revision24 in [false, true] {
            f.store
                .request_interpretation(w, g, revision24)
                .await
                .unwrap();
        }
        for j in f
            .store
            .job_states()
            .await
            .unwrap()
            .iter()
            .filter(|j| j.kind == "interpret" && j.payload["window"] == json!(w))
        {
            assert_eq!(
                (&*j.state, j.attempts, j.revivals),
                ("failed", 5, 3),
                "a generation bump cannot purchase a new paid budget: {j:?}"
            );
        }
    }
    // The explicit owner path still works on an exhausted window.
    let p = Counts::default();
    assert!(
        hx_server::processing::interpret_window(&f.store, w, &p)
            .await
            .unwrap()
    );
    assert_eq!(p.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn cap_exhausted_24h_pass_reaches_the_sweep() {
    let f = Fixture::new().await;
    let w = stale_sweep_window(&f.store, 8).await;
    f.store.reset_revision24(w.start).await.unwrap();
    let g = f.store.generation(w).await.unwrap();
    f.store.request_interpretation(w, g, true).await.unwrap();
    for revival in 0..=3 {
        exhaust_slot(&f.store, &format!("revision24:{}", w.start)).await;
        if revival < 3 {
            f.store.make_jobs_due("failed", None).await.unwrap();
            hx_jobs::revive_window_jobs(&f.store).await.unwrap();
        }
    }
    f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
    assert_eq!(
        f.store.sweep_reservations().await.unwrap().len(),
        1,
        "an exhausted pass must not require a successful revision24_finished flag"
    );
    assert!(
        f.store
            .interpretation_state(w)
            .await
            .unwrap()
            .revision24_finished,
        "reserving the sweep retires the uncapped pass before any provider work"
    );
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn cap_failed_or_deferred_sweep_retries_after_seven_days() {
    for deferred in [false, true] {
        let f = Fixture::new().await;
        let w = stale_sweep_window(&f.store, 8).await;
        let now = Utc::now();
        f.store.schedule_daily_sweep(now).await.unwrap();
        if deferred {
            let j = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
            hx_jobs::defer(&f.store, &j, &hx_jobs::Deferred::RateLimited)
                .await
                .unwrap();
            f.store.expire_sweep_day().await.unwrap();
            f.store.make_jobs_due("ready", None).await.unwrap();
            let runner = hx_server::processing::Runner {
                store: f.store.clone(),
                provider: Counts::default(),
            };
            assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
            assert_eq!(
                runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
                0
            );
        } else {
            let runner = hx_server::processing::Runner {
                store: f.store.clone(),
                provider: CountsFail(std::sync::atomic::AtomicUsize::new(0)),
            };
            assert!(hx_jobs::run_one(&f.store, &runner).await.is_err());
            f.store.make_jobs_due("ready", None).await.unwrap();
            assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
        }
        let day = f.store.sweep_reservations().await.unwrap()[0].0;
        let boundary = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let before = boundary + chrono::Duration::days(7) - chrono::Duration::seconds(1);
        f.store.schedule_daily_sweep(before).await.unwrap();
        assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 1);
        f.store
            .schedule_daily_sweep(boundary + chrono::Duration::days(7))
            .await
            .unwrap();
        let reservations = f.store.sweep_reservations().await.unwrap();
        assert_eq!(
            reservations.len(),
            2,
            "unchanged failed/deferred input deserves another capped attempt after seven days"
        );
        assert_eq!(reservations[1].1, w.start);
        // Changed input bypasses the cooldown, but still cannot reserve twice in a day.
        let j = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
        hx_jobs::finish(&f.store, &j, None).await.unwrap();
        f.store.change_input(w).await.unwrap();
        hx_server::processing::compile_window(&f.store, w)
            .await
            .unwrap();
        f.store
            .schedule_daily_sweep(boundary + chrono::Duration::days(7))
            .await
            .unwrap();
        assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 2);
        f.store
            .schedule_daily_sweep(boundary + chrono::Duration::days(8))
            .await
            .unwrap();
        assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 3);
        f.close().await;
    }
}

#[tokio::test]
async fn cap_housekeeping_does_not_change_a_visible_windows_paid_identity() {
    let f = Fixture::new().await;
    let w = window();
    ingest(&f.store, vec![event("visible", 8)]).await;
    f.store.normalize_window(w).await.unwrap();
    let p = Counts::default();
    hx_server::processing::interpret_window(&f.store, w, &p)
        .await
        .unwrap();
    for (kind, payload) in [
        (
            "collector.health",
            json!({"collector":"synthetic","status":"running","dropped":0}),
        ),
        (
            "clock.calibration",
            json!({"offset_ms":0,"error_ms":0,"method":"synthetic"}),
        ),
        ("presence.span", json!({"state":"active"})),
    ] {
        let mut e = event(kind, 8);
        e.kind = Kind::new(kind);
        e.payload = payload;
        e.device = Some(hx_schema::DeviceId::new("synthetic-device"));
        e.time.source_end = Some(w.end);
        ingest(&f.store, vec![e]).await;
        f.store.normalize_window(w).await.unwrap();
        hx_server::processing::compile_window(&f.store, w)
            .await
            .unwrap();
        assert!(
            !f.store
                .interpretation_state(w)
                .await
                .unwrap()
                .timeline_changed,
            "{kind} must not buy the 24h pass, including via coverage or presence lanes"
        );
    }
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: p,
    };
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn cap_sweep_leaves_healthy_ready_and_running_jobs_alone() {
    for running in [false, true] {
        let f = Fixture::new().await;
        let w = stale_sweep_window(&f.store, 8).await;
        let g = f.store.generation(w).await.unwrap();
        f.store.request_interpretation(w, g, true).await.unwrap();
        let claimed = if running {
            hx_jobs::claim(&f.store, 120).await.unwrap()
        } else {
            None
        };
        assert_eq!(claimed.is_some(), running);
        let before = f.store.job_states().await.unwrap();
        f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
        assert!(
            f.store.sweep_reservations().await.unwrap().is_empty(),
            "the sweep must not reserve or rewrite a healthy {} job",
            if running { "running" } else { "ready" }
        );
        let after = f.store.job_states().await.unwrap();
        assert_eq!(format!("{before:?}"), format!("{after:?}"));
        if let Some(j) = claimed {
            hx_jobs::finish(&f.store, &j, None).await.unwrap();
        }
        f.close().await;
    }
}

#[tokio::test]
async fn cap_closed_task_changes_mark_no_windows() {
    let f = Fixture::new().await;
    let w = window();
    let g = f.store.generation(w).await.unwrap();
    f.store
        .publish_revision(w, g, json!({"tasks":[{"title":"closed","open":false}]}))
        .await
        .unwrap();
    let mut windows = Vec::new();
    for hours in [0, 4, 12, 16] {
        let later = hx_store::window_at(w.end + chrono::Duration::hours(hours));
        let generation = f.store.generation(later).await.unwrap();
        hx_server::processing::compile_window(&f.store, later)
            .await
            .unwrap();
        windows.push((later, generation));
    }
    f.store
        .publish_revision(
            w,
            g,
            json!({"tasks":[{"title":"renamed closed task","open":false}]}),
        )
        .await
        .unwrap();
    for (later, generation) in windows {
        assert_eq!(
            f.store.generation(later).await.unwrap(),
            generation,
            "closed tasks cannot invalidate prior context"
        );
        assert!(!f.store.window_state(later).await.unwrap().dirty);
    }
    assert!(f.store.job_states().await.unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn close_old_explicit_revision_cannot_open_a_24h_slot() {
    let f = Fixture::new().await;
    let w = window();
    ingest(&f.store, vec![event("old-explicit", 8)]).await;
    f.store.normalize_window(w).await.unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    assert!(
        hx_server::processing::interpret_window(&f.store, w, &runner.provider)
            .await
            .unwrap()
    );
    ingest(&f.store, vec![event("old-changed", 8)]).await;
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an explicit historical run must not create an automatic 24h purchase"
    );
    let jobs = f.store.job_states().await.unwrap();
    assert!(
        !jobs
            .iter()
            .any(|j| j.payload["revision24"] == true && j.state == "ready")
    );
    // A retired (rather than exhausted) 24h slot must not strand changed history.
    let g = f.store.generation(w).await.unwrap();
    f.store.request_interpretation(w, g, true).await.unwrap();
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an existing historical 24h job cannot buy a run either"
    );
    assert!(
        !f.store
            .interpretation_state(w)
            .await
            .unwrap()
            .revision24_finished
    );
    f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
    assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 1);
    drain(&f.store, &runner).await;
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    f.close().await;
}

#[tokio::test]
async fn close_sweep_requires_a_changed_recorded_timeline_hash() {
    for legacy in [false, true] {
        let f = Fixture::new().await;
        let w = window();
        ingest(&f.store, vec![event("visible-hash", 8)]).await;
        f.store.normalize_window(w).await.unwrap();
        let g = f.store.generation(w).await.unwrap();
        if legacy {
            f.store
                .publish_revision(w, g, json!({"tasks":[],"summary":"legacy without a hash"}))
                .await
                .unwrap();
        } else {
            hx_server::processing::interpret_window(
                &f.store,
                w,
                &hx_server::processing::StubProvider,
            )
            .await
            .unwrap();
        }
        f.store.finish_revision24(w, g).await.unwrap();
        f.store.change_input(w).await.unwrap(); // Prior context alone.
        let mut heartbeat = event("housekeeping-hash", 8);
        heartbeat.kind = Kind::new("collector.health");
        heartbeat.payload = json!({"collector":"synthetic","status":"running","dropped":0});
        ingest(&f.store, vec![heartbeat]).await;
        f.store.normalize_window(w).await.unwrap();
        hx_server::processing::compile_window(&f.store, w)
            .await
            .unwrap();
        f.store.finish_all_jobs().await.unwrap();
        f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
        assert!(
            f.store.sweep_reservations().await.unwrap().is_empty(),
            "context and housekeeping cannot buy a sweep, legacy={legacy}"
        );
        ingest(&f.store, vec![event("real-change-hash", 8)]).await;
        f.store.normalize_window(w).await.unwrap();
        hx_server::processing::compile_window(&f.store, w)
            .await
            .unwrap();
        f.store.finish_all_jobs().await.unwrap();
        f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
        assert_eq!(
            f.store.sweep_reservations().await.unwrap().len(),
            usize::from(!legacy),
            "a revision with no recorded hash requires an explicit request"
        );
        assert!(
            hx_server::processing::interpret_window(
                &f.store,
                w,
                &hx_server::processing::StubProvider
            )
            .await
            .unwrap(),
            "explicit requests remain available for changed or hashless history"
        );
        f.close().await;
    }
}

#[tokio::test]
async fn close_empty_sweep_candidate_releases_today_for_visible_history() {
    let f = Fixture::new().await;
    let visible = stale_sweep_window(&f.store, 8).await;
    let empty = hx_store::window_at(event("empty", 12).time.observed_at);
    f.store.generation(empty).await.unwrap();
    hx_server::processing::compile_window(&f.store, empty)
        .await
        .unwrap();
    // This window has compiled but admission has not classified it yet.
    f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
    assert_eq!(
        f.store.sweep_reservations().await.unwrap()[0].1,
        empty.start
    );
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: Counts::default(),
    };
    drain(&f.store, &runner).await;
    let reservations = f.store.sweep_reservations().await.unwrap();
    assert_eq!(
        reservations,
        vec![(Utc::now().date_naive(), visible.start, true)],
        "the same worker tick must replace empty admission with real work"
    );
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.close().await;
}
