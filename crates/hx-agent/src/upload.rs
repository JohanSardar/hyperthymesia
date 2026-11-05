use crate::{config::Config, outbox::Outbox};
use anyhow::{Result, ensure};
use hx_schema::{
    UploadAck,
    protocol::{DeviceResponse, OcrMode, OcrRequest, OcrStatus},
};
use hx_visual::{OcrEngine, VisionCliEngine};
use std::sync::Arc;
pub struct Uploader {
    client: reqwest::Client,
    ocr_client: reqwest::Client,
    server: String,
    token: String,
    pub failures: u32,
    mode: Option<OcrMode>,
    // OCR text lives only here, never in the outbox, health reports or errors.
    pending_ocr: Option<(String, OcrRequest)>,
}
impl Uploader {
    pub fn new(config: &Config) -> Result<Self> {
        let url = reqwest::Url::parse(&config.server_url)?;
        ensure!(
            ["http", "https"].contains(&url.scheme()),
            "invalid server URL"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            ocr_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            server: config.server_url.trim_end_matches('/').into(),
            token: config.token.clone(),
            failures: 0,
            mode: None,
            pending_ocr: None,
        })
    }
    pub fn delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(if self.failures == 0 {
            10
        } else {
            (1u64 << self.failures.min(8)).min(300)
        })
    }
    pub async fn once(&mut self, outbox: &mut Outbox) -> Result<()> {
        let result = self.send(outbox).await;
        if result.is_ok() {
            self.failures = 0
        } else {
            self.failures = self.failures.saturating_add(1);
        }
        result
    }
    async fn negotiate(&mut self) -> Result<OcrMode> {
        let response = self
            .client
            .get(format!("{}/v1/device", self.server))
            .bearer_auth(&self.token)
            .send()
            .await?;
        let mode = if response.status() == reqwest::StatusCode::NOT_FOUND {
            OcrMode::Server
        } else {
            response
                .error_for_status()?
                .json::<DeviceResponse>()
                .await?
                .ocr
        };
        self.mode = Some(mode);
        if mode == OcrMode::Server {
            self.pending_ocr = None;
        }
        Ok(mode)
    }
    async fn send(&mut self, outbox: &mut Outbox) -> Result<()> {
        let mode = self.negotiate().await?;
        let Some(batch) = outbox.batch()? else {
            return Ok(());
        };
        for e in &batch.envelopes {
            for b in &e.blobs {
                if let Some((media, data)) = outbox.blob(&b.sha256)? {
                    let response = self
                        .client
                        .put(format!("{}/v1/blobs/{}", self.server, b.sha256))
                        .bearer_auth(&self.token)
                        .header("Content-Type", media)
                        .body(data)
                        .send()
                        .await?;
                    ensure!(
                        [200, 201].contains(&response.status().as_u16()),
                        "blob upload: {}",
                        response.status()
                    );
                } else {
                    anyhow::bail!("pending batch references a missing blob {}", b.sha256);
                }
            }
        }
        let compressed = zstd::encode_all(serde_json::to_vec(&batch)?.as_slice(), 3)?;
        let ack: UploadAck = self
            .client
            .post(format!("{}/v1/events", self.server))
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .header("Content-Encoding", "zstd")
            .body(compressed)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        outbox.ack_with_ocr(&ack, mode == OcrMode::Device)
    }
    pub(crate) async fn ocr_once(&mut self, outbox: &mut Outbox, saver: bool) -> Result<()> {
        if saver
            || (self.pending_ocr.is_none()
                && outbox
                    .next_ocr(chrono::Utc::now().timestamp_millis())?
                    .is_none())
        {
            return Ok(());
        }
        if self.negotiate().await? != OcrMode::Device {
            return Ok(());
        }
        // Version resolution errors also stay content-free at the device boundary.
        let version = hx_visual::vision_version()
            .map_err(|_| anyhow::anyhow!("cannot identify OCR engine"))?;
        self.ocr_tick(
            outbox,
            false,
            || chrono::Utc::now().timestamp_millis(),
            Arc::new(VisionCliEngine::default()),
            &version,
        )
        .await
    }
    async fn ocr_tick(
        &mut self,
        outbox: &mut Outbox,
        saver: bool,
        now: impl Fn() -> i64 + Send,
        engine: Arc<dyn OcrEngine>,
        version: &str,
    ) -> Result<()> {
        if saver {
            return Ok(());
        }
        let mode = match self.mode {
            Some(mode) => mode,
            None => self.negotiate().await?,
        };
        if mode != OcrMode::Device {
            return Ok(());
        }
        if self.pending_ocr.is_none() {
            let Some(job) = outbox.next_ocr(now())? else {
                return Ok(());
            };
            let mut request = OcrRequest {
                status: OcrStatus::Failed,
                engine_version: version.into(),
                perceptual_hash: None,
                sampled_from: None,
                result: None,
            };
            if job.attempts < 5 {
                let (media, bytes) = outbox
                    .blob(&job.sha)?
                    .ok_or_else(|| anyhow::anyhow!("OCR blob missing"))?;
                let previous = outbox.previous_ocr(&job.sha, version)?;
                // Expensive decoding/helper I/O never blocks the async executor. No
                // helper stdout/stderr/error is allowed into a returned error or log.
                let result = tokio::task::spawn_blocking(move || -> Result<_> {
                    let hash = hx_visual::thumbnail_hash(&bytes)?;
                    if let Some((sha, prior)) = previous
                        && hx_visual::similar_thumbnail_hashes(hash, prior)
                    {
                        return Ok((hash, Some(sha), None));
                    }
                    let result = serde_json::to_value(engine.ocr(&bytes, &media)?)?;
                    // The server endpoint has a 1 MiB body limit; leave room for metadata.
                    ensure!(
                        serde_json::to_vec(&result)?.len() <= 1024 * 1024 - 4096,
                        "OCR result too large"
                    );
                    Ok((hash, None, Some(result)))
                })
                .await;
                match result {
                    Ok(Ok((hash, sampled, result))) => {
                        request.status = if sampled.is_some() {
                            OcrStatus::Sampled
                        } else {
                            OcrStatus::Complete
                        };
                        request.perceptual_hash = Some(format!("{hash:016x}"));
                        request.sampled_from = sampled;
                        request.result = result;
                    }
                    _ => {
                        outbox.fail_ocr(&job.sha, now())?;
                        if job.attempts < 4 {
                            return Ok(());
                        }
                    }
                }
            }
            self.pending_ocr = Some((job.sha, request));
        }
        let (sha, request) = self.pending_ocr.as_ref().expect("prepared OCR");
        let response = self
            .ocr_client
            .put(format!("{}/v1/ocr/{sha}", self.server))
            .bearer_auth(&self.token)
            .json(request)
            .send()
            .await?;
        // Retrying can't change these answers, and would block every still behind it.
        if [
            reqwest::StatusCode::BAD_REQUEST,
            reqwest::StatusCode::CONFLICT,
            reqwest::StatusCode::PAYLOAD_TOO_LARGE,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        ]
        .contains(&response.status())
        {
            eprintln!("device OCR result refused: {}", response.status());
            outbox.discard_ocr(sha)?;
            self.pending_ocr = None;
            return Ok(());
        }
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "OCR upload: {}",
            response.status()
        );
        outbox.finish_ocr(sha, request)?;
        self.pending_ocr = None;
        Ok(())
    }
}

