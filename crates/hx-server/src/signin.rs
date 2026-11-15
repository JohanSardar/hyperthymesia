//! Provider sign-in started from the app: runs `fx login PROVIDER` on this
//! server, opens the provider's own login page in the browser here, and reports
//! when fx holds a login. The login returns to fx on localhost, so it happens on
//! the computer the server runs on. Never sees or stores a password.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const LIMIT: Duration = Duration::from_secs(600);

struct Session {
    provider: String,
    child: Option<Child>,
    url: Arc<Mutex<Option<String>>>,
    opened: bool,
    state: &'static str,
    started: Instant,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

pub fn supported(provider: &str) -> bool {
    matches!(provider, "codex" | "grok")
}

/// Starts a sign-in, replacing any unfinished one.
pub fn start(provider: &str) -> Result<Value> {
    anyhow::ensure!(supported(provider), "this provider signs in with a key");
    let mut slot = SESSION.lock().expect("sign-in");
    if let Some(mut old) = slot.take()
        && let Some(mut child) = old.child.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
    let config = crate::fx::Config::from_env();
    let mut child = Command::new(&config.binary)
        .args(["login", provider])
        .env("FX_NO_OPEN_BROWSER", "1")
        .env("FX_AUTO_UPGRADE", "0")
        .env("FX_DISABLE_KEYCHAIN", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start the provider sign-in")?;
    let url = Arc::new(Mutex::new(None));
    for stream in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let url = url.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stream)
                .lines()
                .map_while(Result::ok)
            {
                if let Some(found) = line.split_whitespace().find(|w| w.starts_with("https://")) {
                    let mut slot = url.lock().expect("url");
                    if slot.is_none() {
                        *slot = Some(found.to_owned());
                    }
                }
            }
        });
    }
    *slot = Some(Session {
        provider: provider.to_owned(),
        child: Some(child),
        url,
        opened: false,
        state: "waiting",
        started: Instant::now(),
    });
    drop(slot);
    // Give fx a moment to print its page, so the first answer can carry it.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = status(provider);
        if value["url"].is_string() || value["state"] != "waiting" || Instant::now() > deadline {
            return Ok(value);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// The current sign-in for `provider`: idle, waiting, done or failed.
pub fn status(provider: &str) -> Value {
    let mut slot = SESSION.lock().expect("sign-in");
    let Some(session) = slot.as_mut().filter(|s| s.provider == provider) else {
        return json!({"provider":provider,"state":"idle","signed_in":crate::fx::login_present(provider)});
    };
    let url = session.url.lock().expect("url").clone();
    if !session.opened
        && let Some(url) = &url
    {
        session.opened = true;
        crate::guided_setup::open_url(url);
    }
    if let Some(child) = session.child.as_mut() {
        match child.try_wait() {
            Ok(Some(exit)) => {
                session.child = None;
                session.state = if exit.success() && crate::fx::login_present(provider) {
                    "done"
                } else {
                    "failed"
                };
            }
            Ok(None) if session.started.elapsed() > LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                session.child = None;
                session.state = "failed";
            }
            Ok(None) => {}
            Err(_) => {
                session.child = None;
                session.state = "failed";
            }
        }
    }
    json!({"provider":provider,"state":session.state,"url":url,"signed_in":crate::fx::login_present(provider)})
}

pub fn cancel(provider: &str) {
    let mut slot = SESSION.lock().expect("sign-in");
    if slot.as_ref().is_some_and(|s| s.provider == provider)
        && let Some(mut session) = slot.take()
        && let Some(mut child) = session.child.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}
