use crate::{config::Config, outbox::Outbox};
use anyhow::{Result, ensure};
use hx_schema::UploadAck;
pub struct Uploader {
    client: reqwest::Client,
    server: String,
    token: String,
    pub failures: u32,
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
            server: config.server_url.trim_end_matches('/').into(),
            token: config.token.clone(),
            failures: 0,
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
    async fn send(&self, outbox: &mut Outbox) -> Result<()> {
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
        outbox.ack(&ack)
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
