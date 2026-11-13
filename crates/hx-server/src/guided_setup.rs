//! `hx-server setup`: do every first-run step a machine can do, hand the few
//! human-only steps to the Mac app's docked guide, then report exactly what was
//! allowed, skipped or refused, proven by real results rather than messages.
//!
//! Never clicks, approves or types into a system prompt for the person.
use anyhow::{Context, Result, bail};
use hx_store::Store;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const SERVER: &str = "http://127.0.0.1:7377";
const LOGIN_LINK_TTL_S: u64 = 300;
const DASHBOARD_TOKEN: &str = "dashboard (setup)";

pub struct Options {
    pub json: bool,
    pub open: bool,
    pub timeout: Duration,
}

/// One line per change, for a person or (with --json) for the agent that ran
/// setup. `say` is the only thing an agent should tell the person.
struct Out {
    json: bool,
}
impl Out {
    fn event(&self, step: &str, state: &str, say: &str) {
        if self.json {
            println!(
                "{}",
                json!({"event":"step","step":step,"state":state,"say":say})
            );
        } else if !say.is_empty() {
            println!("{say}");
        }
        let _ = std::io::stdout().flush();
    }
}

pub async fn run(store: &Store, options: Options) -> Result<i32> {
    let out = Out { json: options.json };
    let started = Instant::now();
    let mut report = serde_json::Map::new();

    // 1. The service. Start it when it is installed but stopped.
    if !healthy() {
        start_service();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !healthy() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    if !healthy() {
        bail!("the Hyperthymesia service is not answering on {SERVER}; rerun the installer");
    }
    report.insert("service".into(), json!({"state":"done"}));

    // 2. Pair the Mac app with this server (macOS only).
    let app = app_bundle();
    if let Some(app) = &app {
        let state = pair_app(store, app).await?;
        report.insert("pairing".into(), json!({"state":state}));
    }

    // 3. Provider sign-in: needed only when doctor fails for a missing login.
    let provider = std::env::var("HX_FX_PROVIDER").unwrap_or_else(|_| "codex".into());
    let signin_needed = std::env::var("HX_PROVIDER").as_deref() == Ok("fx")
        && matches!(provider.as_str(), "codex" | "grok")
        && crate::fx::doctor(&crate::fx::Config::from_env())
            .await
            .is_err();
    let signin_title = match provider.as_str() {
        "grok" => "Grok",
        _ => "ChatGPT",
    };

    // 4. Human-only steps, guided where they happen.
    let run = ulid::Ulid::generate().to_string();
    let mut steps: Vec<Value> = Vec::new();
    if let Some(app) = &app {
        let mut wanted = vec!["accessibility", "input", "screen"];
        if signin_needed {
            wanted.push("signin");
        }
        steps = guide(&out, app, &run, &wanted, signin_title, &options).await?;
    } else if signin_needed {
        let state = signin_terminal(&out, &provider, signin_title).await;
        steps.push(json!({"id":"signin","state":state}));
    }
    if !signin_needed && std::env::var("HX_PROVIDER").as_deref() == Ok("fx") {
        steps.push(json!({"id":"signin","state":"already_done"}));
    }

    // 5. The guide restarted the app after new grants; prove them with data.
    if app.is_some() {
        let granted: Vec<String> = steps
            .iter()
            .filter(|s| matches!(s["state"].as_str(), Some("done" | "already_done")))
            .filter_map(|s| s["id"].as_str().map(str::to_owned))
            .collect();
        let proof = prove_capture(store, &granted, &out).await;
        report.insert("proof".into(), proof);
    }

    // 6. Dashboard, already signed in.
    let link = login_link(store).await?;
    report.insert("dashboard".into(), json!({"state":"done","url":SERVER}));
    if options.open {
        open_url(&link);
    }

    report.insert("steps".into(), Value::Array(steps.clone()));
    report.insert("seconds".into(), json!(started.elapsed().as_secs()));
    let unfinished = steps
        .iter()
        .any(|s| !matches!(s["state"].as_str(), Some("done" | "already_done")));
    if options.json {
        println!("{}", json!({"event":"finished","report":report}));
    } else {
        print_summary(&steps, report.get("proof"), options.open);
    }
    Ok(if unfinished { 3 } else { 0 })
}

// MARK: service

fn healthy() -> bool {
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:7377".parse().expect("loopback"),
        Duration::from_secs(2),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut body = String::new();
    let _ = stream.read_to_string(&mut body);
    body.starts_with("HTTP/1.") && body.contains(" 200 ") && body.ends_with("ok\n")
}

fn start_service() {
    if cfg!(target_os = "macos") {
        let uid = unsafe_uid();
        let _ = Command::new("/bin/launchctl")
            .args(["kickstart", &format!("gui/{uid}/com.hyperthymesia.server")])
            .output();
    } else {
        let _ = Command::new("systemctl")
            .args(["--user", "start", "hyperthymesia.service"])
            .output();
    }
}

fn unsafe_uid() -> String {
    Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// The installer's `server.env` holds the provider settings the service uses.
/// Values already in the environment win.
pub fn server_env(state_dir: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(state_dir.join("server.env")) else {
        return Vec::new();
    };
    let mut vars = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.is_empty()
            || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || std::env::var_os(key).is_some()
        {
            continue;
        }
        // The installer writes 'single-quoted' values with '\'' escapes.
        let value = match value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Some(inner) => inner.replace("'\\''", "'"),
            None => value.to_owned(),
        };
        vars.push((key.to_owned(), value));
    }
    vars
}

