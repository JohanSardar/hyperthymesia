use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration as StdDuration;
use tokio::sync::Notify;

async fn fixture() -> (tempfile::TempDir, Broker, hx_store::sources::Source) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let source = store
        .connect_source("Synthetic", "Fake source only", 1)
        .await
        .unwrap();
    let mut policy = DisclosurePolicy::laptop();
    policy
        .source_grants
        .insert(source.connector_instance.clone());
    let broker = Broker::new(
        store,
        policy,
        SensitiveEgress::Full,
        hx_store::window_at(Utc::now()),
    );
    (dir, broker, source)
}
fn live_args(source: &str) -> Value {
    json!({"source":source,"request":{"kind":"http","method":"GET","url":"https://example.invalid"},"timeout_s":180})
}

#[tokio::test]
async fn broker_rechecks_source_grants_after_stored_tools() {
    // The execution seam models revocation after the initial policy check,
    // before a stored tool has finished reading. No timing race or sleep.
    let mut leaked = Vec::new();
    for name in [
        "prior_context",
        "read_evidence",
        "search",
        "timeline",
        "window_overview",
        "screen_at",
        "list_sources",
    ] {
        let (_dir, mut broker, source) = fixture().await;
        broker.test_execute = Some(Arc::new({
            let store = broker.store.clone();
            let source = source.clone();
            let run_token = broker.run_token();
            move |name, _| {
                let store = store.clone();
                let source = source.clone();
                Box::pin(async move {
                    let id = store
                        .save_live_snapshot(run_token, &source.id, json!({"synthetic":"secret"}))
                        .await?;
                    store.set_source_enabled(&source.id, false).await?;
                    if name == "prior_context" {
                        Ok(json!({"revisions":[{"summary":"revoked in flight",
                                "input_namespaces":[source.connector_instance]}]}))
                    } else {
                        Ok(json!({"evidence":[EvidenceItem {
                            effective_class: Sensitivity::Sensitive,
                            id,
                            at: Utc::now(),
                            kind: "sensitive:live.snapshot".into(),
                            text: "revoked in flight".into(),
                            self_generated: false,
                        }]}))
                    }
                })
            }
        }));
        let args = match name {
            "read_evidence" => json!({"id":1}),
            "search" => json!({"query":"synthetic"}),
            "screen_at" => json!({"time":broker.window.start}),
            _ => json!({}),
        };
        let result = broker.call(name, args).await.unwrap();
        if result.to_string().contains("revoked in flight") {
            leaked.push(name);
        }
        assert!(
            !broker
                .store
                .source(&source.id)
                .await
                .unwrap()
                .unwrap()
                .enabled,
            "{name} must execute before revocation is checked"
        );
        let audit = broker.audit.lock().await;
        assert!(audit.evidence.is_empty());
        assert!(audit.input_namespaces.is_empty());
    }
    assert!(
        leaked.is_empty(),
        "in-flight revocation leaked through {leaked:?}"
    );
}

#[tokio::test]
async fn broker_source_grant_refresh_failure_withholds_results() {
    for fail_before in [true, false] {
        let (_dir, mut broker, _source) = fixture().await;
        let executions = Arc::new(AtomicUsize::new(0));
        broker.test_execute = Some(Arc::new({
            let store = broker.store.clone();
            let executions = executions.clone();
            move |_, _| {
                let store = store.clone();
                let executions = executions.clone();
                Box::pin(async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    store.pool.close().await;
                    Ok(json!({"secret":"must be withheld"}))
                })
            }
        }));
        if fail_before {
            broker.store.pool.close().await;
        }
        let result = broker.call("list_sources", json!({})).await.unwrap();
        assert!(!result.to_string().contains("must be withheld"));
        assert!(result.get("error").is_some());
        assert_eq!(executions.load(Ordering::SeqCst), usize::from(!fail_before));
        assert_eq!(broker.audit.lock().await.lookups, 1);
    }
}

