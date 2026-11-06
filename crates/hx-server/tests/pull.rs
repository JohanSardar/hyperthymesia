#![cfg(unix)]
use axum::{
    Json, Router,
    extract::Query,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use chrono::{Duration, Utc};
use hx_sources::{keys::KeyStore, recipe::Recipe};
use hx_store::{Store, Ts};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    source: String,
    recipe: String,
    requests: Arc<Mutex<Vec<String>>>,
    auth: Arc<Mutex<Vec<String>>>,
    pages: Arc<Mutex<Vec<Value>>>,
    api: tokio::task::JoinHandle<()>,
    switch: Arc<Mutex<Option<(Store, String, bool)>>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.api.abort();
    }
}
impl Fixture {
    async fn new(pages: Vec<Value>) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let reads = requests.clone();
        let pages = Arc::new(Mutex::new(pages));
        let api_pages = pages.clone();
        let auth: Arc<Mutex<Vec<String>>> = Arc::default();
        let api_auth = auth.clone();
        let switch: Arc<Mutex<Option<(Store, String, bool)>>> = Arc::default();
        let api_switch = switch.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/?cursor={{{{cursor}}}}",
            listener.local_addr().unwrap()
        );
        let app = Router::new().route(
            "/",
            get(
                move |Query(q): Query<std::collections::HashMap<String, String>>,
                      headers: HeaderMap| {
                    let reads = reads.clone();
                    let pages = api_pages.clone();
                    let auth = api_auth.clone();
                    let switch = api_switch.clone();
                    async move {
                        let cursor = q.get("cursor").cloned().unwrap_or_default();
                        reads.lock().unwrap().push(cursor.clone());
                        auth.lock().unwrap().push(
                            headers
                                .get("authorization")
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_owned(),
                        );
                        let index: usize = cursor.parse().unwrap_or(0);
                        let change = switch.lock().unwrap().take();
                        if let Some((store, id, source)) = change {
                            if source {
                                store.set_source_enabled(&id, false).await.unwrap();
                            } else {
                                store.set_recipe_enabled(&id, false).await.unwrap();
                            }
                        }
                        let page = pages
                            .lock()
                            .unwrap()
                            .get(index)
                            .cloned()
                            .unwrap_or(json!({"items":[]}));
                        let status = page
                            .get("status")
                            .and_then(Value::as_u64)
                            .map(|s| StatusCode::from_u16(s as u16).unwrap())
                            .unwrap_or(StatusCode::OK);
                        (status, Json(page))
                    }
                },
            ),
        );
        let api = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(dir.path()).await.unwrap();
        let source = store
            .connect_source(
                "fake",
                &format!("Fake API at {url}. Fixture commands `/usr/bin/printf`, `/bin/sh -c`."),
                1,
            )
            .await
            .unwrap();
        let recipe: Recipe = serde_json::from_value(json!({
            "source":source.id,"name":"mail","request":{"kind":"http","method":"GET","url":url,
            "headers":{"Authorization":"Bearer {{key}}"}},
            "items":"/items","kind":"source.email","map":{"id":"/id","start":"/at","title":"/title"},
            "time_format":"rfc3339","cursor":{"from":"response","pointer":"/next"},
            "pages":{"next":"/next","max":20},"ignore":["/volatile"]
        })).unwrap();
        let recipe = store.save_recipe(&source.id, &recipe).await.unwrap();
        KeyStore::new(dir.path())
            .unwrap()
            .write(&source.id, "planted-fake-secret")
            .unwrap();
        store
            .finish_source_setup(&source.id, 1, json!({}), None)
            .await
            .unwrap();
        sqlx::query("DELETE FROM jobs")
            .execute(&store.pool)
            .await
            .unwrap();
        Self {
            _dir: dir,
            store,
            source: source.id,
            recipe: recipe.id,
            requests,
            auth,
            pages,
            api,
            switch,
        }
    }
    async fn run(&self) -> anyhow::Result<hx_server::pull::PullReport> {
        hx_server::pull::run(&self.store, &self.recipe, Utc::now()).await
    }
    async fn due(&self) {
        sqlx::query("UPDATE recipe_state SET next_run_at=$1")
            .bind(Ts::from(Utc::now() - Duration::hours(12)))
            .execute(&self.store.pool)
            .await
            .unwrap();
    }
    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(match table {
            "events" => "SELECT count(*) FROM events",
            "windows" => "SELECT count(*) FROM windows",
            "device_tokens" => "SELECT count(*) FROM device_tokens",
            _ => panic!("unknown fixture table"),
        })
        .fetch_one(&self.store.pool)
        .await
        .unwrap()
    }
}
fn item(id: usize) -> Value {
    json!({"id":id,"at":Utc::now()-Duration::hours(8),"title":"hello planted\u{0000}-fake-secret\u{0000}","volatile":1})
}

