use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{TimeZone, Utc};
use hx_schema::{
    ConnectorInstance, Envelope, Kind, ObservationId, ObservationTime, Operation, Revision,
    Sensitivity, SourceKey,
};
use hx_server::disclosure::DisclosurePolicy;
use hx_store::{Store, Token, UploadBatch};
use serde_json::{Value, json};
use tower::ServiceExt;

fn event(connector: &str, key: &str, at: chrono::DateTime<Utc>) -> Envelope {
    Envelope {
        envelope_version: 1,
        observation_id: ObservationId::generate(),
        connector_instance: ConnectorInstance::new(connector),
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
        payload: json!({"url":"https://example.invalid", "title":"synthetic secretneedle"}),
        blobs: vec![],
    }
}

async fn seed(store: &Store, envelopes: Vec<Envelope>) {
    store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes,
            },
            &Token {
                prefixes: vec!["test:".into()],
                scopes: vec!["ingest".into()],
            },
        )
        .await
        .unwrap();
    for row in store
        .digest_records(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap(),
        )
        .await
        .unwrap()
    {
        store.normalize(row.id).await.unwrap();
    }
}

async fn get(store: &Store, token: &str, uri: &str) -> Value {
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::get(uri)
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn s1_new_reads_reject_sibling_namespace_prefixes() {
    let scratch = tempfile::tempdir().unwrap();
    let store = Store::create(scratch.path()).await.unwrap();
    seed(
        &store,
        vec![event(
            "test:mac2",
            "foreign",
            Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
        )],
    )
    .await;
    let reader = Token {
        prefixes: vec!["test:mac".into()],
        scopes: vec!["read_raw".into()],
    };
    assert!(!reader.allows("test:mac2"));
    let token = store
        .create_token("review", reader.prefixes, reader.scopes)
        .await
        .unwrap();
    let find = get(&store, &token, "/v1/find?q=secretneedle").await;
    let days = get(&store, &token, "/v1/days").await;
    assert!(
        find["records"].as_array().unwrap().is_empty()
            && days["buckets"].as_array().unwrap().is_empty(),
        "token test:mac must not expose test:mac2: find={find}; days={days}"
    );
}

#[tokio::test]
async fn s2_day_index_matches_past_local_day_across_dst() {
    let scratch = tempfile::tempdir().unwrap();
    let store = Store::create(scratch.path()).await.unwrap();
    // America/New_York, July 1 at 00:30 EDT. The server must not bake one
    // current offset into past days; it returns exact UTC buckets and the
    // browser groups them with its own time zone rules.
    seed(
        &store,
        vec![event(
            "test:mac",
            "summer",
            Utc.with_ymd_and_hms(2026, 7, 1, 4, 30, 0).unwrap(),
        )],
    )
    .await;
    let token = store
        .create_token("review", vec!["test:mac".into()], vec!["read_raw".into()])
        .await
        .unwrap();
    let days = get(&store, &token, "/v1/days").await;
    assert_eq!(
        days["buckets"][0]["at"],
        Utc.with_ymd_and_hms(2026, 7, 1, 4, 30, 0)
            .unwrap()
            .timestamp(),
        "buckets must carry the exact UTC time, not a day made with one offset"
    );
}
