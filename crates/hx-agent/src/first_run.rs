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
    message: Option<String>,
    guide: Option<std::process::Child>,
    guide_checked: Instant,
}
impl Onboarding {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            checked: Instant::now() - Duration::from_secs(1),
            paired: false,
            pending: None,
            message: None,
            guide: None,
            guide_checked: Instant::now() - Duration::from_secs(5),
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
        self.setup_guide();
        if menu.take_open_app() {
            match Config::load_at(&self.dir) {
                Ok(config) => {
                    let url = format!("{}/app", config.server_url.trim_end_matches('/'));
                    if let Err(error) = menu.open_app(&url) {
                        self.message = Some(format!("App could not open: {error}"));
                    }
                }
                Err(error) => {
                    self.message = Some(format!("App configuration unavailable: {error}"))
                }
            }
        }
        if self.checked.elapsed() >= Duration::from_secs(1) {
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
