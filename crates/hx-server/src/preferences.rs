//! Durable owner preferences shared by HTTP clients and the fx runner.
use anyhow::Result;
use hx_store::Store;
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub model: Option<Model>,
    pub mac_recording: bool,
    pub chunk_retention_days: u32,
    pub appearance: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            model: None,
            mac_recording: true,
            chunk_retention_days: 30,
            appearance: "dark".into(),
        }
    }
}
pub async fn load(store: &Store) -> Result<Preferences> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM app_settings WHERE id=1")
        .fetch_optional(&store.pool)
        .await?;
    match value {
        Some(value) => Ok(serde_json::from_str(&value)?),
        None => {
            let mut preferences = Preferences::default();
            // Explicit CLI installations remain usable. There is no implicit model.
            if let (Ok(provider), Ok(model)) = (
                std::env::var("HX_FX_PROVIDER"),
                std::env::var("HX_FX_MODEL"),
            ) && !provider.is_empty()
                && !model.is_empty()
            {
                preferences.model = Some(Model {
                    provider: if provider == "gateway" {
                        "vercel".into()
                    } else {
                        provider
                    },
                    model,
                    effort: std::env::var("HX_FX_EFFORT").ok().filter(|v| !v.is_empty()),
                });
            }
            Ok(preferences)
        }
    }
}
pub async fn save(store: &Store, value: &Preferences) -> Result<()> {
    let mut tx = store.begin_write().await?;
    sqlx::query("INSERT INTO app_settings(id,value) VALUES(1,$1) ON CONFLICT(id) DO UPDATE SET value=excluded.value")
        .bind(serde_json::to_string(value)?).execute(&mut *tx).await?;
    tx.commit().await
}
pub fn gateway_key(store: &Store) -> Result<Option<String>> {
    Ok(hx_sources::keys::KeyStore::new(&store.data_dir)?
        .read("ai-gateway")?
        .or_else(|| {
            std::env::var("AI_GATEWAY_API_KEY")
                .ok()
                .filter(|v| v.len() >= 4 && v.trim() == v && !v.chars().any(char::is_control))
        }))
}
