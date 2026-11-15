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
/// How far the person got in the app's first-run setup.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupProgress {
    /// The step to resume at, for example `permissions`.
    pub step: String,
    pub finished: bool,
    /// Whether the guided tour of the app still has to be shown.
    pub tour_pending: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub model: Option<Model>,
    pub mac_recording: bool,
    pub chunk_retention_days: u32,
    pub appearance: String,
    /// Null for installations from before in-app setup: they count as set up
    /// once a model is chosen.
    pub setup: Option<SetupProgress>,
    /// Monthly spending limit in US dollars; none by default.
    pub spend_limit_usd: Option<f64>,
    /// `balanced` lets the Mac pick its pace from power and heat; `saver`
    /// keeps it at its slowest pace all the time.
    pub recording_profile: String,
    /// Days to keep each type of record (see `admin_api::TYPES`); a missing
    /// type is kept forever.
    pub retention: std::collections::BTreeMap<String, u32>,
    pub notifications: Notifications,
    pub density: String,
    pub font_size: u32,
    /// Command ID to key combination, for commands the owner rebound.
    pub shortcuts: std::collections::BTreeMap<String, String>,
    /// How many days back a new connection's first pull reaches by default.
    pub connection_history_days: u32,
    /// The HTTPS address that reaches this server from elsewhere, if the owner
    /// set one up (for example with Tailscale). Used in pairing links.
    pub remote_url: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    pub day_ready: bool,
    pub failed_run: bool,
    pub failed_pull: bool,
    pub permission_lost: bool,
}
impl Default for Notifications {
    fn default() -> Self {
        Self {
            day_ready: true,
            failed_run: true,
            failed_pull: true,
            permission_lost: true,
        }
    }
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            model: None,
            mac_recording: true,
            chunk_retention_days: 30,
            appearance: "dark".into(),
            setup: None,
            spend_limit_usd: None,
            recording_profile: "balanced".into(),
            retention: Default::default(),
            notifications: Notifications::default(),
            density: "comfortable".into(),
            font_size: 14,
            shortcuts: Default::default(),
            connection_history_days: 1,
            remote_url: None,
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
