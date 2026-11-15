//! Pause and never-record. This Mac keeps its own copy in `recording.json`, so a
//! pause holds across restarts and while the server is unreachable. The server's
//! `/v1/recording` record is what the web app and this Mac agree on: changes
//! made here are sent there, and changes made there are picked up here.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct App {
    pub bundle_id: String,
    pub name: String,
}
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct NeverRecord {
    pub apps: Vec<App>,
    pub sites: Vec<String>,
}
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
#[serde(default)]
struct Seen {
    baseline: bool,
    account_ready: Option<DateTime<Utc>>,
    failed_pass: Option<i64>,
    failed_pull: Option<i64>,
}
#[derive(Clone, Copy, Default, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Granted {
    pub accessibility: bool,
    pub screen: bool,
    pub input: bool,
}
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
#[serde(default)]
struct State {
    paused_until: Option<DateTime<Utc>>,
    never_record: NeverRecord,
    /// Changes made on this Mac that the server has not confirmed yet.
    pause_unsent: bool,
    never_unsent: bool,
    /// Content-free markers already shown as notifications.
    seen: Seen,
    /// Permissions as last seen, so a revocation is noticed across restarts.
    granted: Option<Granted>,
}

/// One desktop notification: id, title, body and what a click opens.
pub type Notice = (String, String, String, &'static str);

#[derive(Clone)]
pub struct Controls {
    path: Arc<PathBuf>,
    state: Arc<Mutex<State>>,
    /// The server's Recording switch: 0 not heard yet, 1 on, 2 off.
    server_switch: Arc<std::sync::atomic::AtomicU8>,
}
impl Controls {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("recording.json");
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let this = Self {
            path: Arc::new(path),
            state: Arc::new(Mutex::new(state)),
            server_switch: Arc::default(),
        };
        this.apply();
        this
    }
    fn with<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().expect("recording controls");
        let before = serde_json::to_value(&*state).ok();
        let out = change(&mut state);
        if before != serde_json::to_value(&*state).ok() {
            if let Err(error) = save(&self.path, &state) {
                eprintln!("cannot save recording controls: {error}");
            }
            crate::policy::set_never_record(&state.never_record);
        }
        out
    }
    fn apply(&self) {
        crate::policy::set_never_record(
            &self.state.lock().expect("recording controls").never_record,
        );
    }
    /// True only once the server has said its Recording switch is off.
    pub fn switched_off(&self) -> bool {
        self.server_switch
            .load(std::sync::atomic::Ordering::Relaxed)
            == 2
    }
    /// When the current pause ends, if recording is paused now.
    pub fn paused_until(&self) -> Option<DateTime<Utc>> {
        self.state
            .lock()
            .expect("recording controls")
            .paused_until
            .filter(|until| *until > Utc::now())
    }
    /// `None` resumes recording.
    pub fn pause(&self, until: Option<DateTime<Utc>>) {
        self.with(|s| {
            s.paused_until = until;
            s.pause_unsent = true;
        });
    }
    pub fn never_records(&self, bundle_id: &str) -> bool {
        self.state
            .lock()
            .expect("recording controls")
            .never_record
            .apps
            .iter()
            .any(|a| a.bundle_id == bundle_id)
    }
    pub fn toggle_never_record(&self, bundle_id: &str, name: &str) {
        self.with(|s| {
            let apps = &mut s.never_record.apps;
            if let Some(i) = apps.iter().position(|a| a.bundle_id == bundle_id) {
                apps.remove(i);
            } else {
                apps.push(App {
                    bundle_id: bundle_id.into(),
                    name: name.into(),
                });
            }
            s.never_unsent = true;
        });
    }
    /// Returns the permissions that were on last time and are off now.
    pub fn permissions_seen(&self, now: Granted) -> Vec<&'static str> {
        let before = self.state.lock().expect("recording controls").granted;
        if before == Some(now) {
            return vec![];
        }
        self.with(|s| s.granted = Some(now));
        let Some(before) = before else { return vec![] };
        [
            ("Accessibility", before.accessibility && !now.accessibility),
            ("Screen Recording", before.screen && !now.screen),
            ("Input Monitoring", before.input && !now.input),
        ]
        .into_iter()
        .filter_map(|(name, lost)| lost.then_some(name))
        .collect()
    }

    /// Takes the server's view, unless this Mac has a change it has not sent.
    /// Returns the notifications the person has not seen yet.
    pub fn adopt(&self, value: &serde_json::Value) -> Vec<Notice> {
        // An older server answers only `enabled`; it must not clear a pause.
        if value.get("changed_at").is_none() {
            return vec![];
        }
        self.with(|s| {
            if !s.pause_unsent {
                s.paused_until =
                    serde_json::from_value(value["paused_until"].clone()).unwrap_or(None);
            }
            if !s.never_unsent
                && let Ok(rules) = serde_json::from_value(value["never_record"].clone())
            {
                s.never_record = rules;
            }
            notices(&mut s.seen, &value["notices"])
        })
    }
    /// What this Mac still has to send: the pause body and the rules body.
    pub fn unsent(&self) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
        let s = self.state.lock().expect("recording controls");
        (
            s.pause_unsent
                .then(|| serde_json::json!({"until": s.paused_until})),
            s.never_unsent.then(|| serde_json::json!(s.never_record)),
        )
    }
    /// The server stored what was sent; adopt its answer unless something
    /// changed here again in the meantime.
    pub fn sent(&self, pause: bool, sent: &serde_json::Value) {
        self.with(|s| {
            let current = if pause {
                serde_json::json!({"until": s.paused_until})
            } else {
                serde_json::json!(s.never_record)
            };
            if &current == sent {
                if pause {
                    s.pause_unsent = false
                } else {
                    s.never_unsent = false
                }
            }
        });
    }
}