#[tokio::test]
async fn broker_slow_tool_releases_lock_and_cancel_keeps_reservation() {
    let (_dir, mut broker, source) = fixture().await;
    broker.live_limit = 20;
    // A store with historical live evidence must keep it through a failed or cancelled call.
    let historical = broker
        .store
        .save_live_snapshot(
            ulid::Ulid::generate(),
            &source.id,
            json!({"old":"synthetic history"}),
        )
        .await
        .unwrap();
    let entered = Arc::new(Notify::new());
    broker.test_execute = Some(Arc::new({
        let entered = entered.clone();
        move |name, _| {
            let entered = entered.clone();
            Box::pin(async move {
                if name == "live_lookup" {
                    entered.notify_one();
                    tokio::time::sleep(StdDuration::from_secs(5)).await;
                }
                Ok(json!({"evidence":[]}))
            })
        }
    }));
    let slow = tokio::spawn({
        let broker = broker.clone();
        let args = live_args(&source.id);
        async move { broker.call("live_lookup", args).await }
    });
    entered.notified().await;
    let second = tokio::time::timeout(
        StdDuration::from_secs(1),
        broker.call("list_sources", json!({})),
    )
    .await;
    assert!(second.is_ok(), "a five-second tool blocked another call");
    assert!(second.unwrap().unwrap().get("error").is_none());
    {
        let audit = broker.audit.lock().await;
        assert!(
            audit.input_namespaces.contains(&source.connector_instance),
            "source must be recorded before execution"
        );
    }
    assert!(slow.await.unwrap().unwrap().get("error").is_none());
    let cancelled = tokio::spawn({
        let broker = broker.clone();
        let args = live_args(&source.id);
        async move { broker.call("live_lookup", args).await }
    });
    entered.notified().await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert_eq!(broker.audit.lock().await.live_calls, 2);
    // Exactly two live calls were spent, even though only one completed.
    for _ in 2..20 {
        assert!(
            broker
                .call(
                    "agent_sql",
                    json!({"source":source.id,"sql":"SELECT 1","params":[]})
                )
                .await
                .unwrap()
                .get("error")
                .is_none()
        );
    }
    let capped = broker
        .call(
            "agent_sql",
            json!({"source":source.id,"sql":"SELECT 1","params":[]}),
        )
        .await
        .unwrap();
    assert!(
        capped["error"]
            .as_str()
            .unwrap()
            .contains("live call budget")
    );
    assert!(broker.store.evidence(historical).await.unwrap().is_some());
}

#[tokio::test]
async fn broker_lookup_and_live_budgets_are_separate() {
    let (_dir, mut broker, source) = fixture().await;
    broker.live_limit = 20;
    broker.lookup_limit = 1;
    let executions = Arc::new(AtomicUsize::new(0));
    broker.test_execute = Some(Arc::new({
        let executions = executions.clone();
        move |_, _| {
            executions.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(json!({"evidence":[]})) })
        }
    }));
    assert!(
        broker
            .call("live_lookup", live_args(&source.id))
            .await
            .unwrap()
            .get("error")
            .is_none()
    );
    assert!(
        broker
            .call("list_sources", json!({}))
            .await
            .unwrap()
            .get("error")
            .is_none(),
        "live calls spent stored lookup budget"
    );
    assert!(
        broker.call("list_sources", json!({})).await.unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("lookup budget")
    );
    for _ in 1..20 {
        assert!(
            broker
                .call("live_lookup", live_args(&source.id))
                .await
                .unwrap()
                .get("error")
                .is_none(),
            "stored lookups spent live call budget"
        );
    }
    assert!(
        broker
            .call("live_lookup", live_args(&source.id))
            .await
            .unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("live call budget")
    );
    assert_eq!(executions.load(Ordering::SeqCst), 21);
    for _ in 23..MAX_CALLS {
        broker.call("list_sources", json!({})).await.unwrap();
    }
    assert!(broker.call("list_sources", json!({})).await.is_err());
    assert!(broker.audit.lock().await.exhausted);
}

#[tokio::test]
async fn broker_three_consecutive_failures_disable_only_that_source() {
    let (_dir, mut broker, source) = fixture().await;
    broker.live_limit = 20;
    let other = broker
        .store
        .connect_source("Other", "Synthetic only", 1)
        .await
        .unwrap();
    broker
        .policy
        .source_grants
        .insert(other.connector_instance.clone());
    let executions = Arc::new(AtomicUsize::new(0));
    broker.test_execute = Some(Arc::new({
        let executions = executions.clone();
        move |_, _| {
            let n = executions.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                // A success resets the streak; subsequent failure messages stay distinct.
                if n == 1 {
                    Ok(json!({"evidence":[]}))
                } else {
                    bail!("synthetic failure {n}")
                }
            })
        }
    }));
    for n in 0..5 {
        let result = broker
            .call("live_lookup", live_args(&source.id))
            .await
            .unwrap();
        if n == 1 {
            assert!(result.get("error").is_none());
        } else {
            assert_eq!(result["error"], format!("synthetic failure {n}"));
            assert_eq!(result["retryable"], n != 4);
            assert_eq!(result["failed_calls"], if n == 0 { 1 } else { n - 1 });
        }
    }
    let off = broker
        .call(
            "source_bash",
            json!({"source":source.id,"command":"echo synthetic"}),
        )
        .await
        .unwrap();
    assert!(
        off["error"]
            .as_str()
            .unwrap()
            .contains("disabled for this run")
    );
    assert_eq!(off["retryable"], false);
    assert_eq!(
        executions.load(Ordering::SeqCst),
        5,
        "disabled source must not execute or retry"
    );
    let other_result = broker
        .call("live_lookup", live_args(&other.id))
        .await
        .unwrap();
    assert_eq!(other_result["failed_calls"], 1);
    assert_eq!(executions.load(Ordering::SeqCst), 6);
    assert!(broker.audit.lock().await.evidence.is_empty());
    assert!(
        !broker.audit.lock().await.exhausted,
        "live failures must not fail a window run"
    );
}
