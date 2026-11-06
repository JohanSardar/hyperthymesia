//! Public broker boundary; all stores and sources are synthetic.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use hx_server::{
    disclosure::DisclosurePolicy,
    mcp::{Broker, HttpState, router},
    processing::SensitiveEgress,
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn rpc(app: &Router, request: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("authorization", "Bearer synthetic")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}
fn app(broker: Broker) -> Router {
    router(HttpState {
        broker,
        secret: "synthetic".into(),
    })
}
fn call(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}})
}
fn answer(response: &Value) -> Value {
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
fn names(response: &Value) -> Vec<&str> {
    response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn broker_modes_enforce_tool_source_and_argument_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let store = hx_store::Store::create(dir.path()).await.unwrap();
    let source = store
        .connect_source("Synthetic", "Synthetic only", 1)
        .await
        .unwrap();
    let other = store
        .connect_source("Other", "Synthetic only", 1)
        .await
        .unwrap();
    let mut policy = DisclosurePolicy::laptop();
    policy.source_grants.extend([
        source.connector_instance.clone(),
        other.connector_instance.clone(),
    ]);
    let window = hx_store::window_at(chrono::Utc::now());
    let broker = Broker::new(store.clone(), policy, SensitiveEgress::Full, window);
    // Reopen a store with history, including snapshots owned by a different run.
    let historical = store
        .save_live_snapshot(
            ulid::Ulid::generate(),
            &source.id,
            json!({"old":"synthetic history"}),
        )
        .await
        .unwrap();
    let store = hx_store::Store::open(dir.path()).await.unwrap();
    let generation = store.generation(window).await.unwrap();
    let jobs = store.record_count("jobs").await.unwrap();
    let window_app = app(broker.clone());
    let setup = broker.clone().for_setup(source.id.clone());
    assert_eq!(setup.live_limit, 40);
    let setup_app = app(setup.clone());
    let list = json!({"id":1,"method":"tools/list"});
    let (_, response) = rpc(&window_app, list.clone()).await;
    assert_eq!(
        names(&response),
        [
            "window_overview",
            "timeline",
            "search",
            "read_evidence",
            "screen_at",
            "list_sources",
            "prior_context",
            "live_lookup",
            "source_bash",
            "agent_sql"
        ]
    );
    let (_, response) = rpc(&setup_app, list).await;
    assert_eq!(
        names(&response),
        [
            "source_info",
            "live_lookup",
            "source_bash",
            "recipe_test",
            "recipe_save",
            "addon_save",
            "agent_sql"
        ]
    );

    // The HTTP body limit must allow setup arguments over the old 8 KB limit,
    // while each mode independently caps the serialized arguments.
    let command = json!({"source":source.id,"command":"x".repeat(9000)});
    let (status, response) = rpc(&setup_app, call("source_bash", command.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer(&response)["error"], "source_bash not available yet");
    let (_, response) = rpc(&window_app, call("source_bash", command)).await;
    assert!(
        answer(&response)["error"]
            .as_str()
            .unwrap()
            .contains("4096")
    );
    let (_, response) = rpc(
        &setup_app,
        call(
            "source_bash",
            json!({"source":source.id,"command":"x".repeat(17 * 1024)}),
        ),
    )
    .await;
    assert!(
        answer(&response)["error"]
            .as_str()
            .unwrap()
            .contains("16384")
    );
    let (status, _) = rpc(
        &setup_app,
        json!({"id":1,"method":"ping","padding":"x".repeat(31 * 1024)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = rpc(
        &setup_app,
        json!({"id":1,"method":"ping","padding":"x".repeat(33 * 1024)}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    for name in [
        "window_overview",
        "timeline",
        "search",
        "read_evidence",
        "screen_at",
        "list_sources",
        "prior_context",
    ] {
        let (_, response) = rpc(&setup_app, call(name, json!({"id":historical}))).await;
        assert_eq!(
            answer(&response)["error"],
            "tool not available in this run mode",
            "{name}"
        );
    }
    for args in [
        json!({"source":other.id}),
        json!({"source":"synthetic"}),
        json!({"source":source.id.to_uppercase()}),
    ] {
        let (_, response) = rpc(&setup_app, call("source_info", args)).await;
        assert!(response["result"]["isError"].as_bool().unwrap());
        assert_eq!(answer(&response)["retryable"], false);
        assert!(
            !answer(&response)["error"]
                .as_str()
                .unwrap()
                .contains("not available yet")
        );
    }
    let token = broker.run_token();
    let (_, response) = rpc(
        &window_app,
        call(
            "live_lookup",
            json!({"source":source.id,"request":{"kind":"http"},"run_token":token}),
        ),
    )
    .await;
    assert_eq!(answer(&response)["error"], "unexpected argument: run_token");
    assert_eq!(broker.run_token(), token);
    let (_, response) = rpc(&window_app, call("read_evidence", json!({"id":historical}))).await;
    assert_eq!(answer(&response)["error"], "evidence unavailable");
    let (_, response) = rpc(&window_app, call("recipe_save", json!({}))).await;
    assert_eq!(
        answer(&response)["error"],
        "tool not available in this run mode"
    );
    for timeout in [json!(0), json!(181), json!("180")] {
        let (_, response) = rpc(
            &window_app,
            call(
                "live_lookup",
                json!({"source":source.id,"request":{},"timeout_s":timeout}),
            ),
        )
        .await;
        assert_eq!(answer(&response)["retryable"], false);
        assert!(
            answer(&response)["error"]
                .as_str()
                .unwrap()
                .contains("timeout_s")
        );
    }
    // Failed reads that were never authorized must not taint a window's
    // manifest and cause a later disclosure guard to defer that window.
    let unknown = ulid::Ulid::generate().to_string().to_lowercase();
    let (_, response) = rpc(
        &window_app,
        call("live_lookup", json!({"source":unknown,"request":{}})),
    )
    .await;
    assert_eq!(answer(&response)["error"], "unknown source");
    assert!(
        !broker
            .audit
            .lock()
            .await
            .input_namespaces
            .contains(&format!("source:{unknown}"))
    );
    store.set_source_enabled(&other.id, false).await.unwrap();
    let (_, response) = rpc(
        &window_app,
        call("live_lookup", json!({"source":other.id,"request":{}})),
    )
    .await;
    assert_eq!(answer(&response)["error"], "source is off");
    assert!(
        !broker
            .audit
            .lock()
            .await
            .input_namespaces
            .contains(&other.connector_instance)
    );
    store.set_source_enabled(&source.id, false).await.unwrap();
    let (_, response) = rpc(&setup_app, call("source_info", json!({"source":source.id}))).await;
    assert_eq!(answer(&response)["error"], "source is off");
    assert_eq!(store.generation(window).await.unwrap(), generation);
    assert_eq!(store.record_count("jobs").await.unwrap(), jobs);
    assert!(store.evidence(historical).await.unwrap().is_some());
    assert!(broker.audit.lock().await.evidence.is_empty());
    assert!(setup.audit.lock().await.evidence.is_empty());
    assert!(!broker.audit.lock().await.exhausted);
}