#[tokio::test]
async fn interrupted_page_replays_duplicates_then_resumes_the_saved_cursor() {
    let f = Fixture::new(vec![
        json!({"items":[item(1)],"next":"1"}),
        json!({"items":[item(2),item(3)],"next":"2"}),
        json!({"items":[item(4)]}),
    ])
    .await;
    // Kill the page midway through ingest: the transaction must roll back,
    // leaving page one's committed cursor intact.
    sqlx::query("CREATE TRIGGER crash_ingest BEFORE INSERT ON events WHEN NEW.source_key LIKE '%:3' BEGIN SELECT RAISE(ABORT,'synthetic crash'); END").execute(&f.store.pool).await.unwrap();
    assert!(f.run().await.is_err());
    assert_eq!(f.count("events").await, 1);
    sqlx::query("DROP TRIGGER crash_ingest")
        .execute(&f.store.pool)
        .await
        .unwrap();
    // Also kill after page two commits, before its checkpoint commits.
    sqlx::query("CREATE TRIGGER crash_checkpoint BEFORE UPDATE ON recipe_state WHEN (SELECT count(*) FROM events)=3 BEGIN SELECT RAISE(ABORT,'synthetic crash'); END").execute(&f.store.pool).await.unwrap();
    assert!(f.run().await.is_err());
    assert_eq!(f.count("events").await, 3);
    sqlx::query("DROP TRIGGER crash_checkpoint")
        .execute(&f.store.pool)
        .await
        .unwrap();
    let report = f.run().await.unwrap();
    assert_eq!(report.duplicates, 2);
    assert_eq!(f.count("events").await, 4);
    assert_eq!(*f.requests.lock().unwrap(), ["", "1", "1", "1", "2"]);
    let values: Vec<String> = sqlx::query_scalar("SELECT envelope FROM events")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
    assert!(
        values
            .iter()
            .all(|v| !v.contains("planted-fake-secret") && !v.contains("\\u0000"))
    );
    assert_eq!(f.count("device_tokens").await, 0);
}

#[tokio::test]
async fn three_missed_boundaries_queue_one_pull_and_no_model() {
    use hx_jobs::Handler;
    let f = Fixture::new(vec![json!({"items":[item(1)]})]).await;
    f.due().await;
    for _ in 0..3 {
        hx_server::processing::schedule(&f.store).await.unwrap();
    }
    let jobs: Vec<(String, Value)> =
        sqlx::query_as("SELECT kind,payload FROM jobs WHERE kind='pull'")
            .fetch_all(&f.store.pool)
            .await
            .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].0, "pull");
    let runner = hx_server::processing::Runner {
        store: f.store.clone(),
        provider: hx_server::processing::AnyProvider::Fx,
    };
    loop {
        let job = hx_jobs::claim(&f.store, 120).await.unwrap().unwrap();
        runner.handle(&job).await.unwrap();
        hx_jobs::finish(&f.store, &job, None).await.unwrap();
        if job.kind == "pull" {
            break;
        }
    }
    hx_server::processing::schedule(&f.store).await.unwrap();
    f.run().await.unwrap();
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    let state = f.store.recipe_state(&f.recipe).await.unwrap();
    assert_eq!(state.next_run_at.timestamp() % (4 * 60 * 60), 0);
    assert!(state.next_run_at > Utc::now());
    assert!(state.next_run_at <= Utc::now() + Duration::hours(4));
    assert!(state.last_ok_at.is_some());
}

