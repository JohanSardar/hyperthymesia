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
use hx_store::{Store, Token, UploadBatch};
use serde_json::{Value, json};
use sqlx::{Row, postgres::PgPoolOptions};
use tower::ServiceExt;

struct Fixture {
    store: Store,
    admin: sqlx::PgPool,
    schema: String,
    _dir: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("HX_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://hx:hx@localhost:55432/hx".into());
        let admin = sqlx::PgPool::connect(&url)
            .await
            .expect("test Postgres must be running");
        let schema = format!("test_{}", ulid::Ulid::generate().to_string().to_lowercase());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |conn, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query("SELECT set_config('search_path',$1,false)")
                        .bind(&search)
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(pool, dir.path());
        store.migrate().await.unwrap();
        Self {
            store,
            admin,
            schema,
            _dir: dir,
        }
    }
    async fn close(self) {
        self.store.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .unwrap();
        self.admin.close().await;
    }
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
    sqlx::query_scalar(match table {
        "events" => "SELECT count(*) FROM events",
        "jobs" => "SELECT count(*) FROM jobs",
        "quarantine" => "SELECT count(*) FROM quarantine",
        "normalized" => "SELECT count(*) FROM normalized",
        "revisions" => "SELECT count(*) FROM revisions",
        _ => panic!("unrecognized table"),
    })
    .fetch_one(&store.pool)
    .await
    .unwrap()
}
async fn request(
    store: &Store,
    method: &str,
    path: &str,
    secret: &str,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let response = hx_server::router(store.clone())
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
    sqlx::query("UPDATE windows SET dirty=false")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let mut e = event("late", 2);
    e.time.source_end = Some(e.time.observed_at + Duration::hours(8));
    ingest(&f.store, vec![e]).await;
    let rows = sqlx::query("SELECT dirty,generation FROM windows ORDER BY start_at")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.get::<bool, _>("dirty")));
    assert_eq!(rows[2].get::<i64, _>("generation"), 2);
    let watermark: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT observed_through FROM watermarks")
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
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
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    sqlx::query("UPDATE blobs SET created_at=now()-interval '31 days',retention_class=CASE WHEN sha256=$1 THEN 'keep' ELSE 'chunk' END").bind(&still).execute(&f.store.pool).await.unwrap();
    f.store.pin(&sha, true).await.unwrap();
    assert_eq!(f.store.gc().await.unwrap(), 0);
    f.store.pin(&sha, false).await.unwrap();
    assert_eq!(f.store.gc().await.unwrap(), 1);
    assert!(f.store.blob_path(&still).unwrap().exists());
    assert!(!f.store.blob_path(&sha).unwrap().exists());
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let canonical: i64 = sqlx::query_scalar("SELECT canonical_revision FROM windows")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM events")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
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
    let html = hx_server::dashboard::render_day(&timeline, &[], &[]).0;
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("https://"));
}
#[tokio::test]
async fn leases_fence_expired_workers_and_limit_attempts() {
    let f = Fixture::new().await;
    let mut tx = f.store.pool.begin().await.unwrap();
    hx_jobs::enqueue(
        &mut tx,
        "one",
        "test",
        json!({}),
        true,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    hx_jobs::enqueue(
        &mut tx,
        "one",
        "test",
        json!({}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let first = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert!(hx_jobs::claim(&f.store.pool, 120).await.unwrap().is_none());
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second'")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let next = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert_eq!(first.id, next.id);
    assert!(!hx_jobs::finish(&f.store.pool, &first, None).await.unwrap());
    assert!(hx_jobs::finish(&f.store.pool, &next, None).await.unwrap());
    assert!(hx_jobs::claim(&f.store.pool, 120).await.unwrap().is_none());
    let mut tx = f.store.pool.begin().await.unwrap();
    hx_jobs::enqueue(
        &mut tx,
        "two",
        "test",
        json!({}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE jobs SET max_attempts=1 WHERE output_key='two'")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let job = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    hx_jobs::finish(&f.store.pool, &job, Some("synthetic failure"))
        .await
        .unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE output_key='two'")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let value: Value = sqlx::query_scalar("SELECT interpretation FROM revisions")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(value["summary"], "original");
    assert!(
        sqlx::query("UPDATE revisions SET interpretation='{}'")
            .execute(&f.store.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM events")
            .execute(&f.store.pool)
            .await
            .is_err()
    );
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
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    f.store.normalize(id).await.unwrap();
    let (timeline, generation) = hx_server::processing::compile_window(&f.store, w)
        .await
        .unwrap();
    let row = sqlx::query("SELECT timeline,compiled_generation,dirty FROM windows")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let response = hx_server::router(f.store.clone())
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
    let stored: String = sqlx::query_scalar("SELECT hash FROM device_tokens")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_ne!(stored, key);
    sqlx::query("UPDATE device_tokens SET revoked=true")
        .execute(&f.store.pool)
        .await
        .unwrap();
    assert!(f.store.authenticate(&key).await.unwrap().is_none());
    f.close().await;
}
#[tokio::test]
async fn browser_signin_sets_a_private_cookie_and_renders_day() {
    let f = Fixture::new().await;
    let key = secret(&f.store).await;
    let response = hx_server::router(f.store.clone())
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
    let response = hx_server::router(f.store.clone())
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
    let windows: i64 = sqlx::query_scalar("SELECT count(*) FROM windows")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(windows, 2);
    f.close().await;
}
#[tokio::test]
async fn queue_prefers_interactive_and_enqueue_rolls_back() {
    let f = Fixture::new().await;
    let mut tx = f.store.pool.begin().await.unwrap();
    hx_jobs::enqueue(
        &mut tx,
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
    let mut tx = f.store.pool.begin().await.unwrap();
    hx_jobs::enqueue(
        &mut tx,
        "backfill",
        "test",
        json!({"queue":"backfill"}),
        true,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    hx_jobs::enqueue(
        &mut tx,
        "interactive",
        "test",
        json!({"queue":"interactive"}),
        false,
        event("due", 0).time.observed_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let job = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
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
    let partial = hx_server::timeline_value(&f.store, &token(), w, "episodes")
        .await
        .unwrap();
    assert_eq!(partial["revisions"], json!([]));
    let mut complete = token();
    complete.prefixes.push("other:".into());
    let full = hx_server::timeline_value(&f.store, &complete, w, "episodes")
        .await
        .unwrap();
    assert_eq!(full["revisions"].as_array().unwrap().len(), 1);
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
    let id: i64 = sqlx::query_scalar("SELECT min(id) FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    f.store
        .assert_entity("project", json!({"name":"synthetic"}), &[id])
        .await
        .unwrap();
    assert_eq!(f.store.generation(later).await.unwrap(), before + 2);
    f.close().await;
}
#[tokio::test]
async fn runner_normalizes_then_schedules_cadence_and_day_revision() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("runner", 8)]).await;
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    assert!(hx_jobs::run_one(&f.store.pool, &runner).await.unwrap());
    assert_eq!(count(&f.store, "normalized").await, 1);
    assert!(hx_jobs::run_one(&f.store.pool, &runner).await.unwrap());
    let rows = sqlx::query("SELECT output_key,queue,available_at FROM jobs WHERE kind='interpret'")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
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
    let rows = sqlx::query("SELECT id,kind,sensitivity FROM events")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
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
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM events ORDER BY id")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
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
    let canonical: i64 = sqlx::query_scalar("SELECT canonical_revision FROM windows")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(canonical, newer.0);
    f.close().await;
}

#[tokio::test]
async fn synthetic_day_burst() {
    synthetic_burst(false).await;
}

#[tokio::test]
async fn synthetic_browser_point_stress() {
    synthetic_burst(true).await;
}

async fn synthetic_burst(browser_points: bool) {
    let f = Fixture::new().await;
    let started = std::time::Instant::now();
    for chunk in 0..40 {
        let events = (chunk * 500..(chunk + 1) * 500)
            .map(|i| {
                let mut e = event(&format!("burst-{i}"), 0);
                e.time =
                    ObservationTime::point(e.time.observed_at + Duration::milliseconds(i * 4320));
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
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='compile' AND state IN ('ready','running')",
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    eprintln!("burst ingest={ingested:?}, live compile={live}");
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    assert!(live <= 7);
    let mut normalized_compiled = None;
    while hx_jobs::run_one(&f.store.pool, &runner).await.unwrap() {
        if normalized_compiled.is_none() {
            let pending: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM jobs WHERE kind IN ('normalize','compile') AND state!='done'",
            )
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
            if pending == 0 {
                normalized_compiled = Some(started.elapsed());
            }
        }
    }
    assert_eq!(count(&f.store, "normalized").await, 20_000);
    let unfinished: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE state!='done'")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(unfinished, 0);
    let canonical: i64 = sqlx::query_scalar("SELECT count(*) FROM windows w JOIN revisions r ON r.id=w.canonical_revision WHERE r.input_generation=w.generation").fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(canonical, 7);
    eprintln!("normalize+compile={normalized_compiled:?}");
    let stale: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM windows WHERE compiled_generation IS DISTINCT FROM generation",
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(stale, 0);
    eprintln!(
        "burst total={:?}, processing={:?}",
        started.elapsed(),
        started.elapsed() - ingested
    );
    assert!(started.elapsed().as_secs_f64() < 60.0);
    f.close().await;
}

#[tokio::test]
async fn coalesced_work_keeps_leases_and_requeues_new_input() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("first", 8)]).await;
    let first = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert_eq!(first.kind, "normalize");
    ingest(&f.store, vec![event("late", 8)]).await;
    assert!(
        hx_jobs::heartbeat(&f.store.pool, &first, 120)
            .await
            .unwrap()
    );
    assert!(hx_jobs::finish(&f.store.pool, &first, None).await.unwrap());
    let next = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert_eq!(first.id, next.id);
    assert_ne!(first.owner, next.owner);
    assert!(!hx_jobs::finish(&f.store.pool, &first, None).await.unwrap());
    assert!(
        !hx_jobs::defer(&f.store.pool, &first, &hx_jobs::Deferred::InputChanged)
            .await
            .unwrap()
    );
    // A newer request survives a crashed worker even on its last allowed attempt.
    sqlx::query("UPDATE jobs SET max_attempts=1 WHERE id=$1")
        .bind(next.id)
        .execute(&f.store.pool)
        .await
        .unwrap();
    ingest(&f.store, vec![event("newest", 8)]).await;
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(next.id)
        .execute(&f.store.pool)
        .await
        .unwrap();
    let newest = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    assert_eq!(newest.id, first.id);
    assert_eq!(newest.attempts, 1);
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    hx_jobs::Handler::handle(&runner, &newest).await.unwrap();
    hx_jobs::finish(&f.store.pool, &newest, None).await.unwrap();
    assert_eq!(count(&f.store, "normalized").await, 3);
    // Reopen a completed slot rather than append a second live job.
    ingest(&f.store, vec![event("after-done", 8)]).await;
    assert_eq!(
        hx_jobs::claim(&f.store.pool, 120)
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='compile' AND state IN ('ready','running')",
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(live, 1);
    f.close().await;
}

#[tokio::test]
async fn pending_overlap_defers_interpretation_without_spending_attempts() {
    let f = Fixture::new().await;
    let w = hx_store::window_at(event("target", 8).time.observed_at);
    let mut e = event("overlap", 7);
    e.time = ObservationTime::point(e.time.observed_at + Duration::minutes(45));
    ingest(&f.store, vec![e]).await;
    // Isolate the interpreter first so the actual overlap readiness guard is exercised.
    sqlx::query("UPDATE jobs SET available_at=now()+interval '1 hour'")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let mut tx = f.store.pool.begin().await.unwrap();
    hx_jobs::enqueue(
        &mut tx,
        "pending-interpret",
        "interpret",
        json!({"window":w}),
        false,
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE jobs SET max_attempts=1 WHERE output_key='pending-interpret'")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    for _ in 0..7 {
        assert!(hx_jobs::run_one(&f.store.pool, &runner).await.unwrap());
        let row = sqlx::query(
            "SELECT state,attempts,last_error FROM jobs WHERE output_key='pending-interpret'",
        )
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("state"), "ready");
        assert_eq!(row.get::<i32, _>("attempts"), 0);
        assert_eq!(row.get::<String, _>("last_error"), "normalization pending");
        sqlx::query("UPDATE jobs SET available_at=now() WHERE output_key='pending-interpret'")
            .execute(&f.store.pool)
            .await
            .unwrap();
    }
    f.store
        .normalize_window(hx_store::window_at(event("overlap", 7).time.observed_at))
        .await
        .unwrap();
    assert!(hx_jobs::run_one(&f.store.pool, &runner).await.unwrap());
    let state: String =
        sqlx::query_scalar("SELECT state FROM jobs WHERE output_key='pending-interpret'")
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
    assert_eq!(state, "done");
    let canonical: Option<i64> =
        sqlx::query_scalar("SELECT canonical_revision FROM windows WHERE start_at=$1")
            .bind(w.start)
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
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
    let mut tx = f.store.pool.begin().await.unwrap();
    hx_jobs::enqueue(&mut tx, "error", "test", json!({}), false, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(hx_jobs::run_one(&f.store.pool, &Failing).await.is_err());
    let error: String = sqlx::query_scalar("SELECT last_error FROM jobs")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert_eq!(error, "invalid job or derived JSON");
    assert!(error.len() < 128);
    f.close().await;
}

#[tokio::test]
async fn migration_supersedes_legacy_backlog_and_preserves_work() {
    let f = Fixture::new().await;
    ingest(&f.store, vec![event("legacy", 8)]).await;
    sqlx::query("DELETE FROM jobs")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let w = hx_store::window_at(event("legacy", 8).time.observed_at);
    let id: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    let mut tx = f.store.pool.begin().await.unwrap();
    for generation in 1..=10 {
        hx_jobs::enqueue(
            &mut tx,
            &format!("compile:{}:{generation}", w.start),
            "compile",
            json!({"window":w,"generation":generation}),
            false,
            Utc::now(),
        )
        .await
        .unwrap();
    }
    hx_jobs::enqueue(
        &mut tx,
        &format!("normalize:{id}"),
        "normalize",
        json!({"event_id":id}),
        false,
        Utc::now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let old = hx_jobs::claim(&f.store.pool, 120).await.unwrap().unwrap();
    sqlx::query("ALTER TABLE jobs DROP COLUMN request_version, DROP COLUMN leased_version")
        .execute(&f.store.pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../hx-store/migrations/0004_coalesced_work.sql"
    ))
    .execute(&f.store.pool)
    .await
    .unwrap();
    assert!(!hx_jobs::finish(&f.store.pool, &old, None).await.unwrap());
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='compile' AND state IN ('ready','running')",
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(live, 1);
    ingest(&f.store, vec![event("after-upgrade", 8)]).await;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='compile' AND state IN ('ready','running')",
    )
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
    assert_eq!(live, 1);
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::StubProvider,
    };
    while hx_jobs::run_one(&f.store.pool, &runner).await.unwrap() {}
    assert_eq!(count(&f.store, "normalized").await, 2);
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
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM events ORDER BY id")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
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
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM events")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
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
    let rows =
        sqlx::query("SELECT id,source_event_id,sensitivity,envelope FROM ocr_derived ORDER BY id")
            .fetch_all(&f.store.pool)
            .await
            .unwrap();
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
    let response = hx_server::router(f.store.clone())
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
    let sampled: i64 = sqlx::query_scalar("SELECT count(*) FROM ocr_jobs WHERE status='sampled' AND result IS NULL AND sampled_from IS NOT NULL").fetch_one(&f.store.pool).await.unwrap();
    assert_eq!(sampled, 1);
    let evidence: i64 = sqlx::query_scalar("SELECT count(*) FROM ocr_evidence")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
        sqlx::query("UPDATE ocr_jobs SET available_at=now() WHERE engine_version='broken/v1'")
            .execute(&f.store.pool)
            .await
            .unwrap();
    }
    assert!(
        !hx_server::ocr::run_one(&f.store, broken.clone(), "broken/v1")
            .await
            .unwrap()
    );
    assert_eq!(broken.calls(), 5);
    let status: String =
        sqlx::query_scalar("SELECT status FROM ocr_jobs WHERE engine_version='broken/v1'")
            .fetch_one(&f.store.pool)
            .await
            .unwrap();
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
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM ocr_jobs WHERE status='complete'")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
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
    let dirty: i64 =
        sqlx::query_scalar("SELECT count(*) FROM windows WHERE dirty AND start_at=ANY($1)")
            .bind(vec![w.start, next.start])
            .fetch_one(&f.store.pool)
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
