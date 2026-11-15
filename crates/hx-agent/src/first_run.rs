use crate::config::Config;
use hx_macos_sys::MenuBar;
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(crate) struct Onboarding {
    dir: PathBuf,
    checked: Instant,
    paired: bool,
    pending: Option<mpsc::Receiver<anyhow::Result<()>>>,
    remote: Option<mpsc::Receiver<anyhow::Result<()>>>,
    message: Option<String>,
    guide: Option<std::process::Child>,
    guide_checked: Instant,
    started: bool,
    reopen_after_guide: bool,
}
impl Onboarding {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            checked: Instant::now() - Duration::from_secs(1),
            paired: false,
            pending: None,
            remote: None,
            message: None,
            guide: None,
            guide_checked: Instant::now() - Duration::from_secs(5),
            started: false,
            reopen_after_guide: false,
        }
    }
    /// Starts the setup guide as this app's child, so macOS attributes its privacy
    /// requests to Hyperthymesia. Also resumes a run that was quit halfway.
    fn setup_guide(&mut self) {
        if cfg!(feature = "e2e-harness") || self.guide_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.guide_checked = Instant::now();
        if let Some(child) = &mut self.guide {
            match child.try_wait() {
                Ok(None) => return,
                _ => self.guide = None,
            }
        }
        if !crate::setup::guide_wanted(&self.dir.join("setup")) {
            return;
        }
        let Some(guide) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("hx-setup-guide")))
            .filter(|path| path.is_file())
        else {
            return;
        };
        use std::os::unix::process::CommandExt as _;
        // Its own process group, so it outlives the app restart it asks for.
        match std::process::Command::new(guide)
            .process_group(0)
            .env("HX_SETUP_DIR", self.dir.join("setup"))
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => self.guide = Some(child),
            Err(error) => self.message = Some(format!("Setup guide could not start: {error}")),
        }
    }
    pub fn update(&mut self, menu: &mut MenuBar, warning: Option<&str>) {
        if !self.started {
            self.started = true;
            // The setup guide restarted the app so a new grant counts. Opening
            // the window now would cover the System Settings box the person
            // still has to answer.
            let marker = self.dir.join("setup/restarting");
            if std::fs::metadata(&marker)
                .and_then(|m| m.modified())
                .is_ok_and(|at| at.elapsed().is_ok_and(|age| age < Duration::from_secs(120)))
            {
                let _ = menu.take_open_app();
                // Setup in the window asked for this guide: bring the window
                // back once the guide is done, so setup carries on there.
                self.reopen_after_guide = std::fs::read(self.dir.join("setup/request.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .and_then(|v| v["run"].as_str().map(|run| run.starts_with("app-")))
                    .unwrap_or(false);
            }
            let _ = std::fs::remove_file(marker);
        }
        self.setup_guide();
        if let Some(page) = menu.take_open_app() {
            match Config::load_at(&self.dir) {
                Ok(config) => {
                    let base = format!("{}/app", config.server_url.trim_end_matches('/'));
                    let url = if page.is_empty() {
                        base
                    } else {
                        format!("{base}/{page}")
                    };
                    let session = desktop_session(&self.dir, &config);
                    if let Err(error) = menu.open_app(&url, session.as_deref()) {
                        self.message = Some(format!("App could not open: {error}"));
                    }
                }
                Err(error) => {
                    self.message = Some(format!("App configuration unavailable: {error}"))
                }
            }
        }
        if self.checked.elapsed() >= Duration::from_secs(1) {
            if self.reopen_after_guide && !crate::setup::guide_wanted(&self.dir.join("setup")) {
                self.reopen_after_guide = false;
                menu.request_page("setup");
            }
            // A malformed edit must not claim that a configured device is unpaired.
            if let Ok(cfg) = Config::load_at(&self.dir) {
                self.paired = !cfg.token.is_empty();
                if self.paired && self.pending.is_none() {
                    self.message = None;
                }
            }
            self.checked = Instant::now();
        }
        if let Some(result) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pending = None;
            self.message = match result {
                Ok(()) => {
                    self.paired = true;
                    None
                }
                Err(e) => Some(format!("Pairing failed: {e}")),
            };
        }
        // The app's setup page asked to send recording to another computer.
        let request = self.dir.join("pair-request.json");
        if self.pending.is_none() && self.remote.is_none() && request.is_file() {
            let link = std::fs::read(&request)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v["link"].as_str().map(str::to_owned));
            let _ = std::fs::remove_file(&request);
            if let Some(link) = link {
                let (tx, rx) = mpsc::channel();
                let dir = self.dir.clone();
                self.remote = Some(rx);
                self.message = Some("Pairing…".into());
                std::thread::spawn(move || {
                    let result = (|| -> anyhow::Result<()> {
                        tokio::runtime::Runtime::new()?
                            .block_on(crate::pair::redeem(&link, true, &dir))
                    })();
                    let _ = tx.send(result);
                });
            }
        }
        if let Some(result) = self.remote.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.remote = None;
            self.message = None;
            let value = match &result {
                Ok(()) => serde_json::json!({"state":"done"}),
                Err(error) => serde_json::json!({"state":"failed","error":format!("{error:#}")}),
            };
            let _ = std::fs::write(
                self.dir.join("pair-result.json"),
                serde_json::to_vec(&value).unwrap_or_default(),
            );
        }
        if let Some(link) = menu.take_pair_link()
            && self.pending.is_none()
        {
            match link {
                Ok(link) => {
                    let (tx, rx) = mpsc::channel();
                    let dir = self.dir.clone();
                    self.pending = Some(rx);
                    self.message = Some("Pairing…".into());
                    std::thread::spawn(move || {
                        let result = (|| -> anyhow::Result<()> {
                            tokio::runtime::Runtime::new()?
                                .block_on(crate::pair::redeem(&link, false, &dir))
                        })();
                        let _ = tx.send(result);
                    });
                }
                Err(e) => self.message = Some(e),
            }
        }
        menu.set_paired(self.paired);
        menu.update(self.message.as_deref().or(warning));
    }
}

/// Read only same-user local server credentials. Never turn an ingest token into
/// a reader token, or send the local owner's credential to a remote server.
fn desktop_session(dir: &std::path::Path, config: &Config) -> Option<String> {
    let server = reqwest::Url::parse(&config.server_url).ok()?;
    let host = server.host_str()?;
    if config.token.is_empty()
        || server.scheme() != "http"
        || !host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        return None;
    }
    let mut state = dir.join("state");
    if !cfg!(feature = "e2e-harness") {
        let pointer = std::path::PathBuf::from(std::env::var_os("HOME")?)
            .join(".local/share/hyperthymesia/.state-dir");
        if let Ok(path) = std::fs::read_to_string(pointer) {
            state = path.trim().into();
        }
    }
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join("desktop-session.json")).ok()?).ok()?;
    let session_server = reqwest::Url::parse(value["server_url"].as_str()?).ok()?;
    if server.origin() != session_server.origin() {
        return None;
    }
    let token = value["token"].as_str()?;
    (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())).then(|| token.to_owned())
}
