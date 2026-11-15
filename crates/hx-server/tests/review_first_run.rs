use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use hx_server::disclosure::DisclosurePolicy;
use hx_store::{Store, Token};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn owner(store: &Store) -> String {
    store
        .create_token(
            "review",
            vec![
                "source:".into(),
                "macos-agent:".into(),
                "macos-media:".into(),
                "companion:".into(),
            ],
            vec!["read_raw".into(), "ingest".into()],
        )
        .await
        .unwrap()
}
async fn api(store: &Store, token: &str, method: &str, path: &str, body: Value) -> Value {
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[tokio::test]
async fn s1_proof_shows_current_recorder_screen_text() {
    use hx_schema::*;
    let scratch = tempfile::tempdir().unwrap();
    let store = Store::create(scratch.path()).await.unwrap();
    let token = owner(&store).await;
    let now = chrono::Utc::now();
    // Same namespace and payload shape as media.rs -> screen_text::merge.
    let envelope = Envelope {
        envelope_version: 1,
        observation_id: ObservationId::generate(),
        connector_instance: ConnectorInstance::new("macos-media:review"),
        source_key: SourceKey::new("screen-text:review"),
        revision: Revision::append_only(),
        operation: Operation::Observe,
        sequence: None,
        kind: Kind::new("text.snapshot"),
        schema_version: 1,
        time: ObservationTime::point(now),
        device: None,
        actor: None,
        subject: None,
        resource: None,
        links: vec![],
        coverage: None,
        sensitivity: Sensitivity::Restricted,
        payload: json!({"surface":"review.editor", "source":"ocr", "text":"Screen text (top to bottom):\nReview screen sentinel", "text_sha256":"a".repeat(64), "completeness":"partial", "app_name":"Review Editor", "window_title":"Review", "ocr":{"lines":[{"text":"Review screen sentinel"}]}}),
        blobs: vec![],
    };
    let ack = store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![envelope],
            },
            &Token {
                prefixes: vec!["macos-media:".into()],
                scopes: vec!["ingest".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(ack.accepted, 1);
    let proof = api(&store, &token, "GET", "/v1/setup/proof", Value::Null).await;
    assert_eq!(
        proof["items"][0]["lines"][0], "Review screen sentinel",
        "real recorder capture must appear: {proof}"
    );
}

// Run environment-sensitive checks in their own process, never reading the owner's HOME.
fn isolated(case: &str) -> bool {
    if std::env::var("HX_REVIEW_CASE").as_deref() == Ok(case) {
        return false;
    }
    let scratch = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", case, "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", scratch.path())
        .env("HX_REVIEW_CASE", case)
        .env("HX_FX_CREDENTIAL", scratch.path().join("absent-auth.json"))
        .env("HX_FX_BINARY", scratch.path().join("must-not-launch"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated check failed ({}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[tokio::test]
async fn s2_missing_login_offers_signin_fix() {
    if isolated("s2_missing_login_offers_signin_fix") {
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    let store = Store::create(scratch.path()).await.unwrap();
    let token = owner(&store).await;
    api(
        &store,
        &token,
        "PUT",
        "/v1/settings",
        json!({"model":{"provider":"codex", "model":"synthetic-model", "effort":null}}),
    )
    .await;
    let check = api(&store, &token, "POST", "/v1/model/check", Value::Null).await;
    assert_eq!(check["ok"], false);
    assert_eq!(
        check["problem"], "signin",
        "a removed login needs Sign in again, not waiting for recording: {check}"
    );
}

#[tokio::test]
async fn s3_spending_limit_blocks_new_paid_check() {
    if isolated("s3_spending_limit_blocks_new_paid_check") {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let binary = std::path::PathBuf::from(std::env::var_os("HX_FX_BINARY").unwrap());
    let marker = binary.with_extension("called");
    std::fs::write(&binary, format!("#!/bin/sh\ntouch '{}'\nprintf '%s' '{{\"exit_code\":0,\"model\":\"synthetic-model\",\"final_output\":\"OK\"}}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::create(scratch.path()).await.unwrap();
    let token = owner(&store).await;
    api(
        &store,
        &token,
        "PUT",
        "/v1/settings/gateway-key",
        json!({"key":"synthetic-gateway-key"}),
    )
    .await;
    api(&store, &token, "PUT", "/v1/settings", json!({"model":{"provider":"vercel", "model":"synthetic-model", "effort":null}, "spend_limit_usd":0})).await;
    let check = api(&store, &token, "POST", "/v1/model/check", Value::Null).await;
    assert!(
        !marker.exists(),
        "no new paid run may start at the monthly limit: {check}"
    );
}
