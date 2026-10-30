//! Shared CLI, durable delivery, read-only sources and resumable JSONL tails.
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use hx_schema::{
    Completeness, Coverage, Envelope, Kind, ObservationId, ObservationTime, Operation, Sensitivity,
    UploadAck, UploadBatch,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub use chrono;
pub use hx_schema;
pub use reqwest;
pub use rusqlite;
pub use serde_json;
pub const TEXT_CAP: usize = 32_768;

#[derive(Parser, Debug)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[arg(long, env = "HX_SERVER", global = true)]
    pub server: Option<String>,
    #[arg(long, env = "HX_TOKEN", global = true, hide_env_values = true)]
    pub token: Option<String>,
    #[arg(long, env = "HX_INSTANCE", global = true)]
    pub instance: Option<String>,
    /// Override the source database, directory, or Apple Bridge base URL.
    #[arg(long, env = "HX_SOURCE", global = true)]
    pub source: Option<String>,
    #[arg(long, global = true)]
    pub state_dir: Option<PathBuf>,
    /// Git discovery roots; repeat for multiple roots.
    #[arg(long, global = true)]
    pub root: Vec<PathBuf>,
    #[arg(long, default_value_t = 3, global = true)]
    pub depth: usize,
    #[arg(long, env = "HX_APPLE_TOKEN", global = true, hide_env_values = true)]
    pub apple_token: Option<String>,
}
#[derive(Subcommand, Debug)]
pub enum Command {
    Describe,
    Sync {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value_t = 1000)]
        limit: usize,
        #[arg(long)]
        since: Option<DateTime<Utc>>,
    },
}
#[derive(Clone, Debug)]
pub struct Request {
    pub instance: String,
    pub source: String,
    pub limit: usize,
    pub since: Option<DateTime<Utc>>,
    pub roots: Vec<PathBuf>,
    pub depth: usize,
    pub apple_token: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scan {
    pub envelopes: Vec<Envelope>,
    pub next: Value,
    pub complete: bool,
    pub coverage: Completeness,
    pub reason: Option<String>,
}
impl Scan {
    pub fn new(next: Value) -> Self {
        Self {
            envelopes: vec![],
            next,
            complete: true,
            coverage: Completeness::Partial,
            reason: None,
        }
    }
    pub fn unknown(next: Value, reason: &str) -> Self {
        Self {
            complete: false,
            coverage: Completeness::Unknown,
            reason: Some(reason.into()),
            ..Self::new(next)
        }
    }
    pub fn push(&mut self, e: Envelope) {
        if !self
            .envelopes
            .iter()
            .any(|old| old.dedup_key() == e.dedup_key())
        {
            self.envelopes.push(e);
        }
    }
}
#[derive(Serialize, Deserialize, Default)]
struct State {
    #[serde(default)]
    cursor: Value,
    #[serde(default)]
    pending: Option<Pending>,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    batch: UploadBatch,
    next: Value,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME must be set"))
}
pub fn run(
    name: &str,
    default_source: &str,
    kinds: &[&str],
    scan: impl Fn(&Request, &Value) -> Result<Scan>,
) -> Result<()> {
    run_cli(Cli::parse(), name, default_source, kinds, scan)
}
pub fn run_cli(
    cli: Cli,
    name: &str,
    default_source: &str,
    kinds: &[&str],
    scan: impl Fn(&Request, &Value) -> Result<Scan>,
) -> Result<()> {
    let source = cli
        .source
        .unwrap_or_else(|| default_source.replace('~', &home().to_string_lossy()));
    let instance = cli
        .instance
        .unwrap_or_else(|| format!("{name}:{}", &hx_schema::sha256_hex(source.as_bytes())[..16]));
    ensure!(
        instance
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)),
        "invalid instance name"
    );
    if matches!(cli.command, Command::Describe) {
        println!(
            "{}",
            serde_json::to_string(&hx_schema::protocol::Capabilities {
                connector_instance: hx_schema::ConnectorInstance::new(instance),
                connector: name.into(),
                version: env!("CARGO_PKG_VERSION").into(),
                kinds: kinds.iter().map(|s| (*s).into()).collect(),
                supports_backfill: true,
                supports_changes: true,
                mutable_source: name != "git",
            })?
        );
        return Ok(());
    }
    let Command::Sync {
        dry_run,
        limit,
        since,
    } = cli.command
    else {
        unreachable!()
    };
    ensure!(limit > 0, "limit must be positive");
    let req = Request {
        instance: instance.clone(),
        source,
        limit,
        since,
        roots: cli.root,
        depth: cli.depth,
        apple_token: cli.apple_token,
    };
    let dir = cli
        .state_dir
        .unwrap_or_else(|| home().join("Library/Application Support/Hyperthymesia/connectors"));
    let path = dir.join(format!("{instance}.json"));
    // Dry runs deliberately backfill independently of any saved cursor or pending delivery.
    if dry_run {
        let result = scan(&req, &Value::Null)?;
        for e in &result.envelopes {
            e.validate()?;
            println!("{}", serde_json::to_string(e)?);
        }
        report(&result);
        return Ok(());
    }
    let server = cli.server.context("HX_SERVER or --server required")?;
    let token = cli.token.context("HX_TOKEN or --token required")?;
    fs::create_dir_all(&dir)?;
    let lock = private_open(&path.with_extension("lock"))?;
    lock.try_lock_exclusive()
        .context("connector instance already syncing")?;
    let mut state: State = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e.into()),
    };
    if state.pending.is_some() {
        deliver(&path, &mut state, &server, &token)?;
    }
    let result = scan(&req, &state.cursor)?;
    for e in &result.envelopes {
        e.validate()?;
    }
    state.pending = Some(Pending {
        batch: UploadBatch {
            batch_id: ulid::Ulid::generate(),
            envelopes: result.envelopes.clone(),
        },
        next: result.next.clone(),
    });
    atomic_json(&path, &state)?;
    deliver(&path, &mut state, &server, &token)?;
    report(&result);
    Ok(())
}
fn report(s: &Scan) {
    eprintln!(
        "{}",
        json!({"complete":s.complete,"coverage":s.coverage,"reason":s.reason,"count":s.envelopes.len()})
    );
}
fn private_open(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)?)
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("state has no parent")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut tmp, value)?;
    tmp.flush()?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn deliver(path: &Path, state: &mut State, server: &str, token: &str) -> Result<()> {
    let pending = state.pending.as_ref().context("missing pending batch")?;
    upload(server, token, &pending.batch)?;
    state.cursor = pending.next.clone();
    state.pending = None;
    atomic_json(path, state)
}
pub fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}
pub fn upload(server: &str, token: &str, batch: &UploadBatch) -> Result<()> {
    let body = zstd::encode_all(serde_json::to_vec(batch)?.as_slice(), 3)?;
    let client = client()?;
    for attempt in 0..3 {
        let result = (|| -> Result<()> {
            let response = client
                .post(format!("{}/v1/events", server.trim_end_matches('/')))
                .bearer_auth(token)
                .header("Content-Type", "application/json")
                .header("Content-Encoding", "zstd")
                .body(body.clone())
                .send()?;
            ensure!(
                response.status().is_success(),
                "upload HTTP {}",
                response.status()
            );
            let ack: UploadAck = response.json()?;
            ensure!(
                ack.batch_id == batch.batch_id
                    && ack.rejected.is_empty()
                    && u64::from(ack.accepted) + u64::from(ack.duplicates)
                        == batch.envelopes.len() as u64,
                "incomplete or mismatched upload acknowledgement"
            );
            Ok(())
        })();
        if result.is_ok() || attempt == 2 {
            return result;
        }
        std::thread::sleep(Duration::from_millis(100 * (1 << attempt)));
    }
    unreachable!()
}
pub fn readonly(path: &Path) -> Result<rusqlite::Connection> {
    // URI mode=ro retains SQLite's WAL visibility; immutable=1 would hide WAL changes.
    let url =
        reqwest::Url::from_file_path(path).map_err(|_| anyhow::anyhow!("invalid SQLite path"))?;
    let conn = rusqlite::Connection::open_with_flags(
        format!("{url}?mode=ro"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_millis(500))?;
    Ok(conn)
}
pub fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            v.as_i64().and_then(|n| {
                if n.abs() > 10_000_000_000 {
                    DateTime::from_timestamp_millis(n)
                } else {
                    DateTime::from_timestamp(n, 0)
                }
            })
        })
}
pub fn envelope(
    req: &Request,
    key: &str,
    revision: &str,
    kind: &str,
    payload: Value,
    at: Option<DateTime<Utc>>,
) -> Envelope {
    let mut time = ObservationTime::point(Utc::now());
    time.source_start = at;
    Envelope {
        envelope_version: 1,
        observation_id: ObservationId::generate(),
        connector_instance: hx_schema::ConnectorInstance::new(&req.instance),
        source_key: hx_schema::SourceKey::new(key),
        revision: hx_schema::Revision::new(revision),
        operation: Operation::Upsert,
        sequence: None,
        kind: Kind::new(kind),
        schema_version: 1,
        time,
        device: None,
        actor: None,
        subject: None,
        resource: None,
        links: vec![],
        coverage: Some(Coverage {
            scope: "source_record".into(),
            completeness: Completeness::Partial,
        }),
        sensitivity: Sensitivity::Sensitive,
        payload,
        blobs: vec![],
    }
}
pub fn agent(
    req: &Request,
    key: &str,
    revision: &str,
    mut payload: Value,
    at: Option<DateTime<Utc>>,
) -> Envelope {
    let text = payload["text"].as_str().unwrap_or_default();
    if text.len() > TEXT_CAP {
        let mut end = TEXT_CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let capped = text[..end].to_owned();
        payload["text"] = json!(capped);
        payload["text_truncated"] = json!(true);
    }
    envelope(req, key, revision, "agent.message", payload, at)
}
pub fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().into()
}
pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => {
            for key in ["text", "content", "output", "input", "arguments"] {
                if let Some(v) = o.get(key) {
                    return if v.is_object() {
                        v.to_string()
                    } else {
                        text(v)
                    };
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}
pub fn digest(v: &Value) -> String {
    hx_schema::sha256_hex(v.to_string().as_bytes())
}
pub fn link(e: &mut Envelope, rel: &str, target: &str) {
    if !target.is_empty() {
        e.links.push(hx_schema::Link {
            rel: rel.into(),
            target: target.into(),
        });
    }
}

/// Sorted traversal; symlinks are never followed. Unreadable entries fail the scan visibly.
pub fn files(root: &Path, depth: usize, accept: &impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    if root.is_file() {
        if accept(root) {
            out.push(root.into());
        }
        return Ok(out);
    }
    if !root.exists() {
        return Ok(out);
    }
    let mut entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let ty = entry.file_type()?;
        let path = entry.path();
        if ty.is_file() && accept(&path) {
            out.push(path);
        } else if ty.is_dir() && depth > 0 {
            out.extend(files(&path, depth - 1, accept)?);
        }
    }
    Ok(out)
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TailCursor {
    pub generation: String,
    pub offset: u64,
    #[serde(default)]
    pub anchor: String,
    #[serde(default)]
    pub context: Value,
}
pub struct TailLine {
    pub value: Value,
    pub offset: u64,
    pub end: u64,
}
/// Offset is in decompressed bytes for zstd, raw bytes otherwise. Partial last lines stay unread.
pub fn tail(
    path: &Path,
    saved: &TailCursor,
    budget: usize,
) -> Result<(Vec<TailLine>, TailCursor, bool)> {
    use std::os::unix::fs::MetadataExt;
    let file = File::open(path)?;
    let meta = file.metadata()?;
    let generation = format!("{}:{}", meta.dev(), meta.ino());
    let compressed = path.extension().is_some_and(|e| e == "zst");
    let mut reader: Box<dyn Read> = if compressed {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut prefix = vec![];
    reader.by_ref().take(saved.offset.min(64)).read_to_end(&mut prefix)?;
    let got = prefix.len();
    let valid = saved.generation == generation
        && (saved.offset == 0
            || (got as u64 == saved.offset.min(64)
                && hx_schema::sha256_hex(&prefix) == saved.anchor))
        && (compressed || meta.len() >= saved.offset);
    let mut cursor = if valid {
        saved.clone()
    } else {
        TailCursor {
            generation,
            ..TailCursor::default()
        }
    };
    // Reopen to avoid consuming a prefix from a newly rotated file.
    let file = File::open(path)?;
    let reader: Box<dyn Read> = if compressed {
        let mut r = zstd::stream::read::Decoder::new(file)?;
        std::io::copy(&mut r.by_ref().take(cursor.offset), &mut std::io::sink())?;
        Box::new(r)
    } else {
        let mut r = file;
        r.seek(std::io::SeekFrom::Start(cursor.offset))?;
        Box::new(r)
    };
    let mut reader = BufReader::new(reader);
    let mut lines = vec![];
    let mut complete = true;
    while lines.len() < budget {
        let mut bytes = vec![];
        let n = reader.read_until(b'\n', &mut bytes)?;
        if n == 0 {
            break;
        }
        if bytes.last() != Some(&b'\n') {
            complete = false;
            break;
        }
        let offset = cursor.offset;
        cursor.offset += n as u64;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value = serde_json::from_slice(&bytes)
            .context("invalid complete JSONL record; checkpoint retained")?;
        lines.push(TailLine {
            value,
            offset,
            end: cursor.offset,
        });
    }
    if lines.len() == budget {
        complete = false;
    }
    let file = File::open(path)?;
    let mut reader: Box<dyn Read> = if compressed {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut anchor = vec![];
    reader
        .by_ref()
        .take(cursor.offset.min(64))
        .read_to_end(&mut anchor)?;
    cursor.anchor = hx_schema::sha256_hex(&anchor);
    Ok((lines, cursor, complete))
}
pub fn tail_state(state: &Value) -> Result<BTreeMap<String, TailCursor>> {
    if state.is_null() {
        Ok(BTreeMap::new())
    } else {
        Ok(serde_json::from_value(state.clone())?)
    }
}
pub fn after_since(req: &Request, e: &Envelope) -> bool {
    req.since
        .is_none_or(|s| e.time.source_start.is_none_or(|t| t >= s))
}