fn notices(seen: &mut Seen, v: &serde_json::Value) -> Vec<Notice> {
    let ready: Option<(DateTime<Utc>, DateTime<Utc>)> =
        serde_json::from_value(v["account_ready"]["start"].clone())
            .ok()
            .zip(serde_json::from_value(v["account_ready"]["end"].clone()).ok());
    let pass = v["failed_pass"].as_i64();
    let pull = v["failed_pull"]["id"].as_i64();
    // The first answer only sets the baseline: never announce old news.
    let first = !seen.baseline;
    seen.baseline = true;
    let mut out = vec![];
    if let Some((start, end)) = ready
        && seen.account_ready.is_none_or(|last| start > last)
    {
        seen.account_ready = Some(start);
        if !first {
            let local =
                |t: DateTime<Utc>| t.with_timezone(&chrono::Local).format("%H:%M").to_string();
            out.push((
                format!("ready-{}", start.timestamp()),
                "Your day so far is written up".into(),
                format!("{}–{} is ready to read on Today.", local(start), local(end)),
                "today",
            ));
        }
    }
    if let Some(id) = pass
        && seen.failed_pass.is_none_or(|last| id > last)
    {
        seen.failed_pass = Some(id);
        if !first {
            out.push((
                format!("pass-{id}"),
                "A write-up run failed".into(),
                "Part of your day couldn’t be written up. Open Runs to see why.".into(),
                "runs",
            ));
        }
    }
    if let Some(id) = pull
        && seen.failed_pull.is_none_or(|last| id > last)
    {
        seen.failed_pull = Some(id);
        if !first {
            let name = v["failed_pull"]["recipe_id"]
                .as_str()
                .unwrap_or("A connection");
            out.push((
                format!("pull-{id}"),
                "A connection run failed".into(),
                format!("{name} couldn’t fetch new data. Open Runs to see why."),
                "runs",
            ));
        }
    }
    out
}

fn save(path: &Path, state: &State) -> anyhow::Result<()> {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let dir = path.parent().expect("support dir");
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    tmp.write_all(&serde_json::to_vec_pretty(state)?)?;
    tmp.persist(path)?;
    Ok(())
}

/// Keeps this Mac and the server in step. Runs on the I/O worker's runtime.
pub async fn sync(
    controls: Controls,
    dir: PathBuf,
    fallback: crate::config::Config,
    recording: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::{sync::atomic::Ordering, time::Duration};
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return;
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut turn = 0u64;
    let mut unreachable = false;
    loop {
        tick.tick().await;
        turn += 1;
        let (pause, never) = controls.unsent();
        // Changes made here go out within a second; otherwise check every 5 s.
        if (unreachable || (pause.is_none() && never.is_none())) && !turn.is_multiple_of(5) {
            continue;
        }
        let config = crate::config::Config::load_at(&dir).unwrap_or_else(|_| fallback.clone());
        if config.token.is_empty() {
            continue;
        }
        let base = format!("{}/v1/recording", config.server_url.trim_end_matches('/'));
        for (is_pause, body) in [(true, pause), (false, never)] {
            let Some(body) = body else { continue };
            let url = format!("{base}/{}", if is_pause { "pause" } else { "never-record" });
            match client
                .put(url)
                .bearer_auth(&config.token)
                .json(&body)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => controls.sent(is_pause, &body),
                // An older server: keep the change here and retry slowly.
                _ => unreachable = true,
            }
        }
        let answer = client.get(&base).bearer_auth(&config.token).send().await;
        unreachable = answer.is_err() || unreachable && controls.unsent() != (None, None);
        if let Ok(response) = answer
            && response.status().is_success()
            && let Ok(value) = response.json::<serde_json::Value>().await
        {
            if let Some(enabled) = value["enabled"].as_bool() {
                recording.store(enabled, Ordering::Relaxed);
                controls
                    .server_switch
                    .store(if enabled { 1 } else { 2 }, Ordering::Relaxed);
            }
            for (id, title, body, target) in controls.adopt(&value) {
                hx_macos_sys::notify(&id, &title, &body, target);
            }
        }
    }
}
