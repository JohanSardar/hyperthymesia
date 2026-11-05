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
}
impl Onboarding {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            checked: Instant::now() - Duration::from_secs(1),
            paired: false,
            pending: None,
            message: None,
        }
    }
    pub fn update(&mut self, menu: &mut MenuBar, warning: Option<&str>) {
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
