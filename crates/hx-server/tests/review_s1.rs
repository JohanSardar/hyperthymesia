use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::get,
};
use hx_server::disclosure::DisclosurePolicy;
use hx_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

#[tokio::test]
async fn notion_catalog_configuration_can_connect() {
    let scratch = tempfile::tempdir().unwrap();
    let store = Store::create(scratch.path()).await.unwrap();
    let token = store
        .create_token("review", vec!["source:".into()], vec!["read_raw".into()])
        .await
        .unwrap();
    let app = hx_server::router(store, DisclosurePolicy::default());
    let catalog = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/connections/catalog")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let catalog: Value =
        serde_json::from_slice(&to_bytes(catalog.into_body(), usize::MAX).await.unwrap()).unwrap();
    let notion = catalog["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "notion")
        .unwrap();
    // Reuse the actual catalog entry, with only its destination replaced by a
    // loopback service. No third-party account or installed server is involved.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let probe = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/users/me",
                get(|| async { axum::Json(json!({"id":"synthetic-user"})) }),
            ),
        )
        .await
        .unwrap();
    });
    let mut connection = notion["connection"].clone();
    connection["base_url"] = json!(origin);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/connections")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"name":notion["name"], "description":notion["description"],
            "key":"synthetic-notion-token", "connection":connection})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    probe.abort();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
}
