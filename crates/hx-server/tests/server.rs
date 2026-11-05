use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::{Duration, TimeZone, Utc};
use http_body_util::BodyExt;
use hx_schema::{
    ConnectorInstance, Envelope, Kind, ObservationId, ObservationTime, Operation, Revision,
    Sensitivity, SourceKey,
};
use hx_server::disclosure::DisclosurePolicy;
use hx_store::test_support::Fixture;
use hx_store::{Store, Token, UploadBatch};
use serde_json::{Value, json};
use tower::ServiceExt;

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
async fn request(
    store: &Store,
    method: &str,
    path: &str,
    secret: &str,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    request_with_policy(
        store,
        DisclosurePolicy::default(),
        method,
        path,
        secret,
        body,
    )
    .await
}
async fn request_with_policy(
    store: &Store,
    policy: DisclosurePolicy,
    method: &str,
    path: &str,
    secret: &str,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let response = hx_server::router(store.clone(), policy)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {secret}"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}
async fn secret(store: &Store) -> String {
    store
        .create_token("synthetic", token().prefixes, token().scopes)
        .await
        .unwrap()
}

#[tokio::test]
async fn duplicate_batch_is_counted_and_transactional() {
    let f = Fixture::new().await;
    let e = event("one", 8);
    assert_eq!(
        ingest(&f.store, vec![e.clone(), e.clone()]).await.accepted,
        1
    );
    assert_eq!(ingest(&f.store, vec![e]).await.duplicates, 1);
    assert_eq!(count(&f.store, "events").await, 1);
    assert_eq!(count(&f.store, "jobs").await, 2);
    f.close().await;
}
#[tokio::test]
async fn reordered_and_late_data_dirty_all_touched_windows() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("later", 9)]).await;
    f.store.clear_window_dirty_flags().await.unwrap();
    let mut e = event("late", 2);
    e.time.source_end = Some(e.time.observed_at + Duration::hours(8));
    ingest(&f.store, vec![e]).await;
    let rows = f.store.ordered_window_flags().await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.get::<bool, _>("dirty")));
    assert_eq!(rows[2].get::<i64, _>("generation"), 2);
    let watermark: chrono::DateTime<Utc> = f.store.observed_watermark().await.unwrap();
    assert_eq!(watermark.hour(), 9);
    f.close().await;
}
use chrono::Timelike;
#[tokio::test]
async fn bad_core_is_quarantined_without_losing_l0() {
    let f = Fixture::new().await;
    let mut e = event("bad", 8);
    e.payload = json!({"title":"missing required url"});
    ingest(&f.store, vec![e]).await;
    let id: i64 = f.store.only_event_id().await.unwrap();
    f.store.normalize(id).await.unwrap();
    f.store.normalize(id).await.unwrap();
    assert_eq!(count(&f.store, "events").await, 1);
    assert_eq!(count(&f.store, "quarantine").await, 1);
    assert_eq!(count(&f.store, "normalized").await, 0);
    f.close().await;
}
#[tokio::test]
async fn api_rejects_foreign_invalid_and_malformed_envelopes_individually() {
    let f = Fixture::new().await;
    let key = secret(&f.store).await;
    let mut foreign = event("foreign", 8);
    foreign.connector_instance = ConnectorInstance::new("other:device");
    let mut invalid = event("invalid", 8);
    invalid.envelope_version = 9;
    let (status, body) = request(
        &f.store,
        "POST",
        "/v1/events",
        &key,
        serde_json::to_vec(
            &json!({"batch_id":ulid::Ulid::generate(),"envelopes":[event("valid",8),foreign,invalid,{"observation_id":ObservationId::generate(),"broken":true}]}),
        )
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ack: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(ack["accepted"], 1);
    assert_eq!(ack["rejected"].as_array().unwrap().len(), 3);
    assert_eq!(count(&f.store, "events").await, 1);
    let (status, _) = request(&f.store, "POST", "/v1/events", "invalid", b"{}".to_vec()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    f.close().await;
}
#[tokio::test]
async fn blob_api_is_idempotent_and_hash_checked() {
    let f = Fixture::new().await;
    let key = secret(&f.store).await;
    let bytes = b"synthetic media";
    let sha = hx_store::hash(bytes);
    let path = format!("/v1/blobs/{sha}");
    assert_eq!(
        request(&f.store, "PUT", &path, &key, bytes.to_vec())
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        request(&f.store, "PUT", &path, &key, bytes.to_vec())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&f.store, "PUT", &path, &key, b"wrong".to_vec())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        tokio::fs::read(f.store.blob_path(&sha).unwrap())
            .await
            .unwrap(),
        bytes
    );
    f.close().await;
}
#[tokio::test]
async fn retention_preserves_pins_and_stills_and_marks_expiration() {
    let f = Fixture::new().await;
    let sha = hx_store::hash(b"video");
    let still = hx_store::hash(b"still");
    f.store.upload_blob(&sha, b"video").await.unwrap();
    f.store.upload_blob(&still, b"still").await.unwrap();
    let mut e = event("chunk", 8);
    e.kind = Kind::new("media.chunk");
    e.blobs = vec![hx_schema::BlobRef {
        sha256: sha.clone(),
        media_type: "video/mp4".into(),
        bytes: 5,
    }];
    ingest(&f.store, vec![e]).await;
    f.store.age_blobs_for_retention(&still).await.unwrap();
    f.store.pin(&sha, true).await.unwrap();
    assert_eq!(f.store.gc().await.unwrap(), 0);
    f.store.pin(&sha, false).await.unwrap();
    assert_eq!(f.store.gc().await.unwrap(), 1);
    assert!(f.store.blob_path(&still).unwrap().exists());
    assert!(!f.store.blob_path(&sha).unwrap().exists());
    let id: i64 = f.store.only_event_id().await.unwrap();
    assert_eq!(
        f.store.evidence(id).await.unwrap().unwrap()["blobs"][0]["evidence_expired"],
        true
    );
    assert_eq!(count(&f.store, "events").await, 1);
    f.close().await;
}
#[tokio::test]
async fn stale_revision_never_replaces_canonical_view() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("first", 8)]).await;
    let w = hx_store::window_at(event("first", 8).time.observed_at);
    let generation = f.store.generation(w).await.unwrap();
    let (first, published) = f
        .store
        .publish_revision(w, generation, json!({"summary":"first"}))
        .await
        .unwrap();
    assert!(published);
    ingest(&f.store, vec![event("late", 9)]).await;
    let (_, published) = f
        .store
        .publish_revision(w, generation, json!({"summary":"stale"}))
        .await
        .unwrap();
    assert!(!published);
    let canonical: i64 = f.store.served_revision().await.unwrap();
    assert_eq!(canonical, first);
    assert_eq!(count(&f.store, "revisions").await, 2);
    f.close().await;
}
#[tokio::test]
async fn search_filters_restricted_sensitive_and_foreign_namespaces() {
    let f = Fixture::new().await;
    let mut restricted = event("restricted", 8);
    restricted.sensitivity = Sensitivity::Restricted;
    let mut sensitive = event("sensitive", 8);
    sensitive.sensitivity = Sensitivity::Sensitive;
    let mut foreign = event("foreign", 8);
    foreign.connector_instance = ConnectorInstance::new("other:device");
    let mut all = token();
    all.prefixes.push("other:".into());
    f.store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![event("normal", 8), restricted, sensitive, foreign],
            },
            &all,
        )
        .await
        .unwrap();
    let ids: Vec<i64> = f.store.event_ids().await.unwrap();
    for id in ids {
        f.store.normalize(id).await.unwrap();
    }
    assert_eq!(
        f.store
            .search("nebula", false, &token().prefixes)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.store
            .search("nebula", true, &token().prefixes)
            .await
            .unwrap()
            .len(),
        3
    );
    let key = f
        .store
        .create_token("reader", token().prefixes, vec!["read_l3".into()])
        .await
        .unwrap();
    let (status, body) = request(&f.store, "GET", "/v1/search?q=nebula", &key, vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.close().await;
}
#[test]
fn dashboard_lists_episodes_in_the_owners_local_time() {
    let w = hx_schema::Interval {
        start: Utc.with_ymd_and_hms(2026, 1, 1, 6, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 1, 2, 6, 0, 0).unwrap(),
    };
    let timeline = hx_compile::Timeline {
        window: w,
        compiler_version: "test".into(),
        lanes: vec![],
        points: vec![],
        coverage: vec![],
        aggregates: vec![],
    };
    let episode = |id: &str, h: u32, title: &str| {
        json!({"id":id,"title":title,"description":"Wrote <b>notes</b>","evidence":[7],
            "intervals":[{"start":Utc.with_ymd_and_hms(2026,1,1,h,0,0).unwrap(),"end":Utc.with_ymd_and_hms(2026,1,1,h,30,0).unwrap()}]})
    };
    let revision = json!({"revision_id":1,"has_agent_run":false,"interpretation":{
        "window":{"start":"2026-01-01T16:00:00Z","end":"2026-01-01T20:00:00Z"},
        "summary":"Drafted notes.","tasks":[],
        "episodes":[episode("b",18,"Second"),episode("a",16,"First"),episode("old",2,"Before the day")]}});
    let zone = chrono::FixedOffset::west_opt(6 * 3600).unwrap();
    let mut timeline = timeline;
    // A span in the day's last instant must render rather than panic the worker.
    timeline.lanes.push(hx_compile::Lane {
        id: hx_compile::LaneId {
            key: "device:test".into(),
            kind: "focus".into(),
        },
        intervals: [
            (0, 60, "Merged"),
            (60, 120, "Merged"),
            (86_399, 86_400, "Late"),
        ]
        .into_iter()
        .map(|(a, b, label)| hx_compile::LaneInterval {
            interval: hx_schema::Interval {
                start: w.start + chrono::Duration::seconds(a),
                end: w.start + chrono::Duration::seconds(b),
            },
            label: label.into(),
            attrs: json!({}),
            evidence: vec![1],
            uncertain_edges: false,
        })
        .collect(),
    });
    let html = hx_server::dashboard::render_day(&timeline, &[revision], &[], zone).0;
    assert_eq!(html.matches("title=\"Merged\"").count(), 1);
    assert_eq!(html.matches("title=\"Late\"").count(), 1);
    assert!(html.contains("Thursday, January 1, 2026"), "{html}");
    assert!(html.contains("UTC−06:00"));
    assert!(html.contains("10:00–14:00"));
    let first = html.find("10:00–10:30").unwrap();
    assert!(first < html.find("12:00–12:30").unwrap());
    assert!(html.contains(">First</a>") && !html.contains(">Before the day<"));
    assert!(html.contains("Wrote &lt;b&gt;notes&lt;/b&gt;"));
}
#[test]
fn dashboard_escapes_malicious_title() {
    let w = hx_store::window_at(event("x", 8).time.observed_at);
    let title = "<script>alert('captured')</script>";
    let timeline = hx_compile::Timeline {
        window: w,
        compiler_version: "test".into(),
        lanes: vec![hx_compile::Lane {
            id: hx_compile::LaneId {
                key: "device:test".into(),
                kind: "focus".into(),
            },
            intervals: vec![hx_compile::LaneInterval {
                interval: w,
                label: title.into(),
                attrs: json!({}),
                evidence: vec![1],
                uncertain_edges: false,
            }],
        }],
        points: vec![],
        coverage: vec![],
        aggregates: vec![],
    };
    let html = hx_server::dashboard::render_day(
        &timeline,
        &[],
        &[],
        chrono::FixedOffset::east_opt(0).unwrap(),
    )
    .0;
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("https://"));
}
#[tokio::test]
async fn leases_fence_expired_workers_and_limit_attempts() {
    let f = Fixture::new().await;
    let mut tx = f.store.transaction().await.unwrap();
    tx.enqueue(
        "one",
        "test",
        json!({}),
        true,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.enqueue(
        "one",
        "test",
        json!({}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let first = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert!(hx_jobs::claim(&f.store, 120).await.unwrap().is_none());
    f.store.expire_all_leases().await.unwrap();
    let next = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(first.id, next.id);
    assert!(!hx_jobs::finish(&f.store, &first, None).await.unwrap());
    assert!(hx_jobs::finish(&f.store, &next, None).await.unwrap());
    assert!(hx_jobs::claim(&f.store, 120).await.unwrap().is_none());
    let mut tx = f.store.transaction().await.unwrap();
    tx.enqueue(
        "two",
        "test",
        json!({}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    f.store.limit_second_job_attempts().await.unwrap();
    let job = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    hx_jobs::finish(&f.store, &job, Some("synthetic failure"))
        .await
        .unwrap();
    let state: String = f.store.second_job_state().await.unwrap();
    assert_eq!(state, "failed");
    f.close().await;
}

#[tokio::test]
async fn revision_retries_are_idempotent_and_records_are_immutable() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("first", 8)]).await;
    let w = hx_store::window_at(event("first", 8).time.observed_at);
    let generation = f.store.generation(w).await.unwrap();
    let first = f
        .store
        .publish_revision_key(
            w,
            generation,
            json!({"summary":"original"}),
            "synthetic-output",
        )
        .await
        .unwrap();
    let second = f
        .store
        .publish_revision_key(
            w,
            generation,
            json!({"summary":"changed"}),
            "synthetic-output",
        )
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(count(&f.store, "revisions").await, 1);
    let value: Value = f.store.stored_interpretation().await.unwrap();
    assert_eq!(value["summary"], "original");
    assert!(f.store.try_mutate_revision().await.is_err());
    assert!(f.store.try_delete_events().await.is_err());
    f.close().await;
}
#[tokio::test]
async fn compile_waits_for_normalization_then_saves_current_generation() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("compile", 8)]).await;
    let w = hx_store::window_at(event("compile", 8).time.observed_at);
    assert!(
        hx_server::processing::compile_window(&f.store, w)
            .await
            .is_err()
    );
    let id: i64 = f.store.only_event_id().await.unwrap();
    f.store.normalize(id).await.unwrap();
    let (timeline, generation) = hx_server::processing::compile_window(&f.store, w)
        .await
        .unwrap();
    let row = f.store.compiled_window().await.unwrap();
    assert_eq!(
        row.get::<Value, _>("timeline"),
        serde_json::to_value(&timeline).unwrap()
    );
    assert_eq!(row.get::<i64, _>("compiled_generation"), generation);
    assert!(!row.get::<bool, _>("dirty"));
    ingest(&f.store, vec![event("late", 9)]).await;
    assert!(!f.store.save_timeline(&timeline, generation).await.unwrap());
    f.close().await;
}
#[tokio::test]
async fn entity_assertions_require_real_evidence_and_inherit_restrictions() {
    let f = Fixture::new().await;
    let mut e = event("entity", 8);
    e.sensitivity = Sensitivity::Restricted;
    ingest(&f.store, vec![e]).await;
    let id: i64 = f.store.only_event_id().await.unwrap();
    assert!(
        f.store
            .assert_entity("project", json!({}), &[])
            .await
            .is_err()
    );
    assert!(
        f.store
            .assert_entity("project", json!({}), &[id + 100])
            .await
            .is_err()
    );
    f.store
        .assert_entity("project", json!({"name":"synthetic"}), &[id])
        .await
        .unwrap();
    let key = f
        .store
        .create_token("reader", token().prefixes, vec!["read_l3".into()])
        .await
        .unwrap();
    let (_, body) = request(&f.store, "GET", "/v1/entities", &key, vec![]).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["entities"],
        json!([])
    );
    let key = secret(&f.store).await;
    let (_, body) = request(&f.store, "GET", "/v1/entities", &key, vec![]).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["entities"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.close().await;
}
#[tokio::test]
async fn raw_evidence_and_blob_reads_are_namespace_scoped() {
    let f = Fixture::new().await;
    let bytes = b"synthetic still";
    let sha = hx_store::hash(bytes);
    f.store.upload_blob(&sha, bytes).await.unwrap();
    let mut e = event("evidence", 8);
    e.blobs.push(hx_schema::BlobRef {
        sha256: sha.clone(),
        media_type: "application/octet-stream".into(),
        bytes: bytes.len() as u64,
    });
    ingest(&f.store, vec![e]).await;
    let id: i64 = f.store.only_event_id().await.unwrap();
    let key = secret(&f.store).await;
    assert_eq!(
        request(&f.store, "GET", &format!("/v1/evidence/{id}"), &key, vec![])
            .await
            .0,
        StatusCode::OK
    );
    let (status, body) = request(&f.store, "GET", &format!("/v1/blobs/{sha}"), &key, vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, bytes);
    let other = f
        .store
        .create_token("other", vec!["foreign:".into()], token().scopes)
        .await
        .unwrap();
    assert_eq!(
        request(
            &f.store,
            "GET",
            &format!("/v1/evidence/{id}"),
            &other,
            vec![]
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&f.store, "GET", &format!("/v1/blobs/{sha}"), &other, vec![])
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    f.close().await;
}
#[tokio::test]
async fn compressed_upload_preserves_batch_identity_and_tokens_can_be_revoked() {
    let f = Fixture::new().await;
    let key = secret(&f.store).await;
    let batch = UploadBatch {
        batch_id: ulid::Ulid::generate(),
        envelopes: vec![event("compressed", 8)],
    };
    let data = zstd::stream::encode_all(serde_json::to_vec(&batch).unwrap().as_slice(), 1).unwrap();
    let response = hx_server::router(f.store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/events")
                .header("authorization", format!("Bearer {key}"))
                .header("content-encoding", "zstd")
                .body(Body::from(data))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ack: hx_schema::UploadAck =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(ack.batch_id, batch.batch_id);
    assert_eq!(ack.accepted, 1);
    let stored: String = f.store.stored_token_hash().await.unwrap();
    assert_ne!(stored, key);
    f.store.revoke_tokens().await.unwrap();
    assert!(f.store.authenticate(&key).await.unwrap().is_none());
    f.close().await;
}
#[tokio::test]
async fn browser_signin_sets_a_private_cookie_and_renders_day() {
    let f = Fixture::new().await;
    let key = secret(&f.store).await;
    let response = hx_server::router(f.store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("token={key}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    let response = hx_server::router(f.store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::builder()
                .uri("/day/2026-01-01")
                .header("cookie", cookie.split(';').next().unwrap())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("24:00"));
    assert!(body.contains("Content-Security-Policy"));
    f.close().await;
}
#[tokio::test]
async fn late_overlap_data_invalidates_the_next_window() {
    let f = Fixture::new().await;
    let mut e = event("edge", 7);
    e.time.source_start = Some(e.time.observed_at + Duration::minutes(45));
    ingest(&f.store, vec![e]).await;
    assert_eq!(count(&f.store, "events").await, 1);
    let windows: i64 = f.store.window_count().await.unwrap();
    assert_eq!(windows, 2);
    f.close().await;
}
#[tokio::test]
async fn queue_prefers_interactive_and_enqueue_rolls_back() {
    let f = Fixture::new().await;
    let mut tx = f.store.transaction().await.unwrap();
    tx.enqueue(
        "rollback",
        "test",
        json!({}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(count(&f.store, "jobs").await, 0);
    let mut tx = f.store.transaction().await.unwrap();
    tx.enqueue(
        "backfill",
        "test",
        json!({"queue":"backfill"}),
        true,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.enqueue(
        "interactive",
        "test",
        json!({"queue":"interactive"}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let job = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(job.payload["queue"], "interactive");
    f.close().await;
}

#[tokio::test]
async fn canonical_revision_checks_transitive_namespace_lineage() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(event("lineage", 8).time.observed_at);
    let generation = f.store.generation(w).await.unwrap();
    f.store
        .publish_revision_manifest(
            w,
            generation,
            json!({"tasks":[],"summary":"synthetic cross-source summary"}),
            "lineage",
            &["test:device".into(), "other:device".into()],
        )
        .await
        .unwrap();
    let partial = hx_server::timeline_value(
        &f.store,
        &token(),
        &DisclosurePolicy::default(),
        w,
        "episodes",
    )
    .await
    .unwrap();
    assert_eq!(partial["revisions"], json!([]));
    let mut complete = token();
    complete.prefixes.push("other:".into());
    let full = hx_server::timeline_value(
        &f.store,
        &complete,
        &DisclosurePolicy::default(),
        w,
        "episodes",
    )
    .await
    .unwrap();
    assert_eq!(full["revisions"].as_array().unwrap().len(), 1);
    f.close().await;
}
#[tokio::test]
async fn api_withholds_canonical_revisions_citing_restricted_evidence() {
    let f = Fixture::new().await;
    let at = (Utc::now() - Duration::minutes(1))
        .with_nanosecond(0)
        .unwrap();
    let w = hx_store::window_at(at);
    let mut core = event("disclosure-core", 8);
    core.time = ObservationTime::point(at);
    let mut unknown = core.clone();
    unknown.observation_id = ObservationId::generate();
    unknown.source_key = SourceKey::new("disclosure-unknown");
    unknown.kind = Kind::new("extension.note");
    unknown.payload = json!({"text":"synthetic withheld note"});
    ingest(&f.store, vec![core, unknown]).await;
    f.store.normalize_window(w).await.unwrap();
    let core_id: i64 = f.store.core_event_id().await.unwrap();
    let unknown_id: i64 = f.store.extension_event_id().await.unwrap();
    let reader = f
        .store
        .create_token("l3-reader", token().prefixes, vec!["read_l3".into()])
        .await
        .unwrap();
    let raw_reader = f
        .store
        .create_token("raw-reader", token().prefixes, vec!["read_raw".into()])
        .await
        .unwrap();
    let timeline = format!(
        "/v1/timeline?from={}&to={}&level=episodes",
        w.start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        w.end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );
    let grant = DisclosurePolicy::from_json(
        r#"{"rules":[{"connector_prefix":"test:","kind":"extension.note","class":"normal"}]}"#,
    )
    .unwrap();
    let restrict = DisclosurePolicy::from_json(
        r#"{"rules":[{"connector_prefix":"test:","kind":"web.visit","class":"restricted"}]}"#,
    )
    .unwrap();
    for (id, withheld, policy) in [
        (core_id, false, DisclosurePolicy::default()),
        (unknown_id, true, DisclosurePolicy::default()),
        (unknown_id, false, grant),
        (core_id, true, restrict),
    ] {
        let interpretation = json!({
            "episodes":[{"evidence":[core_id],"claims":[{"outcome_evidence":[id]}]}],
            "tasks":[],"summary":"synthetic stored interpretation"
        });
        let generation = f.store.generation(w).await.unwrap();
        let (revision, published) = f
            .store
            .publish_revision(w, generation, interpretation.clone())
            .await
            .unwrap();
        assert!(published);
        for path in [timeline.as_str(), "/v1/now"] {
            for (key, hidden) in [(&reader, withheld), (&raw_reader, false)] {
                let (status, body) =
                    request_with_policy(&f.store, policy.clone(), "GET", path, key, vec![]).await;
                assert_eq!(status, StatusCode::OK);
                let value: Value = serde_json::from_slice(&body).unwrap();
                assert!(value["coverage"].is_array());
                assert_eq!(value["observed_through"], json!(at));
                if hidden {
                    assert_eq!(value["revisions"], json!([]), "{path}");
                    assert!(value["interpreted_through"].is_null(), "{path}");
                } else {
                    assert_eq!(value["revisions"].as_array().unwrap().len(), 1, "{path}");
                    assert_eq!(value["revisions"][0]["revision_id"], revision);
                    assert_eq!(value["revisions"][0]["interpretation"], interpretation);
                    assert_eq!(value["interpreted_through"], json!(w.end));
                }
            }
        }
    }
    f.close().await;
}

#[tokio::test]
async fn context_changes_invalidate_dependent_generations() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("early", 4), event("later", 8)]).await;
    let early = hx_store::window_at(event("early", 4).time.observed_at);
    let later = hx_store::window_at(event("later", 8).time.observed_at);
    let generation = f.store.generation(early).await.unwrap();
    let before = f.store.generation(later).await.unwrap();
    f.store
        .publish_revision_key(
            early,
            generation,
            json!({"tasks":[{"id":"synthetic-task","open":true}]}),
            "context",
        )
        .await
        .unwrap();
    assert_eq!(f.store.generation(later).await.unwrap(), before + 1);
    f.store
        .publish_revision_key(early, generation, json!({"tasks":[]}), "context")
        .await
        .unwrap();
    assert_eq!(f.store.generation(later).await.unwrap(), before + 1);
    let id: i64 = f.store.first_event_id().await.unwrap();
    f.store
        .assert_entity("project", json!({"name":"synthetic"}), &[id])
        .await
        .unwrap();
    assert_eq!(f.store.generation(later).await.unwrap(), before + 1);
    assert_eq!(f.store.generation(early).await.unwrap(), generation + 1);
    f.close().await;
}
#[tokio::test]
async fn runner_normalizes_then_schedules_cadence_and_day_revision() {
    let f = Fixture::new().await;
    // Cadence admission applies inside the first 24 hours, not to old backlog.
    let w = hx_store::window_at(Utc::now() - Duration::hours(4));
    let mut e = event("runner", 8);
    e.time = ObservationTime::point(w.start);
    ingest(&f.store, vec![e]).await;
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert_eq!(count(&f.store, "normalized").await, 1);
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    let rows = f.store.interpretation_queues().await.unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(
        |r| r.get::<String, _>("output_key").starts_with("revision24:")
            && r.get::<String, _>("queue") == "backfill"
    ));
    f.close().await;
}
#[tokio::test]
async fn unsupported_core_versions_quarantine_and_unknown_kinds_stay_restricted() {
    let f = Fixture::new().await;
    let mut version = event("version", 8);
    version.schema_version = 99;
    let mut unknown = event("unknown", 8);
    unknown.kind = Kind::new("extension.note");
    ingest(&f.store, vec![version, unknown]).await;
    let rows = f.store.event_classes().await.unwrap();
    for row in rows {
        f.store.normalize(row.get("id")).await.unwrap();
        if row.get::<String, _>("kind") == "extension.note" {
            assert_eq!(row.get::<String, _>("sensitivity"), "restricted");
        }
    }
    assert_eq!(count(&f.store, "quarantine").await, 1);
    assert_eq!(count(&f.store, "normalized").await, 1);
    f.close().await;
}
#[test]
fn cli_exposes_all_required_commands() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for command in ["migrate", "serve", "token", "compile", "interpret", "gc"] {
        assert!(text.contains(command));
    }
}

#[tokio::test]
async fn interpretation_packet_redacts_private_evidence_and_tracks_inputs() {
    let f = Fixture::new().await;
    let mut restricted = event("private", 8);
    restricted.sensitivity = Sensitivity::Restricted;
    restricted.payload["title"] = json!("PRIVATE_SENTINEL");
    let mut own = event("dashboard", 8);
    own.payload["title"] = json!("Hyperthymesia day");
    ingest(&f.store, vec![event("public", 8), restricted, own]).await;
    let ids: Vec<i64> = f.store.ordered_event_ids().await.unwrap();
    for id in &ids {
        f.store.normalize(*id).await.unwrap();
    }
    f.store
        .assert_entity(
            "private",
            json!({"name":"ENTITY_PRIVATE_SENTINEL"}),
            &[ids[1]],
        )
        .await
        .unwrap();
    let w = hx_store::window_at(event("packet", 8).time.observed_at);
    let input = hx_server::processing::prepare_input(&f.store, w, "stub")
        .await
        .unwrap();
    assert_eq!(input.packet.evidence.len(), 2);
    assert!(input.packet.evidence.iter().any(|e| e.self_generated));
    assert!(
        !serde_json::to_string(&input.packet)
            .unwrap()
            .contains("PRIVATE_SENTINEL")
    );
    assert_eq!(
        input.packet.input_digest,
        hx_store::hash(&serde_json::to_vec(&input.manifest).unwrap())
    );
    assert_eq!(input.namespaces, vec!["test:device"]);
    f.close().await;
}
#[tokio::test]
async fn canonical_pointer_never_moves_backwards_on_retry() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(event("ordering", 8).time.observed_at);
    let generation = f.store.generation(w).await.unwrap();
    let first = f
        .store
        .publish_revision_key(w, generation, json!({"tasks":[]}), "first")
        .await
        .unwrap();
    let newer = f
        .store
        .publish_revision_key(w, generation, json!({"tasks":[]}), "newer")
        .await
        .unwrap();
    assert!(first.0 < newer.0);
    assert!(
        !f.store
            .publish_revision_key(w, generation, json!({"tasks":[]}), "first")
            .await
            .unwrap()
            .1
    );
    let canonical: i64 = f.store.served_revision().await.unwrap();
    assert_eq!(canonical, newer.0);
    f.close().await;
}

#[tokio::test]
async fn synthetic_day_burst() {
    synthetic_burst(false, false).await;
}

#[tokio::test]
async fn synthetic_browser_point_stress() {
    synthetic_burst(true, false).await;
}

#[tokio::test]
async fn synthetic_recent_agent_message_burst() {
    synthetic_burst(false, true).await;
}

async fn synthetic_burst(browser_points: bool, recent: bool) {
    let f = Fixture::new().await;
    let started = std::time::Instant::now();
    let base = if recent {
        hx_store::window_at(Utc::now() - Duration::hours(24)).start
    } else {
        event("base", 0).time.observed_at
    };
    for chunk in 0..40 {
        let events = (chunk * 500..(chunk + 1) * 500)
            .map(|i| {
                let mut e = event(&format!("burst-{i}"), 0);
                e.time =
                    ObservationTime::point(base + Duration::milliseconds(i * 4320));
                if !browser_points {
                    e.kind = Kind::new("agent.message");
                    e.payload = json!({"provider":"synthetic","thread_id":format!("thread-{}",i/100),"role":"assistant","text":"Synthetic activity."});
                }
                e
            })
            .collect();
        assert_eq!(ingest(&f.store, events).await.accepted, 500);
    }
    let ingested = started.elapsed();
    let live: i64 = f.store.live_compile_count().await.unwrap();
    eprintln!("burst ingest={ingested:?}, live compile={live}");
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    assert!(live <= 7);
    // Model elapsed settle time explicitly instead of waiting ten minutes.
    settle_for_test(&f.store).await;
    let mut normalized_compiled = None;
    while hx_jobs::run_one(&f.store, &runner).await.unwrap() {
        if normalized_compiled.is_none() {
            let pending: i64 = f.store.pending_free_job_count().await.unwrap();
            if pending == 0 {
                normalized_compiled = Some(started.elapsed());
            }
        }
    }
    assert_eq!(count(&f.store, "normalized").await, 20_000);
    let unfinished: i64 = f.store.unfinished_job_count().await.unwrap();
    assert_eq!(unfinished, 0);
    let canonical: i64 = f.store.current_revision_count().await.unwrap();
    assert_eq!(
        canonical,
        if recent { 6 } else { 0 },
        "each recent closed window gets a revision; historical data waits for the sweep"
    );
    hx_server::processing::schedule(&f.store).await.unwrap();
    while hx_jobs::run_one(&f.store, &runner).await.unwrap() {}
    let canonical: i64 = f.store.served_window_count().await.unwrap();
    assert_eq!(canonical, if recent { 6 } else { 1 });
    assert_eq!(
        f.store.sweep_reservations().await.unwrap().len(),
        usize::from(!recent),
        "the recent burst must need zero sweeps"
    );
    let tail = hx_store::window_at(base + Duration::days(1));
    let state = f.store.window_state(tail).await.unwrap();
    assert!(state.canonical_revision.is_none());
    assert!(!state.dirty);
    eprintln!("normalize+compile={normalized_compiled:?}");
    let stale: i64 = f.store.stale_compilation_count().await.unwrap();
    assert_eq!(stale, 0);
    eprintln!(
        "burst total={:?}, processing={:?}",
        started.elapsed(),
        started.elapsed() - ingested
    );
    // Shared CI can be much slower than a local Postgres; this is a runaway
    // guard, while the durable assertions above enforce coalescing and cost.
    assert!(started.elapsed().as_secs_f64() < 180.0);
    f.close().await;
}

#[tokio::test]
async fn coalesced_work_keeps_leases_and_requeues_new_input() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("first", 8)]).await;
    let first = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(first.kind, "normalize");
    ingest(&f.store, vec![event("late", 8)]).await;
    assert!(hx_jobs::heartbeat(&f.store, &first, 120).await.unwrap());
    assert!(hx_jobs::finish(&f.store, &first, None).await.unwrap());
    let next = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(first.id, next.id);
    assert_ne!(first.owner, next.owner);
    assert!(!hx_jobs::finish(&f.store, &first, None).await.unwrap());
    assert!(
        !hx_jobs::defer(&f.store, &first, &hx_jobs::Deferred::InputChanged)
            .await
            .unwrap()
    );
    // A newer request survives a crashed worker even on its last allowed attempt.
    f.store.limit_job_attempts(next.id).await.unwrap();
    ingest(&f.store, vec![event("newest", 8)]).await;
    f.store.expire_job_lease(next.id).await.unwrap();
    let newest = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(newest.id, first.id);
    assert_eq!(newest.attempts, 1);
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    hx_jobs::Handler::handle(&runner, &newest).await.unwrap();
    hx_jobs::finish(&f.store, &newest, None).await.unwrap();
    assert_eq!(count(&f.store, "normalized").await, 3);
    // Reopen a completed slot rather than append a second live job.
    ingest(&f.store, vec![event("after-done", 8)]).await;
    assert_eq!(
        hx_jobs::claim(&f.store, 120).await.unwrap().unwrap().id,
        first.id
    );
    let live: i64 = f.store.live_compile_count().await.unwrap();
    assert_eq!(live, 1);
    f.close().await;
}

