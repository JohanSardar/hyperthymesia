//! Public CLI contract; HOME always points at an empty temporary directory.
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct PairState {
    requests: Vec<Value>,
    used: bool,
}
async fn redeem(
    State(state): State<Arc<Mutex<PairState>>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut state = state.lock().unwrap();
    state.requests.push(body);
    if state.used {
        return (StatusCode::GONE, Json(json!({})));
    }
    state.used = true;
    (StatusCode::OK, Json(json!({"token":"synthetic-token"})))
}
async fn server(used: bool) -> (String, Arc<Mutex<PairState>>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Mutex::new(PairState {
        used,
        ..Default::default()
    }));
    let app = Router::new()
        .route("/v1/pair", post(redeem))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, state, task)
}
fn config(home: &Path) -> std::path::PathBuf {
    home.join(if cfg!(feature = "e2e-harness") {
        "Library/Application Support/Hyperthymesia-e2e/agent.json"
    } else {
        "Library/Application Support/Hyperthymesia/agent.json"
    })
}
fn seed(home: &Path, token: &str) -> Vec<u8> {
    let path = config(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = serde_json::to_vec(&json!({"device_id":"synthetic-mac", "token":token,"server_url":"https://old.invalid", "still_cadence_s":123,"future_option":{"keep":true}})).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}
async fn pair(home: &Path, url: &str, replace: bool) -> std::process::Output {
    let mut link = reqwest::Url::parse("hyperthymesia://pair").unwrap();
    link.query_pairs_mut()
        .append_pair("server", url)
        .append_pair("code", &"ab".repeat(32));
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hx-agent"));
    command
        .env("HOME", home)
        .env("HX_E2E_SUPPORT_DIR", config(home).parent().unwrap())
        .arg("pair")
        .arg(link.as_str());
    if replace {
        command.arg("--replace");
    }
    command.output().await.unwrap()
}
#[tokio::test]
async fn pairing_merges_only_credentials_atomically_with_mode_600() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), "");
    let (url, state, task) = server(false).await;
    let result = pair(home.path(), &url, false).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(config(home.path())).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({"device_id":"synthetic-mac", "token":"synthetic-token","server_url":url,"still_cadence_s":123,"future_option":{"keep":true}})
    );
    assert_eq!(
        std::fs::metadata(config(home.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        state.lock().unwrap().requests,
        vec![json!({"code":"ab".repeat(32),"device_id":"synthetic-mac"})]
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("synthetic-token"));
    task.abort();
}
#[tokio::test]
async fn used_code_leaves_agent_json_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let original = seed(home.path(), "kept-token");
    let (url, _, task) = server(true).await;
    let result = pair(home.path(), &url, true).await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("already used"));
    assert_eq!(std::fs::read(config(home.path())).unwrap(), original);
    task.abort();
}
#[tokio::test]
async fn non_loopback_http_is_refused_before_redeeming() {
    let home = tempfile::tempdir().unwrap();
    let original = seed(home.path(), "");
    let result = pair(home.path(), "http://example.com:7377", false).await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("loopback"));
    assert_eq!(std::fs::read(config(home.path())).unwrap(), original);
}
#[tokio::test]
async fn configured_token_requires_replace_before_consuming_code() {
    let home = tempfile::tempdir().unwrap();
    let original = seed(home.path(), "kept-token");
    let (url, state, task) = server(false).await;
    let result = pair(home.path(), &url, false).await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--replace"));
    assert_eq!(std::fs::read(config(home.path())).unwrap(), original);
    assert!(state.lock().unwrap().requests.is_empty());
    assert!(pair(home.path(), &url, true).await.status.success());
    task.abort();
}

#[tokio::test]
async fn redirects_cannot_forward_a_pairing_code() {
    let home = tempfile::tempdir().unwrap();
    let original = seed(home.path(), "");
    let state = Arc::new(Mutex::new(0));
    let hits = state.clone();
    let app = Router::new()
        .route(
            "/v1/pair",
            post(|| async { axum::response::Redirect::temporary("/leaked") }),
        )
        .route(
            "/leaked",
            post(move || {
                let hits = hits.clone();
                async move {
                    *hits.lock().unwrap() += 1;
                    Json(json!({"token":"leaked"}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let result = pair(home.path(), &url, false).await;
    assert!(!result.status.success());
    assert_eq!(*state.lock().unwrap(), 0);
    assert_eq!(std::fs::read(config(home.path())).unwrap(), original);
    task.abort();
}