// MARK: Mac app

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
fn support_dir() -> PathBuf {
    home().join("Library/Application Support/Hyperthymesia")
}
fn app_bundle() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let app = std::env::var_os("HX_APP")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("Applications/Hyperthymesia.app"));
    app.join("Contents/MacOS/hx-agent").is_file().then_some(app)
}

async fn pair_app(store: &Store, app: &Path) -> Result<&'static str> {
    let config: Value = std::fs::read(support_dir().join("agent.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let token = config["token"].as_str().unwrap_or_default();
    let same_server = config["server_url"]
        .as_str()
        .map(|u| u.trim_end_matches('/'))
        == Some(SERVER);
    if same_server && !token.is_empty() && store.authenticate(token).await?.is_some() {
        return Ok("already_done");
    }
    let name = Command::new("/usr/sbin/scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "this-mac".into());
    let code = store.create_pair_code(&name).await?;
    let mut link = url::Url::parse("hyperthymesia://pair")?;
    link.query_pairs_mut()
        .append_pair("server", SERVER)
        .append_pair("code", &code);
    let mut pair = Command::new(app.join("Contents/MacOS/hx-agent"));
    pair.arg("pair").arg(link.as_str());
    // A stale token from another server or a revoked one is replaced; a live
    // token for this server returned above.
    if !token.is_empty() {
        pair.arg("--replace");
    }
    let output = pair.output()?;
    if !output.status.success() {
        bail!(
            "could not pair the Mac app: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok("done")
}

fn ensure_app_running(app: &Path) {
    // Opening hands the app to its login item; an already running copy keeps going.
    let _ = Command::new("/usr/bin/open").arg("-g").arg(app).output();
}

fn write_private(path: &Path, value: &Value) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().context("setup folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(serde_json::to_vec_pretty(value)?.as_slice())?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn say_for(id: &str, state: &str, detail: &str, signin_title: &str) -> String {
    let setting = match id {
        "accessibility" => "Accessibility",
        "input" => "Input Monitoring",
        "screen" => "Screen Recording",
        _ => "",
    };
    match (id, state, detail) {
        (_, "waiting_user", "macos alert") => "Click Open System Settings when macOS asks.".into(),
        (_, "waiting_user", "password prompt cancelled") => {
            format!("Nothing changed. Switch on Hyperthymesia in {setting} to try again.")
        }
        (_, "waiting_system", "confirming") => "Waiting for the first screenshot.".into(),
        (_, "waiting_system", "macos asks") => "Click Allow when macOS asks.".into(),
        ("input", "skipped", "follows Accessibility") => {
            "Input Monitoring comes with Accessibility, which was not switched on.".into()
        }
        _ => match (id, state) {
            ("signin", "waiting_user") => format!("Sign in to {signin_title} in the browser."),
            ("signin", "done") => format!("Signed in to {signin_title}."),
            ("signin", "already_done") => format!("Already signed in to {signin_title}."),
            (_, "waiting_user") => format!("Switch on Hyperthymesia in {setting}."),
            (_, "waiting_system") => "Confirm with Touch ID or your password.".into(),
            (_, "done") => format!("{setting} is on."),
            (_, "already_done") => format!("{setting} was already on."),
            (_, "skipped") => format!(
                "Skipped {}.",
                if id == "signin" {
                    "signing in"
                } else {
                    setting
                }
            ),
            (_, "refused") => format!(
                "{} was not changed.",
                if id == "signin" { "Sign-in" } else { setting }
            ),
            (_, "failed") => format!(
                "{} did not finish.",
                if id == "signin" { "Sign-in" } else { setting }
            ),
            _ => String::new(),
        },
    }
}

async fn guide(
    out: &Out,
    app: &Path,
    run: &str,
    wanted: &[&str],
    signin_title: &str,
    options: &Options,
) -> Result<Vec<Value>> {
    let dir = support_dir().join("setup");
    std::fs::create_dir_all(&dir)?;
    let _ = std::fs::remove_file(dir.join("signin.json"));
    write_private(
        &dir.join("request.json"),
        &json!({"run":run,"steps":wanted,"signin_title":signin_title}),
    )?;
    ensure_app_running(app);

    let mut signin: Option<Signin> = None;
    let mut last: std::collections::HashMap<String, String> = Default::default();
    let started = Instant::now();
    let deadline = started + options.timeout;
    loop {
        let state: Option<Value> = std::fs::read(dir.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .filter(|s: &Value| s["run"] == run);
        if state.is_none() && started.elapsed() > Duration::from_secs(20) {
            bail!(
                "the Mac app did not start its setup guide; update the app by rerunning the installer"
            );
        }
        if let Some(state) = &state {
            for step in state["steps"].as_array().into_iter().flatten() {
                let id = step["id"].as_str().unwrap_or_default().to_owned();
                let now = step["state"].as_str().unwrap_or_default().to_owned();
                let detail = step["detail"].as_str().unwrap_or_default();
                let seen = format!("{now}/{detail}");
                if last.get(&id) != Some(&seen) {
                    if now != "pending" {
                        out.event(&id, &now, &say_for(&id, &now, detail, signin_title));
                    }
                    last.insert(id.clone(), seen);
                }
                // Start the provider sign-in when the guide reaches it.
                if id == "signin" {
                    if now == "waiting_user" && signin.is_none() {
                        signin = Some(Signin::start(&dir, run)?);
                    }
                    if matches!(now.as_str(), "skipped" | "refused" | "failed")
                        && let Some(s) = signin.as_mut()
                    {
                        s.stop();
                    }
                }
            }
            if state["status"] == "finished" {
                if let Some(s) = signin.as_mut() {
                    s.stop();
                }
                return Ok(state["steps"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| json!({"id":s["id"],"state":s["state"],"detail":s["detail"]}))
                    .collect());
            }
        }
        if let Some(s) = signin.as_mut() {
            s.poll(&dir, run).await?;
        }
        if Instant::now() > deadline {
            if let Some(s) = signin.as_mut() {
                s.stop();
            }
            out.event(
                "setup",
                "interrupted",
                "Setup paused. Run hx-server setup again to continue.",
            );
            let steps = state_steps(&dir, run);
            return Ok(steps);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

fn state_steps(dir: &Path, run: &str) -> Vec<Value> {
    std::fs::read(dir.join("state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|s| s["run"] == run)
        .and_then(|s| s["steps"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let state = match s["state"].as_str() {
                Some("pending" | "waiting_user" | "waiting_system") => "interrupted",
                Some(other) => other,
                None => "interrupted",
            };
            json!({"id":s["id"],"state":state,"detail":s["detail"]})
        })
        .collect()
}

// MARK: provider sign-in

/// `fx login` with the browser opening suppressed: the guide opens the page
/// beside its panel. The login returns to fx on localhost, so there is no code
/// to copy.
struct Signin {
    child: Option<std::process::Child>,
    url: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    written: bool,
    finished: bool,
}
impl Signin {
    fn start(dir: &Path, run: &str) -> Result<Self> {
        let config = crate::fx::Config::from_env();
        let provider = std::env::var("HX_FX_PROVIDER").unwrap_or_else(|_| "codex".into());
        let mut child = Command::new(&config.binary)
            .args(["login", &provider])
            .env("FX_NO_OPEN_BROWSER", "1")
            .env("FX_AUTO_UPGRADE", "0")
            .env("FX_DISABLE_KEYCHAIN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start the provider sign-in")?;
        let url = std::sync::Arc::new(std::sync::Mutex::new(None));
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
                    let line = line.trim();
                    if line.starts_with("https://") {
                        *url.lock().expect("url") = Some(line.to_owned());
                    }
                }
            });
        }
        write_private(
            &dir.join("signin.json"),
            &json!({"run":run,"state":"waiting"}),
        )?;
        Ok(Self {
            child: Some(child),
            url,
            written: false,
            finished: false,
        })
    }
    async fn poll(&mut self, dir: &Path, run: &str) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if !self.written
            && let Some(url) = self.url.lock().expect("url").clone()
        {
            write_private(
                &dir.join("signin.json"),
                &json!({"run":run,"state":"waiting","url":url}),
            )?;
            self.written = true;
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if let Some(status) = child.try_wait()? {
            self.finished = true;
            self.child = None;
            let ok = status.success()
                && crate::fx::doctor(&crate::fx::Config::from_env())
                    .await
                    .is_ok();
            let value = if ok {
                json!({"run":run,"state":"done"})
            } else {
                json!({"run":run,"state":"failed","error":"the sign-in did not complete"})
            };
            write_private(&dir.join("signin.json"), &value)?;
        }
        Ok(())
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.finished = true;
    }
}

/// Linux has no app to dock a guide in: open the page and wait in the terminal.
async fn signin_terminal(out: &Out, provider: &str, title: &str) -> &'static str {
    let dir = std::env::temp_dir().join(format!("hx-signin-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return "failed";
    }
    let Ok(mut signin) = Signin::start(&dir, "terminal") else {
        return "failed";
    };
    let _ = provider;
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut opened = false;
    while Instant::now() < deadline {
        let _ = signin.poll(&dir, "terminal").await;
        if !opened && let Some(url) = signin.url.lock().expect("url").clone() {
            opened = true;
            out.event(
                "signin",
                "waiting_user",
                &format!("Sign in to {title} in the browser."),
            );
            if !open_url(&url) {
                out.event(
                    "signin",
                    "waiting_user",
                    &format!("Open this page to sign in: {url}"),
                );
            }
        }
        if signin.finished {
            let done = std::fs::read(dir.join("signin.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .is_some_and(|v| v["state"] == "done");
            let _ = std::fs::remove_dir_all(&dir);
            let state = if done { "done" } else { "failed" };
            out.event("signin", state, &say_for("signin", state, "", title));
            return state;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    signin.stop();
    let _ = std::fs::remove_dir_all(&dir);
    "interrupted"
}

// MARK: proof

/// Grants are only claimed when the server holds data that needed them.
async fn prove_capture(store: &Store, granted: &[String], out: &Out) -> Value {
    let want = |id: &str| granted.iter().any(|g| g == id);
    let mut proof = json!({"accessibility":null,"input":null,"screen":null,"uploads":false});
    if granted.is_empty() {
        return proof;
    }
    out.event("proof", "waiting_system", "Checking that recording works…");
    let start = chrono::Utc::now() - chrono::Duration::seconds(30);
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let window = hx_schema::Interval {
            start,
            end: chrono::Utc::now() + chrono::Duration::seconds(5),
        };
        if let Ok(observations) = store.observations(window, false).await {
            for o in &observations {
                let e = serde_json::to_value(&o.envelope).unwrap_or(Value::Null);
                proof["uploads"] = json!(true);
                match e["kind"].as_str() {
                    Some("focus.span")
                        if e["payload"]["window_title"]
                            .as_str()
                            .is_some_and(|t| !t.is_empty()) =>
                    {
                        proof["accessibility"] = json!(true)
                    }
                    Some("input.bucket")
                        if ["keys", "clicks", "scroll_events", "mouse_px"]
                            .iter()
                            .any(|k| e["payload"][k].as_u64().is_some_and(|n| n > 0)) =>
                    {
                        proof["input"] = json!(true)
                    }
                    Some("media.still") => proof["screen"] = json!(true),
                    _ => {}
                }
            }
        }
        let done = ["accessibility", "input", "screen"]
            .iter()
            .all(|id| !want(id) || proof[*id] == json!(true));
        if done || Instant::now() > deadline {
            for id in ["accessibility", "input", "screen"] {
                if want(id) && proof[id] != json!(true) {
                    proof[id] = json!(false);
                }
            }
            return proof;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

// MARK: dashboard sign-in link

/// A one-use, five-minute link that signs the browser in, so nobody copies a
/// token. Earlier setup tokens are revoked, so reruns do not pile them up.
async fn login_link(store: &Store) -> Result<String> {
    for token in store.list_tokens().await? {
        if token.name == DASHBOARD_TOKEN && !token.revoked {
            store.revoke_token(&token.id).await?;
        }
    }
    let secret = store
        .create_token(
            DASHBOARD_TOKEN,
            ["source:", "macos-agent:", "macos-media:", "companion:"]
                .map(String::from)
                .to_vec(),
            vec!["read_raw".into(), "read_l3".into()],
        )
        .await?;
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| anyhow::anyhow!("random: {e}"))?;
    let code = hex::encode(raw);
    let expires = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + LOGIN_LINK_TTL_S;
    write_private(
        &login_link_path(&store.data_dir, &code),
        &json!({"token":secret,"expires":expires}),
    )?;
    Ok(format!("{SERVER}/login/link?code={code}"))
}

pub fn login_link_path(state_dir: &Path, code: &str) -> PathBuf {
    let digest = hex::encode(sha2::Sha256::digest(code.as_bytes()));
    state_dir.join("login-links").join(digest)
}

/// Claim a link exactly once. Returns the token secret when valid and unexpired.
pub fn claim_login_link(state_dir: &Path, code: &str) -> Option<String> {
    if code.len() != 64 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let path = login_link_path(state_dir, code);
    let claimed = path.with_extension(format!("claimed.{}", std::process::id()));
    std::fs::rename(&path, &claimed).ok()?;
    let value: Option<Value> = std::fs::read(&claimed)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let _ = std::fs::remove_file(&claimed);
    let value = value?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if value["expires"].as_u64()? < now {
        return None;
    }
    value["token"].as_str().map(str::to_owned)
}

fn open_url(url: &str) -> bool {
    let opener = if cfg!(target_os = "macos") {
        "/usr/bin/open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn print_summary(steps: &[Value], proof: Option<&Value>, opened: bool) {
    let name = |id: &str| {
        match id {
            "accessibility" => "Accessibility",
            "input" => "Input Monitoring",
            "screen" => "Screen Recording",
            "signin" => "Model sign-in",
            other => other,
        }
        .to_owned()
    };
    let group = |states: &[&str]| -> Vec<String> {
        steps
            .iter()
            .filter(|s| {
                // Input that only follows a refused Accessibility was not the person's skip.
                let state = match (s["state"].as_str(), s["detail"].as_str()) {
                    (Some("skipped"), Some("follows Accessibility")) => "refused",
                    (state, _) => state.unwrap_or_default(),
                };
                states.contains(&state)
            })
            .filter_map(|s| s["id"].as_str().map(name))
            .collect()
    };
    println!();
    for (label, states) in [
        ("Allowed now", &["done"][..]),
        ("Already set", &["already_done"][..]),
        ("Skipped", &["skipped"][..]),
        ("Not changed", &["refused"][..]),
        ("Did not finish", &["failed", "interrupted"][..]),
    ] {
        let names = group(states);
        if !names.is_empty() {
            println!("{label}: {}", names.join(", "));
        }
    }
    if let Some(proof) = proof {
        let missing: Vec<String> = ["accessibility", "input", "screen"]
            .iter()
            .filter(|id| proof[**id] == json!(false))
            .map(|id| name(id))
            .collect();
        let partial = steps.iter().any(|s| {
            matches!(s["id"].as_str(), Some("accessibility" | "input" | "screen"))
                && !matches!(s["state"].as_str(), Some("done" | "already_done"))
        });
        if proof["uploads"] == json!(true) && missing.is_empty() && partial {
            println!(
                "Recording verified for what is allowed: the server is receiving this Mac's activity."
            );
        } else if proof["uploads"] == json!(true) && missing.is_empty() {
            println!("Recording verified: the server is receiving this Mac's activity.");
        } else if !missing.is_empty() {
            println!("Not yet seen on the server: {}.", missing.join(", "));
        }
    }
    if opened {
        println!("The dashboard is open and signed in. Run hx-server setup again any time.");
    } else {
        println!("Run hx-server setup again any time.");
    }
}
