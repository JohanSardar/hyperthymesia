use anyhow::Result;
use chrono::Duration;
use hx_server::{disclosure::DisclosurePolicy, mcp::Broker, processing::SensitiveEgress};
use hx_store::Store;
use serde_json::json;

#[tokio::test]
async fn broker_cannot_read_or_cite_another_runs_live_snapshot() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::create(dir.path()).await?;
    let source = store.connect_source("Synthetic", "Fake only", 1).await?;
    let mut policy = DisclosurePolicy::laptop();
    policy
        .source_grants
        .insert(source.connector_instance.clone());
    let original = Broker::new(
        store.clone(),
        policy.clone(),
        SensitiveEgress::Full,
        hx_store::window_at(chrono::Utc::now()),
    );
    let id = store
        .save_live_snapshot(
            original.run_token(),
            &source.id,
            json!({"answer":"old run"}),
        )
        .await?;
    assert!(original.allowed_ids(&[id]).await?);
    assert!(original.clone().allowed_ids(&[id]).await?);
    assert_eq!(
        original.call("read_evidence", json!({"id":id})).await?["evidence"][0]["id"],
        id
    );
    let later = hx_store::window_at(chrono::Utc::now() + Duration::days(150));
    let broker = Broker::new(store.clone(), policy.clone(), SensitiveEgress::Full, later);
    assert!(
        !broker.allowed_ids(&[id]).await?,
        "another run must not cite live evidence"
    );
    assert!(broker.call("read_evidence", json!({"id":id})).await?["error"].is_string());
    assert!(
        store.evidence(id).await?.is_some(),
        "owner reads must retain historical evidence"
    );
    assert!(broker.audit.lock().await.evidence.is_empty());
    assert!(
        store
            .evidence_for_run(id, broker.run_token())
            .await?
            .is_none()
    );
    assert!(
        store
            .source_envelope_for_run(id, broker.run_token())
            .await?
            .is_none()
    );
    assert!(
        store
            .source_namespace_for_run(id, broker.run_token())
            .await?
            .is_none()
    );
    assert!(
        store
            .evidence_for_run(id, original.run_token())
            .await?
            .is_some()
    );
    assert_eq!(
        store
            .source_namespace_for_run(id, original.run_token())
            .await?,
        Some(source.connector_instance.clone())
    );
    // Owner revision views retain historical snapshots without borrowing the
    // broker's run-scoped citation admission to inspect an earlier run.
    let generation = store.generation(original.window).await?;
    let (revision, published) = store.publish_revision_manifest(
        original.window, generation,
        json!({"episodes":[{"evidence":[id]}],"tasks":[],"summary":"Synthetic historical result"}),
        "synthetic-run", std::slice::from_ref(&source.connector_instance),
    ).await?;
    assert!(published);
    let reader = hx_store::Token {
        prefixes: vec!["source:".into()],
        scopes: vec!["read_l3".into()],
    };
    let timeline = hx_server::timeline_value(&store, &reader, &policy, original.window, "episodes")
        .await
        .unwrap();
    assert_eq!(timeline["revisions"][0]["revision_id"], revision);
    store.set_source_enabled(&source.id, false).await?;
    let owner = hx_store::Token {
        prefixes: vec!["source:".into()],
        scopes: vec!["read_raw".into()],
    };
    assert_eq!(
        hx_server::authorized_evidence(&store, &owner, id)
            .await
            .unwrap()
            .get("id"),
        Some(&json!(id))
    );
    Ok(())
}