/// Only credentials are live-reloaded: capture policy and device identity remain
/// fixed for this process, preserving the configured app's existing behavior.
pub(crate) struct LiveUploader {
    uploader: Uploader,
    credentials: (String, String),
    seen: Option<Vec<u8>>,
}
impl LiveUploader {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            uploader: Uploader::new(config)?,
            credentials: (config.server_url.clone(), config.token.clone()),
            seen: None,
        })
    }
    pub fn reload(&mut self, dir: &std::path::Path) -> Result<bool> {
        let bytes = match std::fs::read(dir.join("agent.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => b"{}".to_vec(),
            Err(e) => return Err(e.into()),
        };
        if self.seen.as_ref() == Some(&bytes) {
            return Ok(false);
        }
        let config: Config = serde_json::from_slice(&bytes)?;
        let credentials = (config.server_url.clone(), config.token.clone());
        let changed = credentials != self.credentials;
        if changed {
            self.uploader = Uploader::new(&config)?;
            self.credentials = credentials;
        }
        self.seen = Some(bytes);
        Ok(changed)
    }
    pub fn delay(&self) -> std::time::Duration {
        self.uploader.delay()
    }
    pub async fn ocr_once(&mut self, outbox: &mut Outbox, saver: bool) -> Result<()> {
        if self.credentials.1.is_empty() {
            return Ok(());
        }
        self.uploader.ocr_once(outbox, saver).await
    }
    pub async fn once(&mut self, outbox: &mut Outbox) -> Result<()> {
        if self.credentials.1.is_empty() {
            return Ok(());
        }
        self.uploader.once(outbox).await
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    use axum::{Router, http::StatusCode, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn removing_config_stops_uploads_with_the_old_token() {
        let dir = tempfile::tempdir().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let hits = requests.clone();
        let app = Router::new().route(
            "/v1/events",
            post(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::Relaxed);
                    StatusCode::UNAUTHORIZED
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = Config {
            server_url: format!("http://{}", listener.local_addr().unwrap()),
            token: "synthetic-token".into(),
            device_id: "synthetic".into(),
            ..Default::default()
        };
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        config.save_at(dir.path()).unwrap();
        let mut live = LiveUploader::new(&config).unwrap();
        assert!(!live.reload(dir.path()).unwrap());
        std::fs::remove_file(dir.path().join("agent.json")).unwrap();
        let changed = live.reload(dir.path()).unwrap();
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, 100_000).unwrap();
        outbox
            .enqueue(
                &crate::envelope::Producer::new("synthetic".into())
                    .unwrap()
                    .calibration(),
                &[],
            )
            .unwrap();
        let result = live.once(&mut outbox).await;
        server.abort();
        assert!(changed, "removing agent.json must clear live credentials");
        assert!(result.is_ok());
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        assert!(
            outbox.batch().unwrap().is_some(),
            "unpaired data stays queued"
        );
    }
}

#[cfg(test)]
mod device_ocr_tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Bytes,
        http::StatusCode,
        routing::{get, post, put},
    };
    use hx_schema::{kinds::*, *};

    fn still(shade: u8) -> (Envelope, (BlobRef, Vec<u8>)) {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(32, 32, image::Luma([shade])))
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let bytes = jpeg.into_inner();
        let blob = BlobRef {
            sha256: sha256_hex(&bytes),
            media_type: "image/jpeg".into(),
            bytes: bytes.len() as u64,
        };
        let mut e = crate::envelope::Producer::new("synthetic".into())
            .unwrap()
            .core(
                ulid::Ulid::generate().to_string(),
                "0",
                Operation::Observe,
                CorePayload::MediaStill(MediaStill {
                    display: "main".into(),
                    trigger: StillTrigger::FocusChange,
                    width: 32,
                    height: 32,
                    app_bundle_id: Some("synthetic.app".into()),
                    window_title: None,
                }),
            );
        e.blobs.push(blob.clone());
        (e, (blob, bytes))
    }

    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    const TEXT: &str = "SYNTHETIC_OCR_PRIVATE_ORCHID_472";
    #[derive(Default)]
    struct FakeServer {
        puts: Mutex<Vec<(String, serde_json::Value)>>,
        unavailable: AtomicBool,
        redirect: AtomicBool,
        reject: AtomicBool,
        refuse: AtomicBool,
    }
    struct Engine {
        calls: AtomicUsize,
        fail: bool,
    }
    impl Engine {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                fail,
            })
        }
    }
    impl OcrEngine for Engine {
        fn ocr(&self, _: &[u8], _: &str) -> Result<hx_visual::OcrResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                anyhow::bail!(TEXT);
            }
            Ok(hx_visual::OcrResult {
                lines: vec![hx_visual::OcrLine {
                    text: TEXT.into(),
                    confidence: 0.9,
                    bbox: hx_visual::BoundingBox {
                        x: 0.0,
                        y: 0.0,
                        width: 1.0,
                        height: 1.0,
                    },
                }],
                engine_name: "apple-vision".into(),
                engine_version: "synthetic".into(),
                coverage: "partial".into(),
            })
        }
    }
    async fn tick(
        uploader: &mut Uploader,
        outbox: &mut Outbox,
        engine: &Arc<Engine>,
        now: i64,
        saver: bool,
    ) -> Result<()> {
        uploader
            .ocr_tick(outbox, saver, || now, engine.clone(), "vision:synthetic")
            .await
    }

    async fn fake_server(
        mode: &'static str,
    ) -> (Config, tokio::task::JoinHandle<()>, Arc<FakeServer>) {
        let state = Arc::new(FakeServer::default());
        let events = state.clone();
        let ocr = state.clone();
        let app = Router::new()
            .route(
                "/v1/device",
                get(move || async move {
                    if mode == "old" {
                        (StatusCode::NOT_FOUND, Json(serde_json::json!({})))
                    } else {
                        (StatusCode::OK, Json(serde_json::json!({"ocr": mode})))
                    }
                }),
            )
            .route("/v1/blobs/{sha}", put(|| async { StatusCode::CREATED }))
            .route(
                "/v1/events",
                post(move |body: Bytes| {
                    let events = events.clone();
                    async move {
                        let decoded = zstd::decode_all(body.as_ref()).unwrap();
                        let batch: UploadBatch = serde_json::from_slice(&decoded).unwrap();
                        let reject = events.reject.load(Ordering::Relaxed);
                        Json(UploadAck {
                            batch_id: batch.batch_id,
                            accepted: if reject {
                                0
                            } else {
                                batch.envelopes.len() as u32
                            },
                            duplicates: 0,
                            rejected: if reject {
                                batch
                                    .envelopes
                                    .iter()
                                    .map(|e| Rejected {
                                        observation_id: e.observation_id,
                                        reason: "synthetic refusal".into(),
                                    })
                                    .collect()
                            } else {
                                vec![]
                            },
                        })
                    }
                }),
            )
            .route(
                "/v1/ocr/{sha}",
                put(
                    move |axum::extract::Path(sha): axum::extract::Path<String>,
                          headers: axum::http::HeaderMap,
                          Json(request): Json<serde_json::Value>| {
                        let ocr = ocr.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer synthetic");
                            ocr.puts.lock().unwrap().push((sha, request));
                            if ocr.refuse.load(Ordering::Relaxed) {
                                (StatusCode::CONFLICT, "")
                            } else if ocr.unavailable.load(Ordering::Relaxed) {
                                (StatusCode::SERVICE_UNAVAILABLE, TEXT)
                            } else if ocr.redirect.load(Ordering::Relaxed) {
                                (StatusCode::FOUND, "")
                            } else {
                                (StatusCode::OK, "")
                            }
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = Config {
            server_url: format!("http://{}", listener.local_addr().unwrap()),
            token: "synthetic".into(),
            ..Default::default()
        };
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (config, task, state)
    }

    #[tokio::test]
    async fn device_ack_survives_crash_while_server_and_404_keep_legacy_behavior() {
        for mode in ["old", "server", "device"] {
            let (config, server, state) = fake_server(mode).await;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("outbox.sqlite");
            let mut outbox = Outbox::open(&path, 100_000, 100_000).unwrap();
            let (e, b) = still(20);
            outbox.enqueue(&e, std::slice::from_ref(&b)).unwrap();
            Uploader::new(&config)
                .unwrap()
                .once(&mut outbox)
                .await
                .unwrap();
            drop(outbox); // process death after durable ACK, before any OCR PUT
            let mut outbox = Outbox::open(&path, 100_000, 100_000).unwrap();
            let retained = outbox.blob(&b.0.sha256).unwrap().is_some();
            let engine = Engine::new(false);
            let mut restarted = Uploader::new(&config).unwrap();
            tick(&mut restarted, &mut outbox, &engine, 0, true)
                .await
                .unwrap();
            assert!(state.puts.lock().unwrap().is_empty(), "Saver must skip OCR");
            tick(&mut restarted, &mut outbox, &engine, 0, false)
                .await
                .unwrap();
            tick(&mut restarted, &mut outbox, &engine, 5000, false)
                .await
                .unwrap();
            let puts = state.puts.lock().unwrap();
            assert_eq!(puts.len(), usize::from(mode == "device"));
            if mode == "device" {
                assert_eq!(puts[0].0, b.0.sha256);
                assert_eq!(puts[0].1["status"], "complete");
                assert_eq!(puts[0].1["result"]["lines"][0]["text"], TEXT);
                let mut forged = puts[0].1.clone();
                forged["class"] = "normal".into();
                assert!(
                    serde_json::from_value::<OcrRequest>(forged).is_err(),
                    "devices cannot grant disclosure"
                );
                assert_eq!(engine.calls.load(Ordering::Relaxed), 1);
                assert!(outbox.next_ocr(i64::MAX).unwrap().is_none());
                assert!(outbox.blob(&b.0.sha256).unwrap().is_none());
            }
            server.abort();
            assert_eq!(outbox.stats().unwrap().0, 0);
            assert_eq!(
                retained,
                mode == "device",
                "ACK blob retention in {mode} mode"
            );
        }
    }

    // The retry clock and engine seam sit below the HTTP interface. Deterministic
    // ticks prove durable attempt accounting without a two-minute wall-clock test.
    #[tokio::test]
    async fn device_engine_retries_are_durable_and_http_outages_spend_no_attempts() {
        let (config, server, state) = fake_server("device").await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbox.sqlite");
        let mut outbox = Outbox::open(&path, 100_000, 100_000).unwrap();
        let (e, blob) = still(30);
        outbox.enqueue(&e, &[blob]).unwrap();
        let mut uploader = Uploader::new(&config).unwrap();
        uploader.once(&mut outbox).await.unwrap();
        let engine = Engine::new(true);
        for attempt in 0..5 {
            tick(&mut uploader, &mut outbox, &engine, attempt * 30000, false)
                .await
                .unwrap();
            if attempt < 4 {
                tick(
                    &mut uploader,
                    &mut outbox,
                    &engine,
                    attempt * 30000 + 29999,
                    false,
                )
                .await
                .unwrap();
                assert_eq!(engine.calls.load(Ordering::Relaxed), attempt as usize + 1);
                drop(outbox);
                outbox = Outbox::open(&path, 100_000, 100_000).unwrap();
                uploader = Uploader::new(&config).unwrap();
            }
        }
        assert!(outbox.next_ocr(i64::MAX).unwrap().is_none());
        let failed = state.puts.lock().unwrap()[0].1.clone();
        assert_eq!(failed["status"], "failed");
        assert!(failed["result"].is_null());
        assert!(!failed.to_string().contains(TEXT));
        assert_eq!(engine.calls.load(Ordering::Relaxed), 5);

        let (e, blob) = still(40);
        outbox.enqueue(&e, &[blob]).unwrap();
        uploader.once(&mut outbox).await.unwrap();
        let engine = Engine::new(false);
        state.unavailable.store(true, Ordering::Relaxed);
        for now in [0, 5000, 10000, 15000, 20000, 25000] {
            let error = tick(&mut uploader, &mut outbox, &engine, now, false)
                .await
                .unwrap_err();
            assert!(!format!("{error:#}").contains(TEXT));
            assert_eq!(outbox.next_ocr(now).unwrap().unwrap().attempts, 0);
        }
        assert_eq!(
            engine.calls.load(Ordering::Relaxed),
            1,
            "retry the in-memory result"
        );
        state.unavailable.store(false, Ordering::Relaxed);
        state.redirect.store(true, Ordering::Relaxed);
        assert!(
            tick(&mut uploader, &mut outbox, &engine, 30000, false)
                .await
                .is_err(),
            "a redirect is not an OCR acknowledgement"
        );
        assert!(outbox.next_ocr(i64::MAX).unwrap().is_some());
        state.redirect.store(false, Ordering::Relaxed);
        tick(&mut uploader, &mut outbox, &engine, 30000, false)
            .await
            .unwrap();
        assert!(outbox.next_ocr(i64::MAX).unwrap().is_none());
        server.abort();
    }

    #[tokio::test]
    async fn device_sampling_uses_only_completed_same_surface_within_thirty_seconds() {
        let (config, server, state) = fake_server("device").await;
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, 100_000).unwrap();
        let mut uploader = Uploader::new(&config).unwrap();
        let engine = Engine::new(false);
        let start = chrono::Utc::now();
        let mut source = None;
        // Uniform JPEGs differ in SHA but share a thumbnail hash. Sampling must
        // never chain via a sampled frame or cross app/display/device boundaries.
        for (i, seconds, device, display, app, expected) in [
            (0, 0, "a", "main", "app", "complete"),
            (1, 30, "a", "main", "app", "sampled"),
            (2, 31, "a", "main", "app", "complete"),
            (3, 32, "b", "main", "app", "complete"),
            (4, 33, "b", "other", "app", "complete"),
            (5, 34, "b", "other", "other.app", "complete"),
        ] {
            let (mut e, blob) = still(50 + i);
            e.device = Some(DeviceId::new(device));
            e.time = ObservationTime::point(start + chrono::Duration::seconds(seconds));
            e.payload["display"] = display.into();
            e.payload["app_bundle_id"] = app.into();
            if i == 0 {
                source = Some(blob.0.sha256.clone());
            }
            outbox.enqueue(&e, &[blob]).unwrap();
            uploader.once(&mut outbox).await.unwrap();
            tick(&mut uploader, &mut outbox, &engine, 0, false)
                .await
                .unwrap();
            let puts = state.puts.lock().unwrap();
            let request = &puts.last().unwrap().1;
            assert_eq!(request["status"], expected);
            if expected == "sampled" {
                assert_eq!(request["sampled_from"], source.as_ref().unwrap().as_str());
                assert!(request["result"].is_null());
            }
        }
        assert_eq!(engine.calls.load(Ordering::Relaxed), 5);
        server.abort();
    }

    #[tokio::test]
    async fn device_evicted_and_rejected_stills_have_no_ocr_job_and_backlog_never_blocks_capture() {
        let (config, server, state) = fake_server("device").await;
        let dir = tempfile::tempdir().unwrap();
        let (a, ba) = still(10);
        let (b, bb) = still(80);
        let budget = ba.1.len().max(bb.1.len()) as u64;
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, budget).unwrap();
        outbox.enqueue(&a, std::slice::from_ref(&ba)).unwrap();
        assert_eq!(
            outbox.enqueue(&b, std::slice::from_ref(&bb)).unwrap().len(),
            1
        );
        state.reject.store(true, Ordering::Relaxed);
        let mut uploader = Uploader::new(&config).unwrap();
        uploader.once(&mut outbox).await.unwrap();
        let engine = Engine::new(false);
        tick(&mut uploader, &mut outbox, &engine, 0, false)
            .await
            .unwrap();
        assert!(outbox.next_ocr(i64::MAX).unwrap().is_none());
        assert!(state.puts.lock().unwrap().is_empty());
        assert!(outbox.blob(&ba.0.sha256).unwrap().is_none());
        assert!(
            outbox.blob(&bb.0.sha256).unwrap().is_some(),
            "dead-letter retention remains intact"
        );
        // A different scratch file tests pressure against an acknowledged OCR ref.
        let mut outbox = Outbox::open(&dir.path().join("pinned.sqlite"), 100_000, budget).unwrap();
        state.reject.store(false, Ordering::Relaxed);
        outbox.enqueue(&a, std::slice::from_ref(&ba)).unwrap();
        uploader.once(&mut outbox).await.unwrap();
        // The queued still is already on the server; only its text is given up.
        assert!(outbox.capture(&b, std::slice::from_ref(&bb)).unwrap());
        assert!(outbox.blob(&ba.0.sha256).unwrap().is_none());
        assert!(outbox.blob(&bb.0.sha256).unwrap().is_some());
        assert!(outbox.next_ocr(i64::MAX).unwrap().is_none());
        assert!(outbox.losses().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn device_refused_result_does_not_block_the_stills_behind_it() {
        let (config, server, state) = fake_server("device").await;
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, 1 << 20).unwrap();
        let (a, ba) = still(10);
        let (b, bb) = still(200);
        outbox.enqueue(&a, std::slice::from_ref(&ba)).unwrap();
        outbox.enqueue(&b, std::slice::from_ref(&bb)).unwrap();
        let mut uploader = Uploader::new(&config).unwrap();
        uploader.once(&mut outbox).await.unwrap();
        uploader.once(&mut outbox).await.unwrap();
        state.refuse.store(true, Ordering::Relaxed);
        let engine = Engine::new(false);
        for _ in 0..2 {
            let _ = tick(&mut uploader, &mut outbox, &engine, 0, false).await;
        }
        let sent: std::collections::BTreeSet<_> = state
            .puts
            .lock()
            .unwrap()
            .iter()
            .map(|(sha, _)| sha.clone())
            .collect();
        assert_eq!(sent.len(), 2, "a refused result must not be retried first");
        assert!(outbox.next_ocr(i64::MAX).unwrap().is_none());
        assert!(outbox.blob(&ba.0.sha256).unwrap().is_none());
        server.abort();
    }

    #[tokio::test]
    async fn device_text_never_reaches_disk_or_logs() {
        const CHILD: &str = "HX_N6_PRIVACY_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "upload::device_ocr_tests::device_text_never_reaches_disk_or_logs",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "privacy child failed");
            for bytes in [&output.stdout, &output.stderr] {
                assert!(
                    !String::from_utf8_lossy(bytes).contains(TEXT),
                    "OCR text escaped into logs"
                );
            }
            return;
        }
        let (config, server, state) = fake_server("device").await;
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, 100_000).unwrap();
        let mut uploader = Uploader::new(&config).unwrap();
        for (shade, fail) in [(10, false), (90, true)] {
            let (mut e, blob) = still(shade);
            e.payload["app_bundle_id"] = shade.to_string().into(); // avoid sampling
            outbox.enqueue(&e, &[blob]).unwrap();
            uploader.once(&mut outbox).await.unwrap();
            state.unavailable.store(true, Ordering::Relaxed);
            let engine = Engine::new(fail);
            for attempt in 0..5 {
                if let Err(error) =
                    tick(&mut uploader, &mut outbox, &engine, attempt * 30000, false).await
                {
                    eprintln!("{error:#}");
                }
            }
            state.unavailable.store(false, Ordering::Relaxed);
            tick(&mut uploader, &mut outbox, &engine, 150000, false)
                .await
                .unwrap();
        }
        // Include live WAL/SHM, not just the main SQLite file.
        for file in std::fs::read_dir(dir.path()).unwrap() {
            let bytes = std::fs::read(file.unwrap().path()).unwrap();
            assert!(
                !bytes
                    .windows(TEXT.len())
                    .any(|window| window == TEXT.as_bytes())
            );
        }
        server.abort();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn device_slow_helper_does_not_block_capture_persistence() {
        const CHILD: &str = "HX_N6_SLOW_HELPER_CHILD";
        if std::env::var_os(CHILD).is_none() {
            use std::os::unix::fs::PermissionsExt;
            let scratch = tempfile::tempdir().unwrap();
            let helper = scratch.path().join("hx-vision-ocr");
            std::fs::write(&helper, r#"#!/bin/sh
/bin/cat >/dev/null
/usr/bin/touch "$HX_N6_HELPER_STARTED"
while [ ! -f "$HX_N6_HELPER_RELEASE" ]; do /bin/sleep 0.1; done
printf '%s' '{"lines":[],"engine_name":"apple-vision","engine_version":"synthetic","coverage":"partial"}'
"#).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "upload::device_ocr_tests::device_slow_helper_does_not_block_capture_persistence", "--nocapture"])
                .env(CHILD, "1").env("HX_VISION_HELPER", &helper)
                .env("HX_N6_HELPER_STARTED", scratch.path().join("started"))
                .env("HX_N6_HELPER_RELEASE", scratch.path().join("release"))
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let (mut config, server, _) = fake_server("device").await;
        config.saver_thermal_threshold = i64::MAX;
        let dir = tempfile::tempdir().unwrap();
        config.save_at(dir.path()).unwrap();
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, 100_000).unwrap();
        let (e, blob) = still(10);
        outbox.enqueue(&e, &[blob]).unwrap();
        Uploader::new(&config)
            .unwrap()
            .once(&mut outbox)
            .await
            .unwrap();
        drop(outbox);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_dir = dir.path().to_owned();
        let worker = std::thread::spawn(move || {
            crate::runtime::io_worker_at(config, rx, worker_stop, worker_dir)
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !std::path::Path::new(&std::env::var_os("HX_N6_HELPER_STARTED").unwrap()).exists()
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let (reply, persisted) = std::sync::mpsc::sync_channel(1);
        tx.send(crate::runtime::Submission {
            envelope: crate::envelope::Producer::new("synthetic".into())
                .unwrap()
                .calibration(),
            reply: Some(reply),
            focus: None,
            secure: false,
            blobs: vec![],
        })
        .await
        .unwrap();
        let result = tokio::task::spawn_blocking(move || {
            persisted.recv_timeout(std::time::Duration::from_secs(1))
        })
        .await
        .unwrap();
        // Always release the helper and join the worker before asserting.
        std::fs::write(
            std::env::var_os("HX_N6_HELPER_RELEASE").unwrap(),
            b"release",
        )
        .unwrap();
        drop(tx);
        let joined = tokio::task::spawn_blocking(move || worker.join().unwrap())
            .await
            .unwrap();
        server.abort();
        joined.unwrap();
        assert!(
            matches!(result, Ok(true)),
            "capture persistence waited for OCR helper"
        );
    }

    #[tokio::test]
    async fn device_engine_retry_cooldown_starts_when_the_helper_finishes() {
        use std::sync::atomic::AtomicI64;
        struct SlowFailure(Arc<AtomicI64>);
        impl OcrEngine for SlowFailure {
            fn ocr(&self, _: &[u8], _: &str) -> Result<hx_visual::OcrResult> {
                std::thread::sleep(std::time::Duration::from_millis(20));
                self.0
                    .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                anyhow::bail!(TEXT)
            }
        }
        let (config, server, _) = fake_server("device").await;
        let dir = tempfile::tempdir().unwrap();
        let mut outbox = Outbox::open(&dir.path().join("outbox.sqlite"), 100_000, 100_000).unwrap();
        let (e, blob) = still(10);
        outbox.enqueue(&e, &[blob]).unwrap();
        let mut uploader = Uploader::new(&config).unwrap();
        uploader.once(&mut outbox).await.unwrap();
        let failed_at = Arc::new(AtomicI64::new(0));
        uploader
            .ocr_tick(
                &mut outbox,
                false,
                || chrono::Utc::now().timestamp_millis(),
                Arc::new(SlowFailure(failed_at.clone())),
                "vision:synthetic",
            )
            .await
            .unwrap();
        server.abort();
        assert!(
            outbox
                .next_ocr(failed_at.load(Ordering::Relaxed) + 29999)
                .unwrap()
                .is_none(),
            "retry was scheduled before the helper finished"
        );
    }
}