#[tokio::test]
async fn pending_overlap_defers_interpretation_without_spending_attempts() {
    let f = Fixture::new().await;
    // Exercise readiness while a primary run is still age-eligible.
    let w = hx_store::window_at(Utc::now() - Duration::hours(4));
    let mut target = event("target-visible", 8);
    target.time = ObservationTime::point(w.start);
    ingest(&f.store, vec![target]).await;
    f.store.normalize_window(w).await.unwrap();
    let mut e = event("overlap", 7);
    e.time = ObservationTime::point(w.start - Duration::minutes(15));
    ingest(&f.store, vec![e]).await;
    f.store.settle_test_inputs().await.unwrap();
    // Isolate the interpreter first so the actual overlap readiness guard is exercised.
    f.store.postpone_all_jobs().await.unwrap();
    let mut tx = f.store.transaction().await.unwrap();
    // Immediate eligibility is evaluated by Postgres; do not assume the test
    // process and a disposable database VM have identical wall clocks.
    let ready_at = tx.database_now().await.unwrap();
    tx.enqueue(
        "pending-interpret",
        "interpret",
        json!({"window":w}),
        false,
        ready_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    f.store
        .limit_pending_interpretation_attempts()
        .await
        .unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    for _ in 0..7 {
        assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
        let row = f.store.pending_interpretation_job().await.unwrap();
        assert_eq!(row.get::<String, _>("state"), "ready");
        assert_eq!(row.get::<i32, _>("attempts"), 0);
        assert_eq!(row.get::<String, _>("last_error"), "normalization pending");
        f.store.make_pending_interpretation_due().await.unwrap();
    }
    f.store
        .normalize_window(hx_store::window_at(w.start - Duration::minutes(15)))
        .await
        .unwrap();
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    let state: String = f.store.pending_interpretation_state().await.unwrap();
    assert_eq!(state, "done");
    let canonical: Option<i64> = f.store.canonical_revision_at(w.start).await.unwrap();
    assert!(canonical.is_some());
    f.close().await;
}

#[tokio::test]
async fn job_errors_are_bounded_and_do_not_persist_payload_text() {
    struct Failing;
    impl hx_jobs::Handler for Failing {
        async fn handle(&self, _: &hx_jobs::Job) -> anyhow::Result<()> {
            Err(
                serde_json::from_str::<u64>("\"SYNTHETIC_PRIVATE_SENTINEL\"")
                    .unwrap_err()
                    .into(),
            )
        }
    }
    let f = Fixture::new().await;
    let mut tx = f.store.transaction().await.unwrap();
    tx.enqueue("error", "test", json!({}), false, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(hx_jobs::run_one(&f.store, &Failing).await.is_err());
    let error: String = f.store.job_error().await.unwrap();
    assert_eq!(error, "invalid job or derived JSON");
    assert!(error.len() < 128);
    f.close().await;
}

#[tokio::test]
async fn sensitive_egress_policy_controls_text_and_restricted_never_leaves() {
    let f = Fixture::new().await;
    let mut sensitive = event("sensitive", 8);
    sensitive.sensitivity = Sensitivity::Sensitive;
    sensitive.payload["title"] = json!("SENSITIVE_SENTINEL");
    let mut restricted = event("restricted", 8);
    restricted.sensitivity = Sensitivity::Restricted;
    restricted.payload["title"] = json!("RESTRICTED_SENTINEL");
    ingest(&f.store, vec![sensitive, restricted]).await;
    let ids: Vec<i64> = f.store.ordered_event_ids().await.unwrap();
    for id in &ids {
        f.store.normalize(*id).await.unwrap();
    }
    let w = hx_store::window_at(event("packet", 8).time.observed_at);
    let store = &f.store;
    let packet = |p| async move {
        let input = hx_server::processing::prepare_input_with(store, w, "stub", p)
            .await
            .unwrap();
        (
            input.packet.evidence.len(),
            serde_json::to_string(&input.packet).unwrap(),
        )
    };
    let (n, full) = packet(hx_server::processing::SensitiveEgress::Full).await;
    assert_eq!(n, 1);
    assert!(full.contains("SENSITIVE_SENTINEL"));
    assert!(!full.contains("RESTRICTED_SENTINEL"));
    let (n, redacted) = packet(hx_server::processing::SensitiveEgress::Redact).await;
    assert_eq!(n, 1);
    assert!(!redacted.contains("SENSITIVE_SENTINEL"));
    assert!(!redacted.contains("RESTRICTED_SENTINEL"));
    f.close().await;
}

#[tokio::test]
async fn backfilled_evidence_is_placed_at_source_time() {
    let f = Fixture::new().await;
    let mut late = event("late", 8);
    late.time.observed_at += chrono::Duration::hours(7);
    ingest(&f.store, vec![late]).await;
    let ids: Vec<i64> = f.store.event_ids().await.unwrap();
    for id in ids {
        f.store.normalize(id).await.unwrap();
    }
    let w = hx_store::window_at(event("packet", 8).time.observed_at);
    let input = hx_server::processing::prepare_input(&f.store, w, "stub")
        .await
        .unwrap();
    assert_eq!(input.packet.evidence.len(), 1);
    assert!(w.contains(input.packet.evidence[0].at));
    f.close().await;
}

// Every OCR test uses generated PNG bytes, an injected engine, and a scratch schema.
struct SyntheticOcr {
    calls: std::sync::atomic::AtomicUsize,
    fail: bool,
}
impl SyntheticOcr {
    fn new(fail: bool) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            fail,
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}
impl hx_visual::OcrEngine for SyntheticOcr {
    fn ocr(&self, _: &[u8], _: &str) -> anyhow::Result<hx_visual::OcrResult> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        anyhow::ensure!(!self.fail, "synthetic engine failure: PRIVATE TEXT");
        Ok(hx_visual::OcrResult {
            engine_name: "synthetic".into(),
            engine_version: "1".into(),
            coverage: "partial".into(),
            lines: vec![hx_visual::OcrLine {
                text: format!(
                    "SYNTHETIC ORCHID <script>alert(1)</script> {}",
                    "🌸".repeat(3000)
                ),
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
fn synthetic_png(variant: u8) -> Vec<u8> {
    let mut pixels = image::GrayImage::new(64, 64);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = image::Luma([if x < 32 { 10 + variant } else { 240 - variant }]);
        if x == 1 && y == 1 {
            *pixel = image::Luma([variant]);
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(pixels)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}
fn still(key: &str, seconds: i64, sensitivity: Sensitivity, bytes: &[u8]) -> Envelope {
    let mut e = event(key, 1);
    e.time = ObservationTime::point(e.time.observed_at + Duration::seconds(seconds));
    e.kind = Kind::new("media.still");
    e.device = Some(hx_schema::DeviceId::new("synthetic-device"));
    e.payload = json!({"display":"synthetic","trigger":"focus_change","width":64,"height":64,"app_bundle_id":"synthetic.app"});
    e.sensitivity = sensitivity;
    e.blobs.push(hx_schema::BlobRef {
        sha256: hx_store::hash(bytes),
        media_type: "image/png".into(),
        bytes: bytes.len() as u64,
    });
    e
}
async fn drain_ocr(store: &Store, engine: std::sync::Arc<SyntheticOcr>, version: &str) {
    hx_server::ocr::discover(store, version).await.unwrap();
    while hx_server::ocr::run_one(store, engine.clone(), version)
        .await
        .unwrap()
    {}
}

#[tokio::test]
async fn ocr_backfill_exact_cache_lineage_packet_and_dashboard() {
    let f = Fixture::new().await;
    let bytes = synthetic_png(0);
    let sha = hx_store::hash(&bytes);
    let first = still("ocr-first", 0, Sensitivity::Normal, &bytes);
    let observation_id = first.observation_id.to_string();
    let source_time = first.time.source_start.unwrap();
    ingest(&f.store, vec![first]).await;
    let engine = SyntheticOcr::new(false);
    // Event-first arrival does not consume OCR attempts before its blob is published.
    drain_ocr(&f.store, engine.clone(), "fake/v1").await;
    assert_eq!(engine.calls(), 0);
    f.store.upload_blob(&sha, &bytes).await.unwrap();
    drain_ocr(&f.store, engine.clone(), "fake/v1").await;
    // New observations may arrive after the cached blob was already processed.
    ingest(
        &f.store,
        vec![still("ocr-late", 60, Sensitivity::Restricted, &bytes)],
    )
    .await;
    drain_ocr(&f.store, engine.clone(), "fake/v1").await;
    assert_eq!(engine.calls(), 1);
    assert!(!f.store.upload_blob(&sha, &bytes).await.unwrap());
    let rows = f.store.derived_ocr_records().await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String, _>("sensitivity"), "normal");
    assert_eq!(rows[1].get::<String, _>("sensitivity"), "restricted");
    let envelope: Value = rows[0].get("envelope");
    assert_eq!(envelope["payload"]["source_observation_id"], observation_id);
    assert_eq!(envelope["payload"]["sha256"], sha);
    assert_eq!(envelope["connector_instance"], "test:device");
    assert_eq!(envelope["device"], "synthetic-device");
    let source_id: i64 = rows[0].get("source_event_id");
    let derived_id: i64 = rows[0].get("id");
    let source = f.store.evidence(source_id).await.unwrap().unwrap();
    assert_eq!(envelope["time"], source["envelope"]["time"]);
    let w = hx_store::window_at(source_time);
    f.store.normalize_window(w).await.unwrap();
    let prepared = hx_server::processing::prepare_input_with(
        &f.store,
        w,
        "synthetic",
        hx_server::processing::SensitiveEgress::Full,
    )
    .await
    .unwrap();
    let ocr: Vec<_> = prepared
        .packet
        .evidence
        .iter()
        .filter(|e| e.kind == "normal:ocr.text")
        .collect();
    assert_eq!(ocr.len(), 1);
    assert_eq!(ocr[0].id, derived_id);
    assert_eq!(ocr[0].at, source_time);
    assert!(ocr[0].text.contains("ORCHID"));
    assert!(ocr[0].text.len() <= hx_interpret::PacketBudget::default().max_item_chars);
    let value = hx_server::authorized_evidence(&f.store, &token(), derived_id)
        .await
        .unwrap();
    assert_eq!(value["source_event_id"], source_id);
    let denied = Token {
        prefixes: vec!["other:".into()],
        scopes: vec!["read_raw".into()],
    };
    assert!(
        hx_server::authorized_evidence(&f.store, &denied, derived_id)
            .await
            .is_err()
    );
    let secret = f
        .store
        .create_token("ocr-reader", token().prefixes, token().scopes)
        .await
        .unwrap();
    let response = hx_server::router(f.store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::builder()
                .uri(format!("/evidence/{source_id}"))
                .header("authorization", format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("data:image/png;base64,"));
    assert!(body.contains("OCR text"));
    assert!(body.contains("&lt;script&gt;"));
    assert!(!body.contains("<script>alert(1)</script>"));
    // Version bumps produce new durable results, then remain exact-cache hits.
    drain_ocr(&f.store, engine.clone(), "fake/v2").await;
    drain_ocr(&f.store, engine.clone(), "fake/v2").await;
    assert_eq!(engine.calls(), 2);
    f.close().await;
}

#[tokio::test]
async fn ocr_sampling_is_not_text_reuse_and_is_scoped_to_surface() {
    let f = Fixture::new().await;
    let engine = SyntheticOcr::new(false);
    for (key, second, variant, surface) in [
        ("a", 0, 0, "one"),
        ("b", 1, 1, "one"),
        ("c", 2, 2, "two"),
        ("d", 90, 3, "one"),
    ] {
        let bytes = synthetic_png(variant);
        // Blob-first ordering is also discovered.
        f.store
            .upload_blob(&hx_store::hash(&bytes), &bytes)
            .await
            .unwrap();
        let mut e = still(key, second, Sensitivity::Normal, &bytes);
        e.payload["display"] = json!(surface);
        ingest(&f.store, vec![e]).await;
        drain_ocr(&f.store, engine.clone(), "fake/v1").await;
    }
    assert_eq!(engine.calls(), 3);
    let sampled: i64 = f.store.sampled_ocr_count().await.unwrap();
    assert_eq!(sampled, 1);
    let evidence: i64 = f.store.ocr_evidence_count().await.unwrap();
    assert_eq!(evidence, 3);
    assert_eq!(count(&f.store, "events").await, 4);
    f.close().await;
}

#[tokio::test]
async fn ocr_source_policy_redaction_self_generated_and_bounded_retries() {
    let f = Fixture::new().await;
    let bytes = synthetic_png(0);
    f.store
        .upload_blob(&hx_store::hash(&bytes), &bytes)
        .await
        .unwrap();
    let mut e = still("sensitive", 0, Sensitivity::Sensitive, &bytes);
    e.payload["window_title"] = json!("Hyperthymesia synthetic");
    ingest(&f.store, vec![e]).await;
    let engine = SyntheticOcr::new(false);
    drain_ocr(&f.store, engine.clone(), "fake/v1").await;
    let at = Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap();
    let w = hx_store::window_at(at);
    f.store.normalize_window(w).await.unwrap();
    for policy in [
        hx_server::processing::SensitiveEgress::Full,
        hx_server::processing::SensitiveEgress::Redact,
    ] {
        let input = hx_server::processing::prepare_input_with(&f.store, w, "synthetic", policy)
            .await
            .unwrap();
        let item = input
            .packet
            .evidence
            .iter()
            .find(|e| e.kind.ends_with("ocr.text"))
            .unwrap();
        assert!(item.self_generated);
        assert_eq!(
            item.text.contains("ORCHID"),
            policy == hx_server::processing::SensitiveEgress::Full
        );
    }
    let broken = SyntheticOcr::new(true);
    hx_server::ocr::discover(&f.store, "broken/v1")
        .await
        .unwrap();
    for _ in 0..5 {
        assert!(
            hx_server::ocr::run_one(&f.store, broken.clone(), "broken/v1")
                .await
                .unwrap()
        );
        f.store.make_broken_ocr_due().await.unwrap();
    }
    assert!(
        !hx_server::ocr::run_one(&f.store, broken.clone(), "broken/v1")
            .await
            .unwrap()
    );
    assert_eq!(broken.calls(), 5);
    let status: String = f.store.broken_ocr_status().await.unwrap();
    assert_eq!(status, "failed");
    f.close().await;
}

struct ConcurrentOcr {
    calls: std::sync::atomic::AtomicUsize,
    active: std::sync::atomic::AtomicUsize,
    maximum: std::sync::atomic::AtomicUsize,
}
impl hx_visual::OcrEngine for ConcurrentOcr {
    fn ocr(&self, bytes: &[u8], media: &str) -> anyhow::Result<hx_visual::OcrResult> {
        use std::sync::atomic::Ordering::SeqCst;
        self.calls.fetch_add(1, SeqCst);
        let active = self.active.fetch_add(1, SeqCst) + 1;
        self.maximum.fetch_max(active, SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.active.fetch_sub(1, SeqCst);
        hx_visual::NullEngine.ocr(bytes, media)
    }
}
#[tokio::test]
async fn ocr_concurrent_workers_are_bounded_and_committed_results_survive_restart() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::SeqCst},
    };
    let f = Fixture::new().await;
    for i in 0..3 {
        let bytes = synthetic_png(i);
        let e = still(
            &format!("concurrent-{i}"),
            i64::from(i) * 60,
            Sensitivity::Normal,
            &bytes,
        );
        f.store
            .upload_blob(&hx_store::hash(&bytes), &bytes)
            .await
            .unwrap();
        ingest(&f.store, vec![e]).await;
    }
    hx_server::ocr::discover(&f.store, "fake/v1").await.unwrap();
    let engine = Arc::new(ConcurrentOcr {
        calls: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        maximum: AtomicUsize::new(0),
    });
    for _ in 0..3 {
        let (a, b) = tokio::join!(
            hx_server::ocr::run_one(&f.store, engine.clone(), "fake/v1"),
            hx_server::ocr::run_one(&f.store, engine.clone(), "fake/v1")
        );
        a.unwrap();
        b.unwrap();
    }
    assert_eq!(engine.calls.load(SeqCst), 3);
    assert_eq!(engine.maximum.load(SeqCst), 1);
    // A fresh engine/worker sees the committed cache, not in-memory state.
    let restarted = SyntheticOcr::new(false);
    drain_ocr(&f.store, restarted.clone(), "fake/v1").await;
    assert_eq!(restarted.calls(), 0);
    let jobs: i64 = f.store.complete_ocr_count().await.unwrap();
    assert_eq!(jobs, 3);
    f.close().await;
}

#[tokio::test]
async fn ocr_late_results_dirty_overlap_and_keep_original_source_time() {
    let f = Fixture::new().await;
    let bytes = synthetic_png(0);
    let e = still(
        "overlap-ocr",
        2 * 3600 + 50 * 60,
        Sensitivity::Normal,
        &bytes,
    );
    let at = e.time.source_start.unwrap();
    ingest(&f.store, vec![e]).await;
    let w = hx_store::window_at(at);
    let next = hx_store::window_at(w.end);
    f.store.normalize_window(w).await.unwrap();
    hx_server::processing::compile_window(&f.store, w)
        .await
        .unwrap();
    hx_server::processing::compile_window(&f.store, next)
        .await
        .unwrap();
    let before = f.store.generation(next).await.unwrap();
    f.store
        .upload_blob(&hx_store::hash(&bytes), &bytes)
        .await
        .unwrap();
    drain_ocr(&f.store, SyntheticOcr::new(false), "fake/v1").await;
    let after = f.store.generation(next).await.unwrap();
    assert!(after > before);
    let dirty: i64 = f
        .store
        .dirty_window_count(vec![w.start, next.start])
        .await
        .unwrap();
    assert_eq!(dirty, 2);
    // Existing packet policy reserves at most 5% of items for lead-in evidence.
    // Populate the target window so both the source still and OCR fit that quota.
    let target: Vec<_> = (0..40)
        .map(|i| {
            let mut e = event(&format!("target-{i}"), 4);
            e.time = ObservationTime::point(e.time.observed_at + Duration::minutes(i));
            e
        })
        .collect();
    ingest(&f.store, target).await;
    f.store.normalize_window(next).await.unwrap();
    let after = f.store.generation(next).await.unwrap();
    let input = hx_server::processing::prepare_input_with(
        &f.store,
        next,
        "synthetic",
        hx_server::processing::SensitiveEgress::Full,
    )
    .await
    .unwrap();
    let derived = input
        .packet
        .evidence
        .iter()
        .find(|e| e.kind.ends_with("ocr.text"))
        .unwrap();
    assert_eq!(derived.at, at);
    assert!(derived.at < next.start);
    let value = f.store.evidence(derived.id).await.unwrap().unwrap();
    // Derived envelopes retain the schema's valid observation identities and time contract.
    let parsed: Envelope = serde_json::from_value(value["envelope"].clone()).unwrap();
    assert_eq!(parsed.time.source_start, Some(at));
    // Discovery replay does not dirty windows again for existing lineage.
    hx_server::ocr::discover(&f.store, "fake/v1").await.unwrap();
    assert_eq!(f.store.generation(next).await.unwrap(), after);
    f.close().await;
}

/// Exercise authenticated ingest all the way to the provider prompt, using only
/// generated laptop observations. Observed/received time deliberately differs.
#[tokio::test]
async fn laptop_disclosure_reaches_provider_with_source_time_and_focus_support() {
    use hx_server::{
        disclosure::DisclosurePolicy,
        processing::{SensitiveEgress, prepare_input_with_policy},
    };
    struct RecordingProvider(std::sync::Mutex<Vec<String>>);
    impl hx_interpret::Provider for RecordingProvider {
        fn name(&self) -> String {
            "synthetic-recording".into()
        }
        async fn complete(
            &self,
            _system: &str,
            user: &str,
        ) -> Result<String, hx_interpret::InterpretError> {
            self.0.lock().unwrap().push(user.into());
            Ok(json!({"episodes":[],"tasks":[],"summary":"Synthetic fixture."}).to_string())
        }
    }
    let f = Fixture::new().await;
    let secret = f
        .store
        .create_token(
            "synthetic-laptop",
            vec![
                "macos-agent:fixture".into(),
                "macos-media:fixture".into(),
                "companion:browser:fixture".into(),
            ],
            vec!["ingest".into()],
        )
        .await
        .unwrap();
    let source = event("source", 8).time.observed_at;
    let make = |key: &str, connector: &str, kind: &str, payload: Value| {
        let mut e = event(key, 8);
        e.connector_instance = ConnectorInstance::new(connector);
        e.kind = Kind::new(kind);
        e.sensitivity = Sensitivity::Restricted;
        e.device = Some(hx_schema::DeviceId::new("fixture"));
        e.time.source_start = Some(source);
        e.time.source_end = Some(source + Duration::seconds(30));
        e.time.observed_at = source + Duration::hours(7);
        e.payload = payload;
        e
    };
    let focus = make(
        "focus",
        "macos-agent:fixture",
        "focus.span",
        json!({"app_bundle_id":"test.editor","app_name":"Synthetic Editor","window_title":"WINDOW_TITLE_SENTINEL"}),
    );
    let mut title_free = focus.clone();
    title_free.observation_id = ObservationId::generate();
    title_free.source_key = SourceKey::new("title-free-focus");
    title_free.payload["window_title"] = Value::Null;
    title_free.time.source_start = Some(source + Duration::seconds(30));
    title_free.time.source_end = Some(source + Duration::seconds(60));
    let text = make(
        "text",
        "macos-agent:fixture",
        "text.snapshot",
        json!({"surface":"synthetic editor","source":"accessibility","text":"TEXT_SENTINEL","text_sha256":hx_store::hash(b"TEXT_SENTINEL"),"completeness":"partial"}),
    );
    let mut ocr = text.clone();
    ocr.observation_id = ObservationId::generate();
    ocr.source_key = SourceKey::new("ocr");
    ocr.connector_instance = ConnectorInstance::new("macos-media:fixture");
    ocr.payload["source"] = json!("ocr");
    ocr.payload["text"] = json!("OCR_SENTINEL");
    ocr.payload["text_sha256"] = json!(hx_store::hash(b"OCR_SENTINEL"));
    let gap = make(
        "gap",
        "macos-agent:fixture",
        "policy.gap",
        json!({"reason":"GAP_SENTINEL"}),
    );
    let media = make(
        "media",
        "macos-media:fixture",
        "media.still",
        json!({"display":"fixture","trigger":"periodic","width":1,"height":1,"window_title":"MEDIA_SENTINEL"}),
    );
    let companion = make(
        "companion",
        "companion:browser:fixture",
        "browser.navigation",
        json!({"title":"COMPANION_SENTINEL"}),
    );
    let unauthorized = make(
        "spoof",
        "macos-agent:other-device",
        "text.snapshot",
        text.payload.clone(),
    );
    let (status, body) = request(
        &f.store,
        "POST",
        "/v1/events",
        &secret,
        serde_json::to_vec(&UploadBatch {
            batch_id: ulid::Ulid::generate(),
            envelopes: vec![
                focus,
                title_free,
                text,
                ocr,
                gap,
                media,
                companion,
                unauthorized,
            ],
        })
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ack: hx_store::UploadAck = serde_json::from_slice(&body).unwrap();
    assert_eq!(ack.accepted, 7);
    assert_eq!(ack.rejected.len(), 1);
    assert!(ack.rejected[0].reason.contains("namespace"));
    let w = hx_store::window_at(source);
    f.store.normalize_window(w).await.unwrap();
    assert_eq!(count(&f.store, "quarantine").await, 0);
    let policy = DisclosurePolicy::laptop();
    let input = prepare_input_with_policy(&f.store, w, "recording", SensitiveEgress::Full, &policy)
        .await
        .unwrap();
    assert_eq!(input.packet.evidence.len(), 5);
    assert_eq!(
        input.manifest["disclosure_policy"]["digest"],
        policy.digest()
    );
    assert_eq!(
        input.manifest["disclosure_policy"]["version"],
        hx_server::disclosure::POLICY_VERSION
    );
    for item in &input.packet.evidence {
        assert!(item.at == source || item.at == source + Duration::seconds(30));
        assert_eq!(
            item.effective_class,
            if item.at == source + Duration::seconds(30) {
                Sensitivity::Normal
            } else {
                Sensitivity::Sensitive
            }
        );
    }
    let support: Vec<hx_schema::Interval> = serde_json::from_str(
        input
            .packet
            .timeline_text
            .lines()
            .find_map(|line| line.strip_prefix("time_support="))
            .unwrap(),
    )
    .unwrap();
    assert!(support.contains(&hx_schema::Interval {
        start: source,
        end: source + Duration::seconds(30)
    }));
    assert!(support.contains(&hx_schema::Interval {
        start: source + Duration::seconds(30),
        end: source + Duration::seconds(60)
    }));
    let provider = RecordingProvider(Default::default());
    hx_interpret::interpret(&input.packet, &provider)
        .await
        .unwrap();
    let prompts = provider.0.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    let prompt = &prompts[0];
    for sentinel in [
        "WINDOW_TITLE_SENTINEL",
        "TEXT_SENTINEL",
        "OCR_SENTINEL",
        "COMPANION_SENTINEL",
        "Synthetic Editor",
    ] {
        assert!(prompt.contains(sentinel), "missing {sentinel}");
    }
    assert!(!prompt.contains("GAP_SENTINEL"));
    assert!(!prompt.contains("MEDIA_SENTINEL"));
    let redacted =
        prepare_input_with_policy(&f.store, w, "recording", SensitiveEgress::Redact, &policy)
            .await
            .unwrap();
    let redacted = serde_json::to_string(&redacted.packet).unwrap();
    for sentinel in [
        "WINDOW_TITLE_SENTINEL",
        "TEXT_SENTINEL",
        "OCR_SENTINEL",
        "COMPANION_SENTINEL",
    ] {
        assert!(!redacted.contains(sentinel), "leaked {sentinel}");
    }
    assert!(redacted.contains("Synthetic Editor")); // title-free normal span
    let restricted = DisclosurePolicy::from_json(
        r#"{"rules":[
        {"connector_prefix":"macos-","kind":"*","class":"restricted"},
        {"connector_prefix":"companion:","kind":"*","class":"restricted"}
    ]}"#,
    )
    .unwrap();
    for denied in [&DisclosurePolicy::default(), &restricted] {
        let blocked =
            prepare_input_with_policy(&f.store, w, "recording", SensitiveEgress::Full, denied)
                .await
                .unwrap();
        assert!(blocked.packet.evidence.is_empty());
        assert!(!blocked.packet.timeline_text.contains("Synthetic Editor"));
        assert_ne!(blocked.packet.input_digest, input.packet.input_digest);
    }
    // Even an explicit owner grant cannot export gap content or media records.
    let broad = DisclosurePolicy::from_json(
        r#"{"rules":[
        {"connector_prefix":"macos-","kind":"*","class":"normal"},
        {"connector_prefix":"companion:","kind":"*","class":"normal"}
    ]}"#,
    )
    .unwrap();
    let granted =
        prepare_input_with_policy(&f.store, w, "recording", SensitiveEgress::Full, &broad)
            .await
            .unwrap();
    assert_eq!(granted.packet.evidence.len(), 5);
    let granted = serde_json::to_string(&granted.packet).unwrap();
    assert!(!granted.contains("GAP_SENTINEL"));
    assert!(!granted.contains("MEDIA_SENTINEL"));
    // Egress projection must not mutate the immutable producer classification.
    assert!(
        f.store
            .observations(w, true)
            .await
            .unwrap()
            .iter()
            .all(|o| o.envelope.sensitivity == Sensitivity::Restricted)
    );
    f.close().await;
}

/// A restricted-labelled laptop still yields OCR text under the owner's laptop
/// policy, while the still record itself never enters the packet.
#[tokio::test]
async fn laptop_policy_turns_restricted_stills_into_ocr_text_only() {
    let f = Fixture::new().await;
    let bytes = synthetic_png(7);
    let sha = hx_store::hash(&bytes);
    let mut e = still("laptop-still", 0, Sensitivity::Restricted, &bytes);
    e.connector_instance = ConnectorInstance::new("macos-media:fixture");
    let source_time = e.time.source_start.unwrap();
    let laptop = Token {
        prefixes: vec!["macos-media:".into()],
        scopes: vec!["ingest".into()],
    };
    f.store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![e],
            },
            &laptop,
        )
        .await
        .unwrap();
    f.store.upload_blob(&sha, &bytes).await.unwrap();
    drain_ocr(&f.store, SyntheticOcr::new(false), "fake/v1").await;
    let w = hx_store::window_at(source_time);
    f.store.normalize_window(w).await.unwrap();
    let prepare = |policy: hx_server::disclosure::DisclosurePolicy| {
        let store = f.store.clone();
        async move {
            hx_server::processing::prepare_input_with_policy(
                &store,
                w,
                "synthetic",
                hx_server::processing::SensitiveEgress::Full,
                &policy,
            )
            .await
            .unwrap()
        }
    };
    let granted = prepare(hx_server::disclosure::DisclosurePolicy::laptop()).await;
    let kinds: Vec<_> = granted
        .packet
        .evidence
        .iter()
        .map(|e| e.kind.as_str())
        .collect();
    assert_eq!(kinds, ["normal:ocr.text"]);
    assert_eq!(granted.packet.evidence[0].at, source_time);
    assert_eq!(
        granted.packet.evidence[0].effective_class,
        Sensitivity::Sensitive
    );
    assert!(granted.packet.evidence[0].text.contains("ORCHID"));
    let withheld = prepare(hx_server::disclosure::DisclosurePolicy::default()).await;
    assert!(withheld.packet.evidence.is_empty());
    f.close().await;
}

#[tokio::test]
async fn digest_only_cites_current_non_deleted_store_revisions() {
    use hx_server::{mcp::Broker, processing::SensitiveEgress};
    let f = Fixture::new().await;
    let mut rows = vec![];
    for (key, latest_hour, delete) in [
        ("edited", 1, false),
        ("moved", 8, false),
        ("deleted", 1, true),
        ("deleted-later", 8, true),
    ] {
        let mut old = event(key, 1);
        old.kind = Kind::new("source.email");
        old.operation = Operation::Upsert;
        old.revision = Revision::new("old");
        old.sequence = Some(1);
        old.payload = json!({"title":format!("STALE_{key}")});
        let mut latest = old.clone();
        latest.observation_id = ObservationId::generate();
        latest.revision = Revision::new("new");
        latest.sequence = Some(2);
        latest.time =
            ObservationTime::point(Utc.with_ymd_and_hms(2026, 1, 1, latest_hour, 0, 0).unwrap());
        latest.payload = json!({"title":format!("CURRENT_{key}")});
        if delete {
            latest.operation = Operation::Delete;
        }
        // Arrival order and lexical revision order must not beat producer sequence.
        rows.extend([latest, old]);
    }
    let mut independent = event("edited", 2);
    independent.kind = Kind::new("source.email");
    independent.connector_instance = ConnectorInstance::new("test:other");
    independent.payload = json!({"title":"INDEPENDENT_CONNECTOR"});
    rows.push(independent);
    ingest(&f.store, rows).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    let policy = DisclosurePolicy::from_json(
        r#"{"rules":[{"connector_prefix":"test:","kind":"*","class":"normal"}]}"#,
    )
    .unwrap();
    let broker = Broker::new(f.store.clone(), policy, SensitiveEgress::Full, w);
    let digest = broker.digest().await.unwrap();
    let audit = broker.audit.lock().await;
    let evidence: Vec<_> = audit.digest_evidence.values().collect();
    assert!(!digest.contains("STALE_"), "{digest}");
    assert!(!digest.contains("CURRENT_deleted"), "{digest}");
    assert!(!digest.contains("CURRENT_moved"), "{digest}");
    assert!(digest.contains("CURRENT_edited") && digest.contains("INDEPENDENT_CONNECTOR"));
    assert_eq!(evidence.len(), 2);
    assert!(
        evidence
            .iter()
            .all(|e| !e.text.contains("STALE_") && !e.text.contains("CURRENT_deleted"))
    );
    assert_eq!(audit.digest_support.len(), 2);
    assert!(
        audit
            .digest_support
            .keys()
            .all(|id| audit.digest_evidence.contains_key(id))
    );
    drop(audit);
    f.close().await;
}

#[tokio::test]
async fn mcp_tools_are_bounded_and_current_policy_controls_all_context() {
    use hx_server::{disclosure::DisclosurePolicy, mcp::Broker, processing::SensitiveEgress};
    let f = Fixture::new().await;
    let mut hidden = event("mcp-hidden", 1);
    hidden.sensitivity = Sensitivity::Restricted;
    hidden.payload["title"] = json!("SYNTHETIC_RESTRICTED_CANARY");
    let mut visible = event("mcp-visible", 1);
    visible.payload["title"] = json!("synthetic visible");
    ingest(&f.store, vec![hidden, visible]).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    f.store.normalize_window(w).await.unwrap();
    let observations = f.store.observations(w, true).await.unwrap();
    let hidden = observations
        .iter()
        .find(|o| o.envelope.sensitivity == Sensitivity::Restricted)
        .unwrap()
        .id;
    let visible = observations
        .iter()
        .find(|o| o.envelope.sensitivity == Sensitivity::Normal)
        .unwrap()
        .id;
    let broker = Broker::new(
        f.store.clone(),
        DisclosurePolicy::default(),
        SensitiveEgress::Full,
        w,
    );
    for (tool, args) in [
        ("window_overview", json!({"from":w.start,"to":w.end})),
        (
            "timeline",
            json!({"from":w.start,"to":w.end,"level":"seconds"}),
        ),
        ("search", json!({"query":"synthetic"})),
        ("read_evidence", json!({"id":hidden})),
        ("read_evidence", json!({"id":visible})),
        ("screen_at", json!({"time":w.start+Duration::hours(1)})),
        ("list_sources", json!({})),
        ("prior_context", json!({"before":w.end})),
        (
            "live_lookup",
            json!({"source":"synthetic","request":"fixture"}),
        ),
    ] {
        let result = broker.call(tool, args).await.unwrap();
        assert!(
            !result.to_string().contains("SYNTHETIC_RESTRICTED_CANARY"),
            "{tool}"
        );
        assert!(serde_json::to_vec(&result).unwrap().len() <= hx_server::mcp::MAX_RESULT_BYTES);
    }
    assert!(broker.audit.lock().await.evidence.contains_key(&visible));
    assert!(!broker.audit.lock().await.evidence.contains_key(&hidden));
    f.store
        .assert_entity("synthetic", json!({"name":"synthetic memory"}), &[visible])
        .await
        .unwrap();
    assert_eq!(
        broker.prior_context(w.end).await.unwrap()["entities"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let deny = DisclosurePolicy::from_json(
        r#"{"rules":[{"connector_prefix":"test:","kind":"*","class":"restricted"}]}"#,
    )
    .unwrap();
    let denied = Broker::new(f.store.clone(), deny, SensitiveEgress::Full, w);
    let context = denied.prior_context(w.end).await.unwrap();
    assert!(context["entities"].as_array().unwrap().is_empty());
    assert!(context["revisions"].as_array().unwrap().is_empty());
    let grant = DisclosurePolicy::from_json(
        r#"{"rules":[{"connector_prefix":"test:","kind":"*","class":"normal"}]}"#,
    )
    .unwrap();
    // The owner can authorize a producer-restricted record; cached labels cannot veto it.
    f.store
        .assert_entity(
            "synthetic granted",
            json!({"name":"synthetic granted memory"}),
            &[hidden],
        )
        .await
        .unwrap();
    let granted = Broker::new(f.store.clone(), grant, SensitiveEgress::Full, w);
    assert_eq!(
        granted.prior_context(w.end).await.unwrap()["entities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let calls = broker.audit.lock().await.calls.len();
    for _ in calls..hx_server::mcp::MAX_CALLS {
        broker.call("live_lookup", json!({})).await.unwrap();
    }
    assert!(broker.call("list_sources", json!({})).await.is_err());
    assert!(broker.audit.lock().await.exhausted);
    f.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn fx_reads_the_digest_cites_its_printed_times_and_stops_at_the_lookup_cap() {
    let f = Fixture::new().await;
    // Sub-second times: the digest prints whole seconds, and those must validate.
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap() + Duration::milliseconds(250);
    let focus = |key: &str, from: i64, to: i64, app: &str, title: &str| {
        let mut e = event(key, 1);
        e.device = Some(hx_schema::DeviceId::new("synthetic-device"));
        e.kind = Kind::new("focus.span");
        e.time = ObservationTime::point(base + Duration::seconds(from));
        e.time.source_end = Some(base + Duration::seconds(to));
        e.payload = json!({"app_bundle_id":"synthetic.app","app_name":app,"window_title":title,"pid":1,"display":null});
        e
    };
    ingest(
        &f.store,
        vec![
            focus("digest-a", 0, 300, "Synthetic Browser", "Synthetic search"),
            focus(
                "digest-b",
                300,
                1500,
                "Synthetic Editor",
                "Draft the launch note",
            ),
        ],
    )
    .await;
    let w = hx_store::window_at(base);
    f.store.normalize_window(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = fake_fx(scratch.path(), "digest");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-digest", &config)
            .await
            .unwrap()
    );
    let value: Value = f.store.fx_digest_interpretation().await.unwrap();
    // The printed whole-second times validate; only the non-Mac source wording is downgraded.
    assert_eq!(
        value["episodes"][0]["intervals"],
        json!([{"start":"2026-01-01T01:05:00Z","end":"2026-01-01T01:25:00Z"}]),
        "{}",
        value["check_failures"]
    );
    let run: Value = f.store.fx_digest_agent_run().await.unwrap();
    assert!(run["digest"].as_str().unwrap().contains("Synthetic Editor"));
    let calls = run["calls"].as_array().unwrap();
    // Repair attempts share the cap: later attempts get no fresh lookups.
    assert!(calls.len() > hx_server::fx::DIGEST_LOOKUPS);
    let capped = hx_server::fx::DIGEST_LOOKUPS;
    assert!(
        calls[..capped]
            .iter()
            .all(|c| c["result"]["error"].is_null())
    );
    assert!(
        calls[capped..]
            .iter()
            .all(|c| c["result"]["error"].as_str().unwrap().contains("budget"))
    );
    f.close().await;
}

#[cfg(unix)]
fn fake_fx(dir: &std::path::Path, mode: &str) -> hx_server::fx::Config {
    use std::os::unix::fs::PermissionsExt;
    let binary = dir.join(format!("fake-fx-{mode}"));
    let script = r#"#!/usr/bin/python3
import sys,os,json,pathlib,time,urllib.request
if '--version' in sys.argv:
 print('0.0.16');sys.exit(0)
mode = 'MODE'
if mode in ('rate','generate','rate-retrying'):
 with (pathlib.Path(__file__).parent/'paid-calls').open('a') as calls: calls.write('launch\n')
home=pathlib.Path(os.environ['HOME']); work=pathlib.Path.cwd()
settings=json.loads((home/'.fx/settings.json').read_text())
assert settings['context'] is False and settings['permission']['*']=='deny'
assert settings['permission']['mcp_hyperthymesia_*']=='allow'
assert 0<settings['max_agent_steps']<=120
assert work.name=='work' and not (home/'.codex').exists() and not (home/'.claude').exists()
assert '--no-save' in sys.argv and '--fast' in sys.argv and '--no-ultrafast' in sys.argv
assert sys.argv[sys.argv.index('--effort')+1]=='xhigh'
assert sys.argv[sys.argv.index('--model')+1]=='gpt-6.1-sol'
assert 'hx-replica/v9' in sys.argv[sys.argv.index('--system')+1]
instr=sys.stdin.read()
assert instr.strip() and ('<digest>' in instr or 'Repair only' in instr)
if mode=='timeout': time.sleep(3)
if mode=='failure': print(json.dumps({'error':'synthetic failure','exit_code':1}));sys.exit(1)
if mode=='generate': print(json.dumps({'error':'could not generate output','exit_code':1}));sys.exit(1)
if mode=='rate': print(json.dumps({'error':'429 rate limited','exit_code':1}));sys.exit(1)
if mode=='rate-retrying':
 sys.stderr.write('[notice] Rate limited · HTTP 429 · usage_limit_reached: The usage limit has been reached · retrying request in 30s\n'); sys.stderr.flush(); time.sleep(30)
server=json.loads((home/'.fx/mcp.json').read_text())['mcpServers']['hyperthymesia']
def rpc(method,params):
 req=urllib.request.Request(server['url'],data=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+os.environ['HX_MCP_TOKEN']})
 return json.load(urllib.request.urlopen(req))['result']
rpc('initialize',{})
assert len(rpc('tools/list',{})['tools'])==8
if mode=='digest':
 import re
 line=next(l for l in instr.splitlines() if l.startswith('[') and 'Synthetic Editor' in l)
 assert 'Draft the launch note' in line, line
 m=re.match(r'\[(\d+)[^\]]*\] (\d\d:\d\d:\d\d)-(\d\d:\d\d:\d\d) ',line)
 i=int(m.group(1)); iv={'start':'2026-01-01T'+m.group(2)+'Z','end':'2026-01-01T'+m.group(3)+'Z'}
 answer={'episodes':[{'id':'synthetic-digest','intervals':[iv],'title':'Drafted the launch note','description':'Wrote in the synthetic editor','claims':[{'actor':'owner','action':'edited','object':'launch note','outcome':'unknown','basis':'observed','evidence':[i],'outcome_evidence':[]}],'evidence':[i],'confidence':0.8}],'tasks':[],'summary':'Drafted the launch note'}
 for _ in range(12): rpc('tools/call',{'name':'list_sources','arguments':{}})
 print(json.dumps({'exit_code':0,'model':'gpt-6.1-sol','steps':2,'final_output':json.dumps(answer),'tool_calls':[{'name':'mcp_hyperthymesia_list_sources','status':'success'}]*12}))
 sys.exit(0)
start='2026-01-01T03:58:00Z' if mode=='late' else '2026-01-01T00:00:00Z'
result=rpc('tools/call',{'name':'window_overview','arguments':{'from':start,'to':'2026-01-01T04:00:00Z'}})
data=json.loads(result['content'][0]['text'])
assert 'SYNTHETIC_RESTRICTED_CANARY' not in json.dumps(data)
e=data['evidence'][0]; point={'start':e['at'],'end':e['at']}
answer={'episodes':[{'id':'synthetic-episode','intervals':[point],'title':'Synthetic visit','description':'Synthetic visit','claims':[{'actor':'owner','action':'visited','object':'synthetic page','outcome':'unknown','basis':'observed','evidence':[e['id']],'outcome_evidence':[]}],'evidence':[e['id']],'confidence':0.8}], 'tasks':[], 'summary':'Synthetic visit'}
if mode=='attention':
 lanes=data['attention']
 span=next(s for lane in lanes for s in lane['intervals'] if s['label']=='probably_away')
 assert span['attrs']['basis']=='inferred'
 supports=json.loads(next(line.split('=',1)[1] for line in data['timeline'].splitlines() if line.startswith('time_support=')))
 assert span['interval'] in supports
 ids=span['evidence']
 assert ids and set(ids)<=set(e['id'] for e in data['attention_evidence'])
 assert all('payload' not in e['text'] for e in data['attention_evidence'])
 second=rpc('tools/call',{'name':'timeline','arguments':{'from':start,'to':'2026-01-01T04:00:00Z'}})
 assert json.loads(second['content'][0]['text'])['attention']==lanes
 episode=answer['episodes'][0]
 episode.update(intervals=[span['interval']],title='Probably away',description='Inferred absence from healthy capture',evidence=ids)
 episode['claims'][0].update(action='was probably away',object='Mac',basis='inferred',evidence=ids)
 answer['summary']='Probably away'
if mode=='invalid': answer='this is not JSON'
if mode=='empty-repair':
 counter=pathlib.Path('COUNTER')
 if counter.exists(): answer={'episodes':[],'tasks':[],'summary':''}
 else:
  counter.write_text('1')
  bad=json.loads(json.dumps(answer['episodes'][0])); bad['id']='synthetic-unknown'; bad['evidence']=[999999999]
  bad['claims'][0]['evidence']=[999999999]; answer['episodes'].append(bad)
if mode=='merge-repair':
 counter=pathlib.Path('COUNTER')
 good=answer['episodes'][0]
 fixed=json.loads(json.dumps(good)); fixed['id']='synthetic-second'; fixed['title']='Second visit'
 if counter.exists():
  assert 'Repair only these episodes' in instr and '<digest>' in instr and 'synthetic-second' in instr
  assert 'synthetic-episode' not in instr.split('Rejected episodes:')[1]
  intruder=json.loads(json.dumps(good)); intruder['id']='intruder'
  answer={'episodes':[fixed,intruder],'tasks':[],'summary':''}
 else:
  counter.write_text('1')
  bad=json.loads(json.dumps(fixed)); bad['evidence']=[999999999]; bad['claims'][0]['evidence']=[999999999]
  answer['episodes'].append(bad)
  answer['tasks']=[{'id':'synthetic-task','title':'Visits','description':'Two visits','intervals':good['intervals'],'episode_ids':['synthetic-episode','synthetic-second'],'goal':None,'aliases':[],'confidence':0.7,'open':False}]
if mode=='repair-failure':
 counter=pathlib.Path('COUNTER')
 if counter.exists(): print(json.dumps({'error':'synthetic repair failure','exit_code':1}));sys.exit(1)
 counter.write_text('1'); answer['episodes'][0]['claims'][0]['outcome']='completed'
print(json.dumps({'exit_code':0,'model':'gpt-6.1-sol','steps':2,'final_output':answer if isinstance(answer,str) else json.dumps(answer),'tool_calls':[{'name':'mcp_hyperthymesia_window_overview','status':'success'}]}))
"#;
    std::fs::write(
        &binary,
        script
            .replace("MODE", mode)
            .replace("COUNTER", dir.join("counter").to_str().unwrap()),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let credential = dir.join("synthetic-auth.json");
    std::fs::write(&credential, "{}").unwrap();
    hx_server::fx::Config {
        binary,
        credential,
        timeout: std::time::Duration::from_secs(10),
        max_steps: hx_server::fx::DEFAULT_MAX_STEPS,
    }
}

#[tokio::test]
#[cfg(unix)]
async fn fx_attention_intervals_are_citable_time_support() {
    let f = Fixture::new().await;
    let base = event("attention-base", 1).time.observed_at;
    let capture = |key: &str, seconds: i64, kind: &str, payload: Value| {
        let mut e = event(key, 1);
        e.device = Some(hx_schema::DeviceId::new("synthetic-device"));
        e.kind = Kind::new(kind);
        e.time = ObservationTime::point(base + Duration::seconds(seconds));
        e.payload = payload;
        e
    };
    let mut events = vec![
        capture(
            "focus",
            0,
            "focus.span",
            json!({"app_bundle_id":"synthetic.app","app_name":"Synthetic","window_title":"Stuck page"}),
        ),
        capture(
            "input-first",
            0,
            "input.bucket",
            json!({"bucket_ms":1000,"keys":1,"clicks":0,"mouse_px":0,"scroll_events":0}),
        ),
        capture(
            "input-last",
            240,
            "input.bucket",
            json!({"bucket_ms":1000,"keys":1,"clicks":0,"mouse_px":0,"scroll_events":0}),
        ),
        still("attention-still", 0, Sensitivity::Normal, &synthetic_png(0)),
    ];
    for second in (0..270).step_by(30) {
        for collector in ["input", "focus", "media"] {
            events.push(capture(
                &format!("health-{collector}-{second}"),
                second,
                "collector.health",
                json!({"collector":collector,"status":"running","dropped":0}),
            ));
        }
    }
    // More than 32 records ensures attention does not use the raw response sample.
    for second in 1..240 {
        events.push(capture(
            &format!("zero-{second}"),
            second,
            "input.bucket",
            json!({"bucket_ms":1000,"keys":0,"clicks":0,"mouse_px":0,"scroll_events":0}),
        ));
    }
    ingest(&f.store, events).await;
    let w = hx_store::window_at(base);
    f.store.normalize_window(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = fake_fx(scratch.path(), "attention");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-attention", &config)
            .await
            .unwrap()
    );
    let row = f.store.attention_revision().await.unwrap();
    let value: Value = row.get("interpretation");
    let run: Value = row.get("agent_run");
    assert_eq!(value["episodes"].as_array().unwrap().len(), 1, "{value}");
    assert_eq!(value["episodes"][0]["claims"][0]["basis"], "inferred");
    assert_eq!(value["check_failures"], json!([]));
    assert_eq!(
        run["calls"].as_array().unwrap().len(),
        2,
        "validation should need no repair turn"
    );
    assert_eq!(value["input_manifest"]["prompt_version"], "hx-replica/v9");

    // Gap payloads stay private, but the interrupted time must be unknown.
    let mut gap = capture(
        "private-gap",
        30,
        "policy.gap",
        json!({"reason":"SYNTHETIC_GAP_CANARY"}),
    );
    gap.time.source_end = Some(base + Duration::seconds(240));
    gap.sensitivity = Sensitivity::Restricted;
    ingest(&f.store, vec![gap]).await;
    let broker = hx_server::mcp::Broker::new(
        f.store.clone(),
        hx_server::disclosure::DisclosurePolicy::default(),
        hx_server::processing::SensitiveEgress::Full,
        w,
    );
    let args = json!({"from":base,"to":base + Duration::seconds(270)});
    let result = broker.call("timeline", args.clone()).await.unwrap();
    assert!(!result.to_string().contains("SYNTHETIC_GAP_CANARY"));
    let lanes: Vec<hx_compile::Lane> = serde_json::from_value(result["attention"].clone()).unwrap();
    let interrupted = lanes
        .iter()
        .flat_map(|lane| &lane.intervals)
        .find(|span| span.interval.contains(base + Duration::seconds(120)))
        .unwrap();
    assert_eq!(interrupted.label, "unknown");

    // A capped scan cannot certify silence: all returned attention stays unknown.
    let dense: Vec<_> = (0..2100)
        .map(|i| {
            let mut e = event(&format!("attention-dense-{i}"), 1);
            e.time = ObservationTime::point(base + Duration::milliseconds(i));
            e
        })
        .collect();
    ingest(&f.store, dense).await;
    let result = broker.call("window_overview", args).await.unwrap();
    let lanes: Vec<hx_compile::Lane> = serde_json::from_value(result["attention"].clone()).unwrap();
    assert!(!lanes.is_empty());
    assert!(
        lanes
            .iter()
            .flat_map(|lane| &lane.intervals)
            .all(|span| span.label == "unknown")
    );
    f.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn fx_validation_supports_citations_beyond_the_exploration_row_cap() {
    let f = Fixture::new().await;
    // More than the 2,000-row exploration cap, all before the cited record.
    let mut envelopes: Vec<Envelope> = (0..2100)
        .map(|i| {
            let mut e = event(&format!("fx-dense-{i}"), 1);
            e.time = ObservationTime::point(
                Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap()
                    + chrono::Duration::milliseconds(i),
            );
            e
        })
        .collect();
    let mut late = event("fx-late", 3);
    late.time = ObservationTime::point(Utc.with_ymd_and_hms(2026, 1, 1, 3, 59, 0).unwrap());
    envelopes.push(late);
    ingest(&f.store, envelopes).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    f.store.normalize_window(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = fake_fx(scratch.path(), "late");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-late", &config)
            .await
            .unwrap()
    );
    let value: Value = f.store.fx_late_interpretation().await.unwrap();
    assert_eq!(
        value["episodes"].as_array().unwrap().len(),
        1,
        "{}",
        value["check_failures"]
    );
    f.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn fx_job_success_audit_citations_failure_timeout_and_repair_retention() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("fx-visible", 1)]).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    f.store.normalize_window(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = fake_fx(scratch.path(), "success");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-success", &config)
            .await
            .unwrap()
    );
    let previous: i64 = f
        .store
        .required_canonical_revision_at(w.start)
        .await
        .unwrap();
    let row = f.store.revision_contents(previous).await.unwrap();
    let value: Value = row.get("interpretation");
    let run: Value = row.get("agent_run");
    assert_eq!(value["episodes"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["input_manifest"]["input_namespaces"],
        json!(["test:device"])
    );
    assert_eq!(run["fx_version"], "0.0.16");
    assert_eq!(run["effort"], "xhigh");
    assert_eq!(run["fast"], true);
    let id = value["episodes"][0]["evidence"][0].as_i64().unwrap();
    assert_eq!(run["evidence"][id.to_string()]["id"], id);
    assert_eq!(
        run["calls"][0]["result"]["evidence"][0],
        run["evidence"][id.to_string()]
    );
    let secret = f
        .store
        .create_token(
            "synthetic dashboard",
            vec!["test:".into()],
            vec!["read_raw".into()],
        )
        .await
        .unwrap();
    for path in [
        format!("/revision/{previous}/run"),
        format!("/revision/{previous}/evidence/{id}"),
    ] {
        let response = hx_server::router(f.store.clone(), DisclosurePolicy::default())
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header("authorization", format!("Bearer {secret}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("synthetic"));
    }
    // An identical revision skips even when launching fx would fail.
    let mut unavailable = config.clone();
    unavailable.binary = scratch.path().join("does-not-exist");
    assert!(
        !hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-duplicate", &unavailable)
            .await
            .unwrap()
    );
    assert_eq!(count(&f.store, "revisions").await, 1);
    // Exercise failures on genuinely new input; identical input now short-circuits.
    f.store.advance_window_generation(w.start).await.unwrap();
    for mode in ["invalid", "failure", "timeout", "rate", "rate-retrying"] {
        let mut config = fake_fx(scratch.path(), mode);
        if mode == "timeout" {
            config.timeout = std::time::Duration::from_millis(100);
        }
        let started = std::time::Instant::now();
        let result =
            hx_server::fx::interpret_window(&f.store, w, &format!("synthetic-fx-{mode}"), &config)
                .await;
        assert!(result.is_err(), "{mode}");
        // A retrying usage limit must be noticed at once, not after the fx timeout.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(8),
            "{mode}"
        );
        if mode.starts_with("rate") {
            assert!(matches!(
                result.unwrap_err().downcast_ref::<hx_jobs::Deferred>(),
                Some(hx_jobs::Deferred::RateLimited)
            ));
        }
        let canonical: i64 = f
            .store
            .required_canonical_revision_at(w.start)
            .await
            .unwrap();
        assert_eq!(canonical, previous, "{mode} replaced served revision");
    }
    let generation = f.store.generation(w).await.unwrap();
    f.store.advance_window_generation(w.start).await.unwrap();
    assert!(
        !f.store
            .publish_revision_manifest(
                w,
                generation,
                json!({"episodes":[],"tasks":[],"agent_run":{}}),
                "synthetic-stale-agent",
                &[]
            )
            .await
            .unwrap()
            .1
    );
    let stale: i64 = f.store.stale_agent_revision_count().await.unwrap();
    assert_eq!(stale, 0);
    let config = fake_fx(scratch.path(), "repair-failure");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-repaired", &config)
            .await
            .unwrap()
    );
    let value: Value = f.store.fx_repaired_interpretation().await.unwrap();
    assert_eq!(value["episodes"][0]["claims"][0]["outcome"], "attempted");
    assert!(!value["check_failures"].as_array().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn fx_keeps_the_fuller_answer_when_a_repair_comes_back_empty() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("fx-empty-repair", 1)]).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    f.store.normalize_window(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = fake_fx(scratch.path(), "empty-repair");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-empty-repair", &config)
            .await
            .unwrap()
    );
    let value: Value = f.store.fx_empty_repair_interpretation().await.unwrap();
    // The rejected episode triggered one repair; the empty repair must not replace the first answer.
    assert_eq!(value["episodes"].as_array().unwrap().len(), 1, "{value}");
    assert!(std::fs::metadata(scratch.path().join("counter")).is_ok());
    f.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn fx_repair_replaces_only_rejected_episodes_and_keeps_the_rest() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("fx-merge-repair", 1)]).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    f.store.normalize_window(w).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = fake_fx(scratch.path(), "merge-repair");
    assert!(
        hx_server::fx::interpret_window(&f.store, w, "synthetic-fx-merge-repair", &config)
            .await
            .unwrap()
    );
    let value: Value = f.store.fx_merge_repair_interpretation().await.unwrap();
    let ids: Vec<&str> = value["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["synthetic-episode", "synthetic-second"], "{value}");
    assert_eq!(value["tasks"][0]["id"], "synthetic-task", "{value}");
    f.close().await;
}

#[tokio::test]
async fn fx_live_synthetic_only_when_explicitly_enabled() {
    if std::env::var("HX_LIVE_FX").as_deref() != Ok("1") {
        return;
    }
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("synthetic-live-fx", 1)]).await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    f.store.normalize_window(w).await.unwrap();
    let broker = hx_server::mcp::Broker::new(
        f.store.clone(),
        hx_server::disclosure::DisclosurePolicy::default(),
        hx_server::processing::SensitiveEgress::Full,
        w,
    );
    let result=hx_server::fx::run(&hx_server::fx::Config::from_env(),&broker,"SYNTHETIC FIXTURE ONLY. Use window_overview from 2026-01-01T00:00:00Z to 2026-01-01T04:00:00Z. Then return JSON episodes:[], tasks:[], summary:'Synthetic fixture inspected'. Do not call other tools except capability_search or mcp_select_tool to find Hyperthymesia tools.").await.unwrap();
    assert_eq!(result["model"], "gpt-6.1-sol");
    assert!(
        !broker.audit.lock().await.calls.is_empty(),
        "real fx must reach the synthetic MCP server"
    );
    f.close().await;
}

#[tokio::test]
async fn mcp_ocr_only_dense_results_new_sources_and_prior_revision_filter() {
    use hx_server::{disclosure::DisclosurePolicy, mcp::Broker, processing::SensitiveEgress};
    let f = Fixture::new().await;
    let w = hx_store::window_at(Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    let bytes = synthetic_png(0);
    ingest(
        &f.store,
        vec![still("mcp-ocr", 0, Sensitivity::Normal, &bytes)],
    )
    .await;
    f.store
        .upload_blob(&hx_store::hash(&bytes), &bytes)
        .await
        .unwrap();
    drain_ocr(&f.store, SyntheticOcr::new(false), "synthetic-mcp/v1").await;
    let broker = Broker::new(
        f.store.clone(),
        DisclosurePolicy::default(),
        SensitiveEgress::Full,
        w,
    );
    let result = broker
        .call("screen_at", json!({"time":w.start+Duration::hours(1)}))
        .await
        .unwrap();
    let ocr = result["evidence"][0].clone();
    assert_eq!(ocr["kind"], "normal:ocr.text");
    assert!(ocr["text"].as_str().unwrap().contains("SYNTHETIC ORCHID"));
    assert!(!result.to_string().contains("image/png"));
    let id = ocr["id"].as_i64().unwrap();
    let read = broker
        .call("read_evidence", json!({"id":id}))
        .await
        .unwrap();
    assert_eq!(read["evidence"][0], ocr);
    // A new kind stays withheld until a rule grants it, and then needs no prompt or tool changes.
    let mut new_source = event("mcp-new-source", 1);
    new_source.kind = Kind::new("synthetic.document");
    new_source.payload = json!({"text":"synthetic comet discovery"});
    ingest(&f.store, vec![new_source]).await;
    let withheld = broker
        .call("search", json!({"query":"comet discovery"}))
        .await
        .unwrap();
    assert!(withheld["evidence"].as_array().unwrap().is_empty());
    let grant = DisclosurePolicy::from_json(
        r#"{"rules":[{"connector_prefix":"test:","kind":"synthetic.document","class":"normal"}]}"#,
    )
    .unwrap();
    let grant_digest = grant.digest();
    let granted = Broker::new(f.store.clone(), grant, SensitiveEgress::Full, w);
    let search = granted
        .call("search", json!({"query":"comet discovery"}))
        .await
        .unwrap();
    assert_eq!(search["evidence"].as_array().unwrap().len(), 1);
    let visible = search["evidence"][0]["id"].as_i64().unwrap();
    let sources = granted.call("list_sources", json!({})).await.unwrap();
    assert!(sources.to_string().contains("synthetic.document"));
    let generation = f.store.generation(w).await.unwrap();
    f.store.publish_revision(w,generation,json!({"episodes":[{"evidence":[visible]}],"tasks":[],"summary":"synthetic prior","input_manifest":{"disclosure_policy":{"digest":grant_digest},"sensitive_egress":"full"}})).await.unwrap();
    assert_eq!(
        granted.prior_context(w.end).await.unwrap()["revisions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let deny=DisclosurePolicy::from_json(r#"{"rules":[{"connector_prefix":"test:","kind":"synthetic.document","class":"restricted"}]}"#).unwrap();
    let denied = Broker::new(f.store.clone(), deny, SensitiveEgress::Full, w);
    assert!(
        denied.prior_context(w.end).await.unwrap()["revisions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut dense = vec![];
    for i in 0..40 {
        let mut e = event(&format!("mcp-dense-{i}"), 2);
        e.payload["title"] = json!(format!("synthetic dense {}", "🌸".repeat(4000)));
        dense.push(e);
    }
    ingest(&f.store, dense).await;
    let result = broker
        .call("search", json!({"query":"synthetic dense"}))
        .await
        .unwrap();
    assert_eq!(result["truncated"], true);
    assert!(serde_json::to_vec(&result).unwrap().len() <= hx_server::mcp::MAX_RESULT_BYTES);
    for item in result["evidence"].as_array().unwrap() {
        assert!(serde_json::from_str::<Value>(item["text"].as_str().unwrap()).is_ok());
    }
    f.close().await;
}

struct CountingProvider(std::sync::atomic::AtomicUsize);
impl hx_interpret::Provider for CountingProvider {
    fn name(&self) -> String {
        "counting/model-v1".into()
    }
    async fn complete(&self, _: &str, _: &str) -> Result<String, hx_interpret::InterpretError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!({"episodes":[],"tasks":[],"summary":"Synthetic."}).to_string())
    }
}

async fn compile_for_test(
    runner: &hx_server::processing::Runner<CountingProvider>,
    w: hx_schema::Interval,
) {
    runner.store.normalize_window(w).await.unwrap();
    hx_jobs::Handler::handle(
        runner,
        &hx_jobs::Job {
            id: 0,
            kind: "compile".into(),
            payload: json!({"window":w}),
            owner: "synthetic".into(),
            attempts: 1,
        },
    )
    .await
    .unwrap();
    runner.store.finish_free_jobs().await.unwrap();
}
async fn settle_for_test(store: &Store) {
    store.settle_test_inputs().await.unwrap();
    store.make_ready_interpretations_due().await.unwrap();
}

#[tokio::test]
async fn interpretation_storm_coalesces_and_late_data_waits_for_rerun_policy() {
    use std::sync::atomic::Ordering;
    let f = Fixture::new().await;
    let w = hx_store::window_at(event("batch", 8).time.observed_at);
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: CountingProvider(std::sync::atomic::AtomicUsize::new(0)),
    };
    for i in 0..108 {
        ingest(&f.store, vec![event(&format!("batch-{i}"), 8)]).await;
        compile_for_test(&runner, w).await;
    }
    let slots: i64 = f.store.ready_interpretation_count().await.unwrap();
    assert_eq!(
        slots, 0,
        "historical generations cannot buy primary or 24h slots"
    );
    f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
    let g = f.store.generation(w).await.unwrap();
    let generations: Vec<i64> = f.store.interpretation_generations().await.unwrap();
    assert_eq!(generations, vec![g], "the sweep coalesces to one paid slot");
    // Even a capped sweep must wait for the uploaded input to settle.
    f.store.make_ready_interpretations_due().await.unwrap();
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert_eq!(runner.provider.0.load(Ordering::SeqCst), 0);
    let attempts: i32 = f.store.interpretation_attempts().await.unwrap();
    assert_eq!(attempts, 0);
    settle_for_test(&f.store).await;
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert!(!hx_jobs::run_one(&f.store, &runner).await.unwrap()); // no uncapped 24h slot
    assert_eq!(runner.provider.0.load(Ordering::SeqCst), 1);
    // Manual and old queued requests use the same pre-provider identity check.
    assert!(
        !hx_server::processing::interpret_window(&f.store, w, &runner.provider)
            .await
            .unwrap()
    );
    assert_eq!(runner.provider.0.load(Ordering::SeqCst), 1);
    assert_eq!(count(&f.store, "revisions").await, 1);
    for i in 108..120 {
        ingest(&f.store, vec![event(&format!("batch-{i}"), 8)]).await;
        compile_for_test(&runner, w).await;
    }
    settle_for_test(&f.store).await;
    assert!(!hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert_eq!(runner.provider.0.load(Ordering::SeqCst), 1);
    assert_eq!(count(&f.store, "revisions").await, 1);
    let published: i64 = f.store.published_generation(w.start).await.unwrap();
    assert_eq!(published, g);
    assert!(published < f.store.generation(w).await.unwrap());
    f.close().await;
}

#[tokio::test]
async fn interpretation_slot_preserves_lease_and_recovers_new_generation() {
    let f = Fixture::new().await;
    // Keep the primary age-eligible while testing in-flight generation fencing.
    let w = hx_store::window_at(Utc::now() - Duration::hours(4));
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: CountingProvider(std::sync::atomic::AtomicUsize::new(0)),
    };
    let mut first = event("lease-1", 8);
    first.time = ObservationTime::point(w.start);
    ingest(&f.store, vec![first]).await;
    compile_for_test(&runner, w).await;
    settle_for_test(&f.store).await;
    let job = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    let mut second = event("lease-2", 8);
    second.time = ObservationTime::point(w.start);
    ingest(&f.store, vec![second]).await;
    compile_for_test(&runner, w).await;
    assert!(hx_jobs::heartbeat(&f.store, &job, 120).await.unwrap());
    assert!(hx_jobs::finish(&f.store, &job, None).await.unwrap());
    let row = f.store.lease_versions(job.id).await.unwrap();
    assert_eq!(row.get::<String, _>("state"), "ready");
    assert!(row.get::<i64, _>("request_version") > row.get::<i64, _>("leased_version"));
    settle_for_test(&f.store).await;
    let recovered = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
    assert_eq!(job.id, recovered.id);
    assert_eq!(
        recovered.attempts, 2,
        "generation refresh preserves spent attempts"
    );
    assert!(!hx_jobs::finish(&f.store, &job, None).await.unwrap());
    hx_jobs::Handler::handle(&runner, &recovered).await.unwrap();
    hx_jobs::finish(&f.store, &recovered, None).await.unwrap();
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn revision_identity_includes_prompt_model_and_disclosure() {
    for field in ["prompt_version", "provider", "policy", "sensitive_egress"] {
        let f = Fixture::new().await;
        let w = hx_store::window_at(event("identity", 8).time.observed_at);
        ingest(&f.store, vec![event("identity-input", 8)]).await;
        f.store.normalize_window(w).await.unwrap();
        let provider = CountingProvider(std::sync::atomic::AtomicUsize::new(0));
        let prepared = hx_server::processing::prepare_input(&f.store, w, "counting/model-v1")
            .await
            .unwrap();
        let mut manifest = prepared.manifest;
        match field {
            "policy" => manifest["disclosure_policy"]["digest"] = json!("older-policy"),
            "provider" => manifest["provider"] = json!("counting/model-v0"),
            "sensitive_egress" => manifest["sensitive_egress"] = json!("redact"),
            _ => manifest[field] = json!("older-prompt"),
        }
        f.store
            .publish_revision_key(
                w,
                prepared.generation,
                json!({"episodes":[],"tasks":[],"input_manifest":manifest}),
                "older-config",
            )
            .await
            .unwrap();
        assert!(
            hx_server::processing::interpret_window(&f.store, w, &provider)
                .await
                .unwrap(),
            "{field}"
        );
        assert!(
            !hx_server::processing::interpret_window(&f.store, w, &provider)
                .await
                .unwrap()
        );
        assert_eq!(
            provider.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{field}"
        );
        f.close().await;
    }
}

#[tokio::test]
async fn interpretation_cadences_wait_for_window_end_and_day_boundary() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(Utc::now());
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: CountingProvider(std::sync::atomic::AtomicUsize::new(0)),
    };
    let mut input = event("cadence-input", 8);
    input.time = hx_schema::ObservationTime::point(w.start);
    ingest(&f.store, vec![input]).await;
    compile_for_test(&runner, w).await;
    settle_for_test(&f.store).await;
    // Force early claims to exercise the handler guard independently of queue dates.
    for _ in 0..2 {
        assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    }
    assert_eq!(
        runner.provider.0.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let rows = f.store.interpretation_requests().await.unwrap();
    for row in rows {
        let payload: Value = row.get("payload");
        let due: chrono::DateTime<Utc> = row.get("available_at");
        assert_eq!(
            due,
            if payload["revision24"] == true {
                w.end + Duration::hours(24)
            } else {
                w.end
            }
        );
        assert_eq!(row.get::<i32, _>("attempts"), 0);
    }
    f.close().await;
}

// A subprocess scopes fx configuration to this regression, without mutating the
// multithreaded test runner's environment or opening a real credential.
#[tokio::test]
#[cfg(unix)]
async fn close_launched_fx_errors_spend_paid_budgets() {
    const CASE_ENV: &str = "HX_SYNTHETIC_PAID_CASE";
    let Ok(case) = std::env::var(CASE_ENV) else {
        let mut failures = vec![];
        for cadence in ["sweep", "primary", "24h"] {
            for mode in ["rate", "generate", "rate-retrying"] {
                let scratch = tempfile::tempdir().unwrap();
                let config = fake_fx(scratch.path(), mode);
                let status = tokio::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "close_launched_fx_errors_spend_paid_budgets",
                        "--nocapture",
                    ])
                    .env(CASE_ENV, format!("{cadence}:{mode}"))
                    .env("HX_FX_BINARY", config.binary)
                    .env("HX_FX_CREDENTIAL", config.credential)
                    .env("HX_FX_TIMEOUT_S", "5")
                    .env("HX_SYNTHETIC_PAID_CALLS", scratch.path().join("paid-calls"))
                    .status()
                    .await
                    .unwrap();
                if !status.success() {
                    failures.push(format!("{cadence}:{mode}"));
                }
            }
        }
        assert!(failures.is_empty(), "failed fake fx cases: {failures:?}");
        return;
    };
    struct ToolAgent;
    impl hx_interpret::Provider for ToolAgent {
        fn name(&self) -> String {
            "synthetic-fx".into()
        }
        fn tool_agent(&self) -> bool {
            true
        }
        async fn complete(&self, _: &str, _: &str) -> Result<String, hx_interpret::InterpretError> {
            panic!("only the isolated fake fx may run")
        }
    }
    let (cadence, mode) = case.split_once(':').unwrap();
    let f = Fixture::new().await;
    let w = if cadence == "sweep" {
        hx_store::window_at(event("old", 8).time.observed_at)
    } else {
        hx_store::window_at(Utc::now() - Duration::hours(if cadence == "24h" { 32 } else { 4 }))
    };
    let mut first = event("paid-error", 8);
    first.time = ObservationTime::point(w.start);
    ingest(&f.store, vec![first.clone()]).await;
    f.store.normalize_window(w).await.unwrap();
    if cadence == "24h" {
        hx_server::processing::interpret_window(&f.store, w, &hx_server::processing::StubProvider)
            .await
            .unwrap();
        first.source_key = SourceKey::new("paid-error-change");
        first.observation_id = ObservationId::generate();
        ingest(&f.store, vec![first]).await;
        f.store.normalize_window(w).await.unwrap();
    }
    let (_, g) = hx_server::processing::compile_window(&f.store, w)
        .await
        .unwrap();
    f.store.finish_all_jobs().await.unwrap();
    if cadence == "sweep" {
        f.store.schedule_daily_sweep(Utc::now()).await.unwrap();
    } else {
        f.store
            .request_interpretation(w, g, cadence == "24h")
            .await
            .unwrap();
    }
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: ToolAgent,
    };
    let calls_path = std::path::PathBuf::from(std::env::var("HX_SYNTHETIC_PAID_CALLS").unwrap());
    for run in 0..6 {
        settle_for_test(&f.store).await;
        let result = hx_jobs::run_one(&f.store, &runner).await;
        if run == 0 {
            assert_eq!(
                result.is_ok(),
                mode != "generate",
                "generate is a provider failure, not a rate limit"
            );
            let job = f
                .store
                .job_states()
                .await
                .unwrap()
                .into_iter()
                .find(|j| j.kind == "interpret")
                .unwrap();
            assert_eq!(job.attempts, 1, "launched fx spends the attempt for {case}");
            if cadence == "sweep" {
                assert!(
                    f.store.sweep_reservations().await.unwrap()[0].2,
                    "a launched fx error keeps the sweep start spent"
                );
            }
        }
    }
    let calls = std::fs::read_to_string(calls_path).unwrap().lines().count();
    assert_eq!(
        calls,
        if cadence == "sweep" { 1 } else { 5 },
        "fx launches for {case}"
    );
    if cadence == "sweep" {
        f.store.finish_all_jobs().await.unwrap();
        f.store
            .schedule_daily_sweep(Utc::now() + Duration::days(1))
            .await
            .unwrap();
        assert_eq!(
            f.store.sweep_reservations().await.unwrap().len(),
            1,
            "seven-day cooldown"
        );
        f.store
            .schedule_daily_sweep(Utc::now() + Duration::days(7))
            .await
            .unwrap();
        assert_eq!(f.store.sweep_reservations().await.unwrap().len(), 2);
    }
    f.close().await;
}

#[tokio::test]
async fn recent_interpretation_storm_coalesces_108_generations_into_one_paid_run() {
    use std::sync::atomic::Ordering;
    let f = Fixture::new().await;
    let w = hx_store::window_at(Utc::now() - Duration::hours(4));
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: CountingProvider(std::sync::atomic::AtomicUsize::new(0)),
    };
    for i in 0..108 {
        let mut e = event(&format!("recent-storm-{i}"), 8);
        e.time = ObservationTime::point(w.start);
        ingest(&f.store, vec![e]).await;
        compile_for_test(&runner, w).await;
    }
    let g = f.store.generation(w).await.unwrap();
    let slots: Vec<i64> = f.store.ready_interpretation_generations().await.unwrap();
    assert_eq!(
        slots,
        vec![g, g],
        "one primary and one future 24h slot coalesce to the latest generation"
    );
    // Force queue readiness only: the handler still enforces the settle gate.
    f.store.make_all_interpretations_due().await.unwrap();
    assert!(hx_jobs::run_one(&f.store, &runner).await.unwrap());
    assert_eq!(runner.provider.0.load(Ordering::SeqCst), 0);
    settle_for_test(&f.store).await;
    while hx_jobs::run_one(&f.store, &runner).await.unwrap() {}
    assert_eq!(runner.provider.0.load(Ordering::SeqCst), 1);
    assert_eq!(count(&f.store, "revisions").await, 1);
    let published: i64 = f.store.published_generation(w.start).await.unwrap();
    assert_eq!(published, g);
    assert!(f.store.sweep_reservations().await.unwrap().is_empty());
    f.close().await;
}
