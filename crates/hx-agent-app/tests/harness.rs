#![cfg(feature = "e2e-harness")]
//! Starts only the synthetic app binary, never an installed app or launchd job.
use axum::{
    Json, Router,
    body::Bytes,
    http::{HeaderMap, StatusCode},
    routing::{post, put},
};
use hx_schema::{UploadAck, UploadBatch};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test]
async fn harness_pairs_after_start_and_uploads_synthetic_jpeg_without_capture() {
    let scratch = tempfile::tempdir().unwrap();
    let support = scratch.path().join("support");
    let jpeg_path = scratch.path().join("synthetic.jpg");
    let jpeg = include_bytes!("fixtures/synthetic.jpg").to_vec();
    std::fs::write(&jpeg_path, &jpeg).unwrap();
    let blobs = Arc::new(Mutex::new(Vec::new()));
    let (done, received) = tokio::sync::oneshot::channel();
    let done = Arc::new(Mutex::new(Some(done)));
    let app = Router::new()
        .route(
            "/v1/pair",
            post(|Json(request): Json<Value>| async move {
                assert_eq!(request["device_id"], "synthetic-e2e");
                assert_eq!(request["code"], "ab".repeat(32));
                Json(json!({"token":"synthetic-harness-token"}))
            }),
        )
        .route(
            "/v1/blobs/{sha}",
            put({
                let blobs = blobs.clone();
                move |headers: HeaderMap, bytes: Bytes| {
                    let blobs = blobs.clone();
                    async move {
                        assert_eq!(headers["authorization"], "Bearer synthetic-harness-token");
                        assert_eq!(headers["content-type"], "image/jpeg");
                        blobs.lock().unwrap().push(bytes.to_vec());
                        StatusCode::CREATED
                    }
                }
            }),
        )
        .route(
            "/v1/events",
            post(move |headers: HeaderMap, bytes: Bytes| {
                let done = done.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer synthetic-harness-token");
                    let batch: UploadBatch =
                        serde_json::from_slice(&zstd::decode_all(bytes.as_ref()).unwrap()).unwrap();
                    if let Some(tx) = done.lock().unwrap().take() {
                        let _ = tx.send(batch.clone());
                    }
                    Json(UploadAck {
                        batch_id: batch.batch_id,
                        accepted: batch.envelopes.len() as u32,
                        duplicates: 0,
                        rejected: vec![],
                    })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hx-agent-app"))
        .args(["--headless", "--duration-s", "30", "--support-dir"])
        .arg(&support)
        .arg("--seed-jpeg")
        .arg(&jpeg_path)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), stdout.next_line())
            .await
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("Synthetic JPEG queued.")
    );
    assert!(
        !support.join("agent.json").exists(),
        "harness starts unpaired"
    );
    let link = format!("hyperthymesia://pair?server={url}&code={}", "ab".repeat(32));
    hx_agent::pair::redeem(&link, false, &support)
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), received).await;
    child.kill().await.unwrap();
    server.abort();
    let batch = result
        .expect("harness must hot-reload pairing and upload")
        .unwrap();
    assert_eq!(*blobs.lock().unwrap(), vec![jpeg.clone()]);
    assert_eq!(batch.envelopes.len(), 1, "no native collectors may run");
    let still = &batch.envelopes[0];
    assert_eq!(still.kind.as_str(), "media.still");
    assert_eq!(
        still.connector_instance.as_str(),
        "macos-media:synthetic-e2e"
    );
    assert_eq!(still.blobs[0].sha256, hx_schema::sha256_hex(&jpeg));
    assert_eq!(still.payload["width"], 8);
    assert_eq!(still.payload["height"], 8);
}
