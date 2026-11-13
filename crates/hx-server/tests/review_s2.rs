use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use hx_server::{disclosure::DisclosurePolicy, mcp::Broker, processing::SensitiveEgress};
use hx_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

#[tokio::test]
async fn custom_cli_setup_can_discover_other_reads() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let executable = scratch.path().join("activity-cli");
    std::fs::write(&executable, "#!/bin/sh\ncase \"$1\" in\nprobe) printf '{\"ok\":true}' ;;\nentries) printf '{\"items\":[]}' ;;\nesac\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::create(&scratch.path().join("state")).await.unwrap();
    let token = store
        .create_token("review", vec!["source:".into()], vec!["read_raw".into()])
        .await
        .unwrap();
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::post("/v1/connections")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"name":"My activity", "description":"Read recent activity as JSON.",
                "cli_home":scratch.path(), "connection":{"kind":"cli","argv":[executable,"probe"]}})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let connected: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let source = connected["id"].as_str().unwrap();
    let broker = Broker::new(
        store,
        DisclosurePolicy::default(),
        SensitiveEgress::Full,
        hx_store::window_at(chrono::Utc::now()),
    )
    .for_setup(source.to_owned());
    let result = broker
        .call(
            "live_lookup",
            json!({"source":source, "request":{
        "kind":"cli","argv":[executable,"entries"]}}),
        )
        .await
        .unwrap();
    assert!(
        result.get("error").is_none(),
        "setup must allow another read on the pinned executable: {result}"
    );
}