#[tokio::test]
async fn owner_off_reads_neither_source_nor_key_even_with_a_stale_job() {
    let f = Fixture::new(vec![json!({"items":[item(1)]})]).await;
    // A read would repair the mode. Off must leave this deliberately bad key alone.
    use std::os::unix::fs::PermissionsExt;
    let key = f.store.data_dir.join(format!("secrets/{}.key", f.source));
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    for source_off in [true, false] {
        f.store
            .set_source_enabled(&f.source, !source_off)
            .await
            .unwrap();
        f.store
            .set_recipe_enabled(&f.recipe, source_off)
            .await
            .unwrap();
        f.run().await.unwrap();
        assert_eq!(
            std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
    assert!(f.requests.lock().unwrap().is_empty());
    assert_eq!(f.count("events").await, 0);
}

#[tokio::test]
async fn history_floor_and_bad_records_hold_on_every_cycle() {
    let mut old = item(1);
    old["at"] = json!(Utc::now() - Duration::days(365));
    let mut bad = item(2);
    bad["at"] = json!("not a time");
    let mut future = item(3);
    future["at"] = json!("9999-12-31T00:00:00Z");
    let mut huge = item(4);
    huge["title"] = json!("x".repeat(70 * 1024));
    let f = Fixture::new(vec![json!({"items":[old,bad,future,huge,item(5)]})]).await;
    for _ in 0..2 {
        f.due().await;
        let report = f.run().await.unwrap();
        assert_eq!(report.counts.bad_time, 1);
        assert_eq!(report.counts.too_old, 1);
        assert_eq!(report.counts.too_future, 1);
        assert_eq!(report.counts.too_large, 1);
    }
    assert_eq!(f.requests.lock().unwrap().len(), 2);
    assert_eq!(f.count("events").await, 1);
    assert!(f.count("windows").await <= 2);
    assert!(
        f.store
            .recipe_state(&f.recipe)
            .await
            .unwrap()
            .last_ok_at
            .is_some()
    );

    let mut numeric = item(6);
    numeric["credential"] = json!(1234);
    let f = Fixture::new(vec![json!({"items":[numeric]})]).await;
    KeyStore::new(&f.store.data_dir)
        .unwrap()
        .write(&f.source, "1234")
        .unwrap();
    f.run().await.unwrap();
    let envelope: String = sqlx::query_scalar("SELECT envelope FROM events")
        .fetch_one(&f.store.pool)
        .await
        .unwrap();
    assert!(
        !envelope.contains("1234"),
        "numeric secrets must be scrubbed too"
    );
}

#[tokio::test]
async fn item_and_page_limits_keep_the_remainder_for_next_cycle() {
    let items: Vec<_> = (0..1005).map(item).collect();
    let f = Fixture::new(vec![
        json!({"items":items,"next":"1"}),
        json!({"items":[item(1005)]}),
    ])
    .await;
    f.run().await.unwrap();
    assert_eq!(f.count("events").await, 1000);
    f.due().await;
    f.run().await.unwrap();
    assert_eq!(f.count("events").await, 1006);
    assert_eq!(*f.requests.lock().unwrap(), ["", "1"]);
    let pages = (0..22)
        .map(|n| json!({"items":[item(n)],"next":(n+1).to_string()}))
        .collect();
    let f = Fixture::new(pages).await;
    f.run().await.unwrap();
    assert_eq!(f.requests.lock().unwrap().len(), 20);
    f.due().await;
    f.run().await.unwrap();
    assert_eq!(f.count("events").await, 22);
    assert_eq!(f.requests.lock().unwrap()[20], "20");
    // P6 permits a native cursor value; an upgraded runner must not reset it.
    let before = f.requests.lock().unwrap().len();
    let mut state = f.store.recipe_state(&f.recipe).await.unwrap();
    state.cursor = Some(json!("20"));
    state.next_run_at = Utc::now() - Duration::hours(4);
    f.store.save_recipe_state(&state).await.unwrap();
    f.run().await.unwrap();
    assert_eq!(f.requests.lock().unwrap()[before], "20");
}

#[tokio::test]
async fn switching_off_during_a_page_stops_before_the_next_page() {
    for source in [true, false] {
        let f = Fixture::new(vec![
            json!({"items":[item(1)],"next":"1"}),
            json!({"items":[item(2)]}),
        ])
        .await;
        *f.switch.lock().unwrap() = Some((
            f.store.clone(),
            if source {
                f.source.clone()
            } else {
                f.recipe.clone()
            },
            source,
        ));
        f.run().await.unwrap();
        assert_eq!(*f.requests.lock().unwrap(), [""]);
        assert_eq!(
            f.count("events").await,
            0,
            "an off source must not ingest an in-flight result"
        );
    }
}

#[tokio::test]
async fn remote_errors_are_distinct_and_wait_until_the_next_boundary() {
    for status in [401, 403, 429, 500] {
        let f = Fixture::new(vec![json!({"status":status})]).await;
        if status == 401 {
            sqlx::query("CREATE TRIGGER crash_reconnect BEFORE UPDATE ON sources WHEN NEW.status='needs_reconnect' BEGIN SELECT RAISE(ABORT,'synthetic crash'); END").execute(&f.store.pool).await.unwrap();
            assert!(f.run().await.is_err());
            assert!(
                f.store
                    .recipe_state(&f.recipe)
                    .await
                    .unwrap()
                    .last_error
                    .is_none(),
                "reconnect status and next boundary must commit together"
            );
            sqlx::query("DROP TRIGGER crash_reconnect")
                .execute(&f.store.pool)
                .await
                .unwrap();
            f.requests.lock().unwrap().clear();
        }
        f.run().await.unwrap();
        f.run().await.unwrap();
        assert_eq!(f.requests.lock().unwrap().len(), 1);
        let state = f.store.recipe_state(&f.recipe).await.unwrap();
        assert_eq!(state.last_error, Some(format!("HTTP status {status}")));
        assert_eq!(state.next_run_at.timestamp() % (4 * 60 * 60), 0);
        assert!(state.next_run_at > Utc::now());
        assert!(state.next_run_at <= Utc::now() + Duration::hours(4));
        assert_eq!(
            f.store.source(&f.source).await.unwrap().unwrap().status,
            if status == 401 || status == 403 {
                "needs_reconnect"
            } else {
                "ready"
            }
        );
    }
}

#[tokio::test]
async fn untrusted_next_link_never_receives_the_key() {
    let f = Fixture::new(vec![
        json!({"items":[item(1)],"next":"http://localhost:1/steal"}),
    ])
    .await;
    f.run().await.unwrap();
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    assert_eq!(f.count("events").await, 1);
    assert_eq!(
        f.store
            .recipe_state(&f.recipe)
            .await
            .unwrap()
            .last_error
            .as_deref(),
        Some("invalid HTTP request")
    );
}

#[tokio::test]
async fn setup_success_pulls_again_without_replaying_missed_slots() {
    let f = Fixture::new(vec![json!({"items":[item(1)]})]).await;
    f.run().await.unwrap();
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    let generation = f.store.request_source_setup(&f.source).await.unwrap();
    f.run().await.unwrap();
    assert_eq!(
        f.requests.lock().unwrap().len(),
        1,
        "pending setup must not pull"
    );
    f.store
        .finish_source_setup(&f.source, generation, json!({}), None)
        .await
        .unwrap();
    f.run().await.unwrap();
    assert_eq!(f.requests.lock().unwrap().len(), 2);
    assert_eq!(f.count("events").await, 1);
}

#[tokio::test]
async fn cli_dispatch_and_reconnect_codes_and_mcp_transport_are_honest() {
    use hx_sources::request::{McpTransport, Request};
    let f = Fixture::new(vec![]).await;
    f.store.set_recipe_enabled(&f.recipe, false).await.unwrap();
    let mut recipe = f.store.recipe(&f.recipe).await.unwrap().unwrap().recipe.0;
    recipe.pages = None;
    recipe.cursor = None;
    recipe.request = Request::Cli {
        argv: vec![
            "/usr/bin/printf".into(),
            "%s".into(),
            json!({"items":[item(1)]}).to_string(),
        ],
        env: Default::default(),
    };
    let stored = f.store.save_recipe(&f.source, &recipe).await.unwrap();
    f.store.set_recipe_enabled(&stored.id, true).await.unwrap();
    let report = hx_server::pull::run(&f.store, &stored.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(report.accepted, 1);
    assert!(report.error.is_none());
    recipe.request = Request::Cli {
        argv: vec!["/bin/sh".into(), "-c".into(), "exit 2".into()],
        env: Default::default(),
    };
    recipe.reconnect_exit_codes = vec![2];
    let stored = f.store.save_recipe(&f.source, &recipe).await.unwrap();
    f.store.set_recipe_enabled(&stored.id, true).await.unwrap();
    let report = hx_server::pull::run(&f.store, &stored.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        report.error.as_deref(),
        Some("host command exited with code 2")
    );
    assert_eq!(
        f.store.source(&f.source).await.unwrap().unwrap().status,
        "needs_reconnect"
    );
    let generation = f.store.request_source_setup(&f.source).await.unwrap();
    f.store
        .finish_source_setup(&f.source, generation, json!({}), None)
        .await
        .unwrap();
    recipe.request = Request::Mcp {
        transport: McpTransport::Stdio {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "read request; printf 'not-json\\n'".into(),
            ],
        },
        tool: "messages".into(),
        arguments: json!({}),
    };
    let stored = f.store.save_recipe(&f.source, &recipe).await.unwrap();
    f.store.set_recipe_enabled(&stored.id, true).await.unwrap();
    let report = hx_server::pull::run(&f.store, &stored.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        report.error.as_deref(),
        Some("MCP exchange failed or returned an invalid response")
    );
    assert!(f.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn review_all_source_secrets_scrub_events_and_pending_checkpoint() {
    const OTHER_KEY: &str = "synthetic-other-source-credential";
    let mut items: Vec<_> = (0..1001).map(item).collect();
    for item in &mut items {
        item["body"] = json!(OTHER_KEY);
    }
    let f = Fixture::new(vec![json!({"items":items})]).await;
    let other = f
        .store
        .connect_source("other fake source", "fake only", 1)
        .await
        .unwrap();
    KeyStore::new(&f.store.data_dir)
        .unwrap()
        .write(&other.id, OTHER_KEY)
        .unwrap();
    // Off controls requests and grants; saved credentials still need redaction.
    f.store.set_source_enabled(&other.id, false).await.unwrap();
    let report = f.run().await.unwrap();
    assert_eq!(report.accepted, 1000);
    assert!(report.error.is_none());
    let events: Vec<String> = sqlx::query_scalar("SELECT envelope FROM events")
        .fetch_all(&f.store.pool)
        .await
        .unwrap();
    let checkpoint = f
        .store
        .recipe_state(&f.recipe)
        .await
        .unwrap()
        .cursor
        .unwrap();
    assert!(
        checkpoint["pending"].is_object(),
        "exercise durable partial-page storage"
    );
    assert!(
        events.iter().all(|v| !v.contains(OTHER_KEY)),
        "another source's credential leaked into events"
    );
    assert!(
        !checkpoint.to_string().contains(OTHER_KEY),
        "another source's credential leaked into the checkpoint"
    );
    assert_eq!(*f.auth.lock().unwrap(), ["Bearer planted-fake-secret"]);
    assert!(events.iter().all(|v| v.contains("[REDACTED]")));
}

#[tokio::test]
async fn review_unreadable_source_secret_fails_without_narrowing_scrub() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(vec![json!({"items":[item(1)]})]).await;
    let other = f
        .store
        .connect_source("other fake source", "fake only", 1)
        .await
        .unwrap();
    let keys = KeyStore::new(&f.store.data_dir).unwrap();
    keys.write(&other.id, "synthetic-other-source-credential")
        .unwrap();
    let path = f.store.data_dir.join(format!("secrets/{}.key", other.id));
    let target = f.store.data_dir.join("synthetic-key-target");
    std::fs::rename(&path, &target).unwrap();
    symlink(target, &path).unwrap();
    let report = f.run().await.unwrap();
    assert!(
        report.error.is_some(),
        "an unreadable configured secret must fail the pull"
    );
    assert!(f.requests.lock().unwrap().is_empty());
    assert_eq!(f.count("events").await, 0);
    let state = f.store.recipe_state(&f.recipe).await.unwrap();
    assert!(state.cursor.is_none());
    assert_eq!(state.last_error, report.error);
    assert!(state.next_run_at > Utc::now());
}

#[tokio::test]
async fn review_snapshot_traversals_restart_and_old_checkpoints_still_load() {
    let f = Fixture::new(vec![
        json!({"items":[item(1)],"next":"1"}),
        json!({"items":[item(2)]}),
    ])
    .await;
    let mut recipe = f.store.recipe(&f.recipe).await.unwrap().unwrap().recipe.0;
    recipe.cursor = None;
    f.store.save_recipe(&f.source, &recipe).await.unwrap();
    f.run().await.unwrap();
    assert_eq!(*f.requests.lock().unwrap(), ["", "1"]);
    f.pages.lock().unwrap()[0]["items"]
        .as_array_mut()
        .unwrap()
        .push(item(3));
    f.due().await;
    f.run().await.unwrap();
    assert_eq!(
        *f.requests.lock().unwrap(),
        ["", "1", "", "1"],
        "a completed snapshot must start at page one next cycle"
    );
    assert_eq!(f.count("events").await, 3);
    // The old format cannot distinguish a terminal page token from an unfinished
    // traversal. Replaying a snapshot from its start is safe and loses no data.
    let mut state = f.store.recipe_state(&f.recipe).await.unwrap();
    state.cursor =
        Some(json!({"pull_checkpoint_v1":true,"cursor":"1","pending":null,"pulled_at":null}));
    state.next_run_at = Utc::now() - Duration::hours(4);
    f.store.save_recipe_state(&state).await.unwrap();
    f.pages.lock().unwrap()[0] = json!({"items":[item(4)],"next":"1"});
    f.run().await.unwrap();
    assert_eq!(&f.requests.lock().unwrap()[4..], ["", "1"]);
    assert_eq!(f.count("events").await, 4);
    let mut state = f.store.recipe_state(&f.recipe).await.unwrap();
    state.cursor = Some(
        json!({"pull_checkpoint_v1":true,"cursor":"1","pending":{"items":[item(5)]},"pulled_at":Utc::now()}),
    );
    state.next_run_at = Utc::now() - Duration::hours(4);
    f.store.save_recipe_state(&state).await.unwrap();
    f.run().await.unwrap();
    assert_eq!(
        f.requests.lock().unwrap().len(),
        6,
        "an old cached remainder must not be refetched"
    );
    assert_eq!(f.count("events").await, 5);
    f.due().await;
    f.run().await.unwrap();
    assert_eq!(&f.requests.lock().unwrap()[6..], ["", "1"]);

    let f = Fixture::new(vec![
        json!({"items":[item(1)],"next":"1"}),
        json!({"items":[item(2)]}),
    ])
    .await;
    let mut recipe = f.store.recipe(&f.recipe).await.unwrap().unwrap().recipe.0;
    recipe.cursor = None;
    recipe.pages.as_mut().unwrap().max = 1;
    f.store.save_recipe(&f.source, &recipe).await.unwrap();
    for _ in 0..3 {
        f.due().await;
        f.run().await.unwrap();
    }
    assert_eq!(*f.requests.lock().unwrap(), ["", "1", ""]);
    assert_eq!(f.count("events").await, 2);
}

#[tokio::test]
async fn review_pagination_preserves_incremental_watermarks_across_pages() {
    let f = Fixture::new(vec![
        json!({"items":[item(1)],"next":"1","watermark":"100"}),
        json!({"items":[item(2)],"watermark":"200"}),
    ])
    .await;
    let mut recipe = f.store.recipe(&f.recipe).await.unwrap().unwrap().recipe.0;
    recipe.cursor.as_mut().unwrap().pointer = "/watermark".into();
    f.store.save_recipe(&f.source, &recipe).await.unwrap();
    f.run().await.unwrap();
    f.due().await;
    f.run().await.unwrap();
    assert_eq!(*f.requests.lock().unwrap(), ["", "1", "200"]);
    assert_eq!(f.count("events").await, 2);
}
