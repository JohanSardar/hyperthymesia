use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use hx_server::disclosure::DisclosurePolicy;
use hx_store::{Store, Ts};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    store: &Store,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn mint(dir: &std::path::Path) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .arg("--state-dir")
        .arg(dir)
        .args([
            "pair",
            "--name",
            "synthetic device",
            "--url",
            "http://127.0.0.1:7377",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pair command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The secret stays in memory and never appears in assertion diagnostics.
    let link = String::from_utf8(output.stdout).unwrap();
    link.trim()
        .split("&code=")
        .nth(1)
        .expect("pair link has code")
        .to_owned()
}

#[tokio::test]
async fn concurrent_redeems_issue_exactly_one_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let code = mint(dir.path());
    let other = Store::open(dir.path()).await.unwrap();
    let body = json!({"code":code,"device_id":"synthetic-mac"});
    let (a, b) = tokio::join!(
        request(&store, "POST", "/v1/pair", None, body.clone()),
        request(&other, "POST", "/v1/pair", None, body.clone())
    );
    let mut statuses = [a.0.as_u16(), b.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 410]);
    let token = if a.0 == StatusCode::OK {
        a.1["token"].as_str().unwrap()
    } else {
        b.1["token"].as_str().unwrap()
    };
    assert_eq!(token.len(), 64);
    assert!(store.authenticate(token).await.unwrap().is_some());
    let row: (String, Option<Ts>, Option<String>) =
        sqlx::query_as("SELECT hash,used_at,token_hash FROM pair_codes")
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(row.0 == hx_store::hash(code.as_bytes()));
    assert!(row.1.is_some());
    assert!(row.2.as_deref() == Some(hx_store::hash(token.as_bytes()).as_str()));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM device_tokens")
            .fetch_one(&store.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        request(&store, "POST", "/v1/pair", None, body).await.0,
        StatusCode::GONE
    );
}

