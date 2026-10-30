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
