use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
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
            device_id: if cfg!(feature = "e2e-harness") {
                "synthetic-e2e".into()
            } else {
                hx_macos_sys::host_name()
            },
            excluded_bundle_ids: vec![],
            excluded_title_regexes: vec![],
            event_budget_bytes: 64 * 1024 * 1024,
            media_budget_bytes: 1024 * 1024 * 1024,
            idle_threshold_s: 300,
            still_cadence_s: 60,
            still_budget_bytes_per_hour: 64 * 1024 * 1024,
            saver_thermal_threshold: 2,
        }
    }
}
pub fn support_dir() -> PathBuf {
    #[cfg(feature = "e2e-harness")]
    if let Some(dir) = std::env::var_os("HX_E2E_SUPPORT_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(
        if cfg!(feature = "e2e-harness") {
            "Library/Application Support/Hyperthymesia-e2e"
        } else {
            "Library/Application Support/Hyperthymesia"
        },
    )
}
impl Config {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_at(&support_dir())
    }
    pub fn load_at(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("agent.json");
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    /// Merge only pairing credentials, preserving policy and unknown future fields.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_at(&support_dir())
    }
    pub fn save_at(&self, dir: &Path) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir)?;
        let path = dir.join("agent.json");
        let mut value = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(e) => return Err(e.into()),
        };
        let object = value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("agent.json must be an object"))?;
        object.insert("server_url".into(), self.server_url.clone().into());
        object.insert("token".into(), self.token.clone().into());
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        tmp.write_all(&serde_json::to_vec_pretty(&value)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&path)?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
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
