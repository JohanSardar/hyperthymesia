use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_url: String,
    pub token: String,
    pub device_id: String,
    pub excluded_bundle_ids: Vec<String>,
    pub excluded_title_regexes: Vec<String>,
    pub event_budget_bytes: u64,
    pub media_budget_bytes: u64,
    pub idle_threshold_s: u32,
    pub still_cadence_s: u64,
    pub still_budget_bytes_per_hour: u64,
    pub saver_thermal_threshold: i64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:8080".into(),
            token: String::new(),
            device_id: hx_macos_sys::host_name(),
            excluded_bundle_ids: vec![],
            excluded_title_regexes: vec![],
            event_budget_bytes: 64 * 1024 * 1024,
            media_budget_bytes: 1024 * 1024 * 1024,
            idle_threshold_s: 300,
            still_cadence_s: 30,
            still_budget_bytes_per_hour: 64 * 1024 * 1024,
            saver_thermal_threshold: 2,
        }
    }
}
pub fn support_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
        .join("Library/Application Support/Hyperthymesia")
}
impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = support_dir().join("agent.json");
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Profile {
    Full,
    Balanced,
    Saver,
}
impl Profile {
    pub fn select(power: hx_macos_sys::Power, config: &Config) -> Self {
        if power.thermal >= config.saver_thermal_threshold {
            Self::Saver
        } else if power.battery {
            Self::Balanced
        } else {
            Self::Full
        }
    }
    pub fn text_interval(self) -> u64 {
        match self {
            Self::Full => 10,
            Self::Balanced => 30,
            Self::Saver => 120,
        }
    }
}