#[tokio::test]
async fn expired_and_invalid_pairing_requests_uniformly_return_410() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let unknown = json!({"code":"ab".repeat(32),"device_id":"synthetic-mac"});
    let expected = request(&store, "POST", "/v1/pair", None, unknown).await;
    assert_eq!(expected.0, StatusCode::GONE);
    let code = mint(dir.path());
    sqlx::query("UPDATE pair_codes SET expires_at=$1")
        .bind(Ts::now())
        .execute(&store.pool)
        .await
        .unwrap();
    for body in [
        json!({"code":code,"device_id":"synthetic-mac"}),
        json!({"code":"bad","device_id":"synthetic-mac"}),
        json!({"code":code,"device_id":""}),
        json!({"code":code,"device_id":"synthetic-mac","scopes":["read_raw"]}),
        json!({}),
        json!({"code":"a".repeat(8192),"device_id":"synthetic-mac"}),
    ] {
        let actual = request(&store, "POST", "/v1/pair", None, body).await;
        assert!(
            actual == expected,
            "pairing failures must have the same status and body"
        );
    }
    let response = hx_server::router(store.clone(), DisclosurePolicy::default())
        .oneshot(
            Request::post("/v1/pair")
                .header("content-type", "application/json")
                .body(Body::from("{broken"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM device_tokens")
            .fetch_one(&store.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn paired_tokens_are_ingest_only_prefix_bound_and_revocable() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let code = mint(dir.path());
    let (status, body) = request(
        &store,
        "POST",
        "/v1/pair",
        None,
        json!({"code":code,"device_id":"synthetic-mac"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().unwrap();
    let credential = store.authenticate(token).await.unwrap().unwrap();
    assert_eq!(credential.scopes, ["ingest"]);
    assert_eq!(
        credential.prefixes,
        [
            "macos-agent:synthetic-mac",
            "macos-media:synthetic-mac",
            "companion:browser:synthetic-mac",
            "companion:editor:synthetic-mac",
            "companion:shell:synthetic-mac"
        ]
    );
    for route in [
        "/v1/timeline?from=2026-01-01T00:00:00Z&to=2026-01-01T04:00:00Z",
        "/v1/search?q=synthetic",
        "/v1/evidence/1",
        "/v1/entities",
        "/v1/now",
        "/v1/blobs/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert_eq!(
            request(&store, "GET", route, Some(token), Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN,
            "{route}"
        );
    }
    let event = |connector: &str| json!({"envelope_version":1,"observation_id":ulid::Ulid::generate(),"connector_instance":connector,"source_key":"synthetic","revision":"r1","operation":"observe","kind":"web.visit","schema_version":1,"time":{"observed_at":"2026-01-01T00:00:00Z","resolution_ms":1},"sensitivity":"normal","payload":{"url":"https://example.invalid"}});
    let batch = json!({"batch_id":ulid::Ulid::generate(),"envelopes":[event("macos-agent:synthetic-mac"),event("macos-agent:other-device"),event("git:synthetic-mac")]});
    let (status, ack) = request(&store, "POST", "/v1/events", Some(token), batch.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ack["accepted"], 1);
    assert_eq!(ack["rejected"].as_array().unwrap().len(), 2);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .arg("--state-dir")
        .arg(dir.path())
        .args(["token", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&code));
    let id = listed[0]["id"].as_str().unwrap();
    assert_eq!(listed[0]["name"], "synthetic device");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .arg("--state-dir")
        .arg(dir.path())
        .args(["token", "revoke", id])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        request(&store, "POST", "/v1/events", Some(token), batch)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

fn pairing_event(connector: &str) -> Value {
    json!({"envelope_version":1,"observation_id":ulid::Ulid::generate(),"connector_instance":connector,"source_key":"synthetic-boundary","revision":"r1","operation":"observe","kind":"web.visit","schema_version":1,"time":{"observed_at":"2026-01-01T00:00:00Z","resolution_ms":1},"sensitivity":"normal","payload":{"url":"https://example.invalid"}})
}

#[tokio::test]
async fn paired_prefixes_require_a_namespace_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let code = mint(dir.path());
    let (status, body) = request(
        &store,
        "POST",
        "/v1/pair",
        None,
        json!({"code":code,"device_id":"m"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().unwrap();
    let batch = json!({"batch_id":ulid::Ulid::generate(),"envelopes":[pairing_event("macos-agent:m"),pairing_event("macos-agent:m:sub"),pairing_event("macos-agent:mac1"),pairing_event("macos-agent:m-other")]});
    let (status, ack) = request(&store, "POST", "/v1/events", Some(token), batch).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ack["accepted"], 2);
    assert_eq!(ack["rejected"].as_array().unwrap().len(), 2);
    let broad = store
        .create_token(
            "synthetic broad",
            vec!["macos-agent:".into(), "t3code:".into()],
            vec!["ingest".into()],
        )
        .await
        .unwrap();
    let batch = json!({"batch_id":ulid::Ulid::generate(),"envelopes":[pairing_event("macos-agent:MacBook-Pro-1040.local"),pairing_event("t3code:synthetic")]});
    assert_eq!(
        request(&store, "POST", "/v1/events", Some(&broad), batch)
            .await
            .1["accepted"],
        2
    );
}

#[tokio::test]
async fn invalid_device_ids_do_not_burn_a_valid_code() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let code = mint(dir.path());
    for device in [
        ".",
        "..",
        "m:other",
        "m/other",
        "m mac",
        "é",
        "",
        "m\n",
        &"a".repeat(256),
    ] {
        assert_eq!(
            request(
                &store,
                "POST",
                "/v1/pair",
                None,
                json!({"code":code,"device_id":device})
            )
            .await
            .0,
            StatusCode::GONE,
            "invalid device ID"
        );
    }
    assert_eq!(
        request(
            &store,
            "POST",
            "/v1/pair",
            None,
            json!({"code":code,"device_id":"MacBook-Pro-1040.local"})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn live_device_cannot_be_paired_twice_and_refused_code_can_retry_after_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    // A legacy namespace-wide token does not claim this exact device.
    store
        .create_token(
            "synthetic legacy",
            vec!["macos-agent:".into()],
            vec!["ingest".into()],
        )
        .await
        .unwrap();
    let a = mint(dir.path());
    let b = mint(dir.path());
    let other = Store::open(dir.path()).await.unwrap();
    let body_a = json!({"code":a,"device_id":"synthetic-live"});
    let body_b = json!({"code":b,"device_id":"synthetic-live"});
    let (first, second) = tokio::join!(
        request(&store, "POST", "/v1/pair", None, body_a.clone()),
        request(&other, "POST", "/v1/pair", None, body_b.clone())
    );
    let mut statuses = [first.0.as_u16(), second.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 410]);
    let (winner, retry) = if first.0 == StatusCode::OK {
        (&first.1, body_b)
    } else {
        (&second.1, body_a)
    };
    let id = hx_store::hash(winner["token"].as_str().unwrap().as_bytes());
    assert!(store.revoke_token(&id).await.unwrap());
    assert_eq!(
        request(&store, "POST", "/v1/pair", None, retry).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn revoked_pair_code_returns_410_and_listing_never_exposes_the_code() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let code = mint(dir.path());
    let cli = || {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"));
        c.arg("--state-dir").arg(dir.path());
        c
    };
    let listed = cli().args(["pair", "list"]).output().unwrap();
    assert!(listed.status.success(), "pair list command missing");
    assert!(!String::from_utf8_lossy(&listed.stdout).contains(&code));
    let info: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let id = info[0]["id"].as_str().unwrap();
    assert!(id == hx_store::hash(code.as_bytes()));
    assert_eq!(info[0]["revoked"], false);
    assert!(
        cli()
            .args(["pair", "revoke", id])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        request(
            &store,
            "POST",
            "/v1/pair",
            None,
            json!({"code":code,"device_id":"synthetic-revoked"})
        )
        .await
        .0,
        StatusCode::GONE
    );
    let listed = cli().args(["pair", "list"]).output().unwrap();
    let info: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(info[0]["revoked"], true);
    assert!(info[0]["used_at"].is_null());
}

#[tokio::test]
async fn internal_pairing_errors_return_empty_503_and_leave_codes_redeemable() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let code = mint(dir.path());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_with(
            SqliteConnectOptions::new()
                .filename(dir.path().join("core.sqlite"))
                .busy_timeout(std::time::Duration::from_millis(50)),
        )
        .await
        .unwrap();
    let contended = Store::new(pool.clone(), dir.path());
    let check = || async {
        let response = hx_server::router(contended.clone(), DisclosurePolicy::default())
            .oneshot(
                Request::post("/v1/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"code":code,"device_id":"synthetic-busy"}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(bytes.is_empty(), "503 must have no body");
    };
    let writer = store.begin_write().await.unwrap();
    check().await; // SQLITE_BUSY, with a short test-only busy timeout.
    writer.rollback().await.unwrap();
    let held = pool.acquire().await.unwrap();
    check().await; // PoolTimedOut, without waiting thirty seconds.
    drop(held);
    pool.close().await;
    check().await; // Other internal failures are retryable too.
    assert_eq!(
        request(
            &store,
            "POST",
            "/v1/pair",
            None,
            json!({"code":code,"device_id":"synthetic-busy"})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn malformed_and_unknown_codes_are_refused_without_waiting_for_the_writer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path()).await.unwrap();
    let writer = store.begin_write().await.unwrap();
    let attempt = |code: String| {
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            request(
                &store,
                "POST",
                "/v1/pair",
                None,
                json!({"code":code,"device_id":"synthetic-probe"}),
            ),
        )
    };
    let (malformed, unknown) = tokio::join!(attempt("bad".into()), attempt("ab".repeat(32)));
    writer.rollback().await.unwrap();
    assert!(malformed.is_ok(), "malformed code waited for the writer");
    assert!(unknown.is_ok(), "unknown code waited for the writer");
    assert_eq!(malformed.unwrap().0, StatusCode::GONE);
    assert_eq!(unknown.unwrap().0, StatusCode::GONE);
}
