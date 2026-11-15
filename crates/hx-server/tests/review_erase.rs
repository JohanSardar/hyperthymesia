//! Review proofs: synthetic history in disposable state only.
use anyhow::Result;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, TimeZone, Utc};
use hx_schema::{
    ConnectorInstance, Envelope, Kind, ObservationId, ObservationTime, Operation, Revision,
    Sensitivity, SourceKey,
};
use hx_server::{disclosure::DisclosurePolicy, mcp::Broker, processing::SensitiveEgress};
use hx_store::{Store, Token, UploadBatch};
use serde_json::{Value, json};
use tower::ServiceExt;

/// The server allows one backup or delete at a time per process; take turns.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 0).unwrap()
}

async fn record(
    store: &Store,
    start: chrono::DateTime<Utc>,
    end: Option<chrono::DateTime<Utc>>,
) -> Result<i64> {
    let mut time = ObservationTime::point(start);
    time.source_end = end;
    let envelope = Envelope {
        envelope_version: 1,
        observation_id: ObservationId::generate(),
        connector_instance: ConnectorInstance::new("source:review"),
        source_key: SourceKey::new("retreat"),
        revision: Revision::append_only(),
        operation: Operation::Observe,
        sequence: None,
        kind: Kind::new("source.calendar"),
        schema_version: 1,
        time,
        device: None,
        actor: None,
        subject: None,
        resource: None,
        links: vec![],
        coverage: None,
        sensitivity: Sensitivity::Normal,
        payload: json!({"title":"private retreat"}),
        blobs: vec![],
    };
    let ack = store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![envelope],
            },
            &Token {
                prefixes: vec!["source:".into()],
                scopes: vec!["ingest".into()],
            },
        )
        .await?;
    assert_eq!(ack.accepted, 1);
    Ok(
        sqlx::query_scalar("SELECT id FROM events WHERE source_key='retreat'")
            .fetch_one(&store.pool)
            .await?,
    )
}

async fn erase(store: &Store, _clear_tables: bool) -> Result<(StatusCode, Value)> {
    let token = store
        .create_token(
            "review owner",
            vec![
                "source:".into(),
                "macos-agent:".into(),
                "macos-media:".into(),
                "companion:".into(),
            ],
            vec!["read_raw".into()],
        )
        .await?;
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::post("/v1/erase")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"start":at(), "end":at()+Duration::hours(1), "confirm":"delete"})
                        .to_string(),
                ))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
    ))
}

#[tokio::test]
async fn s1_erase_removes_overlapping_multiday_connection_record() -> Result<()> {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir()?;
    let store = Store::create(&dir.path().join("state")).await?;
    let id = record(
        &store,
        at() - Duration::days(3),
        Some(at() + Duration::days(1)),
    )
    .await?;
    let (status, response) = erase(&store, true).await?;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(
        store.evidence(id).await?.is_none(),
        "S1: overlapping four-day calendar record is still readable after successful delete: {response}"
    );
    Ok(())
}

#[tokio::test]
async fn s2_erase_does_not_leave_deleted_content_readable_by_agent_sql() -> Result<()> {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir()?;
    let store = Store::create(&dir.path().join("state")).await?;
    record(&store, at(), None).await?;
    let source = store.connect_source("Review", "Synthetic only", 1).await?;
    let mut policy = DisclosurePolicy::laptop();
    policy
        .source_grants
        .insert(source.connector_instance.clone());
    let broker = Broker::new(
        store.clone(),
        policy,
        SensitiveEgress::Full,
        hx_store::window_at(at()),
    );
    let table = format!("s_{}_notes", source.id);
    let setup = broker.clone().for_setup(source.id.clone());
    setup.call("agent_sql", json!({"source":source.id,"sql":format!("CREATE TABLE {table}(note TEXT)"),"params":[]})).await?;
    setup.call("agent_sql", json!({"source":source.id,"sql":format!("INSERT INTO {table} VALUES(?)"),"params":["private retreat"]})).await?;
    let (status, response) = erase(&store, false).await?;
    assert_eq!(status, StatusCode::OK, "{response}");
    let answer = broker
        .call(
            "agent_sql",
            json!({"source":source.id,"sql":format!("SELECT note FROM {table}"),"params":[]}),
        )
        .await?;
    assert!(
        !answer.to_string().contains("private retreat"),
        "S2: agent can still read deleted content: {answer}"
    );
    Ok(())
}

#[tokio::test]
async fn s3_erase_refuses_when_required_deletion_log_cannot_be_written() -> Result<()> {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir()?;
    let store = Store::create(&dir.path().join("state")).await?;
    let id = record(&store, at(), None).await?;
    // Deterministic log I/O failure; backup destination remains writable.
    std::fs::create_dir_all(dir.path().join("backups/deletions.jsonl"))?;
    let (status, response) = erase(&store, true).await?;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{response}");
    assert!(
        store.evidence(id).await?.is_some(),
        "S3: delete committed without its required log; API returned {status}: {response}"
    );
    Ok(())
}

#[tokio::test]
async fn n1_deleting_backup_does_not_hide_unfinished_erase() -> Result<()> {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir()?;
    let store = Store::create(&dir.path().join("state")).await?;
    let (backup, _) = hx_server::admin_api::take_backup(&store, "before-delete").await?;
    // Durable state left by a delete interrupted before its completion entry.
    std::fs::write(
        dir.path().join("backups/deletions.jsonl"),
        format!(
            "{}\n",
            json!({"at":at(), "action":"deleting", "start":at(),
            "end":at()+Duration::hours(1), "backup":backup})
        ),
    )?;
    let token = store
        .create_token(
            "review owner",
            vec![
                "source:".into(),
                "macos-agent:".into(),
                "macos-media:".into(),
                "companion:".into(),
            ],
            vec!["read_raw".into()],
        )
        .await?;
    let router = hx_server::router(store, DisclosurePolicy::default());
    let response = router
        .clone()
        .oneshot(
            Request::delete(format!("/v1/backups/{backup}"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = router
        .oneshot(
            Request::get("/v1/erase/log")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    assert!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "deleting"),
        "N1: deleting a backup hid the only unfinished erase record: {body}"
    );
    Ok(())
}
