//! The product goal as one test: someone connects a source, does nothing else, and a
//! later day-page entry cites that source's data. Everything runs through the real
//! `hx-server` binary and its scheduler, with a scripted stand-in for fx.
//!
//! Connecting does not exist yet, so the test proposes the smallest contract for it.
//! Issue #34 may change this contract; change the test with it:
//!
//!     POST /v1/sources
//!     Authorization: Bearer <owner token>
//!     {"name": "...", "key": "...", "description": "what it is, in the person's words"}
//!     -> 201
//!
//! Data pulled from a connected source lands under a connector instance beginning
//! `source:`, which is what the owner token below is scoped to read.
//!
//! When issue #36 adds the agent's setup run, the stand-in fx must also answer that run
//! with the recipe that pulls from the fake source.
#![cfg(unix)]

use axum::{Json, Router, http::HeaderMap, http::StatusCode, routing::get};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
};

const SOURCE_KEY: &str = "synthetic-source-key";
const SUBJECT: &str = "Synthetic invoice for the orchid greenhouse";

#[tokio::test]
#[ignore = "the product goal: connecting a source does not exist until issue #37"]
async fn connecting_a_source_is_enough_for_a_cited_day_page_entry() {
    scenario().await;
}

async fn scenario() {
    let scratch = tempfile::tempdir().unwrap();
    let cli = || hx_server_command(scratch.path());
    assert!(cli().arg("migrate").status().await.unwrap().success());
    let token = cli()
        .args(["token", "create", "--name", "owner", "--prefix", "source:"])
        .args(["--scopes", "ingest,read_l3,read_raw"])
        .output()
        .await
        .unwrap();
    assert!(token.status.success());
    let owner = String::from_utf8(token.stdout).unwrap().trim().to_owned();

    // Far enough back that the email's four-hour window has already ended.
    let sent_at = Utc::now() - Duration::hours(8);
    let (source_url, source_reads) = start_fake_source(sent_at).await;
    let (server, _running) = start_server(&cli, scratch.path()).await;

    // The only thing the person does.
    let connect = http(
        &server,
        "POST",
        "/v1/sources",
        &owner,
        Some(json!({
            "name": "Synthetic Mail",
            "key": SOURCE_KEY,
            "description": format!(
                "My email inbox. GET {source_url}/messages with the key as a bearer token."
            ),
        })),
    )
    .await;
    assert_eq!(
        connect.0, 201,
        "connect step: POST /v1/sources did not accept the source: {}",
        connect.1
    );

    let day = sent_at.date_naive();
    let evidence = wait_for_cited_entry(&server, &owner, &day.to_string()).await;
    assert!(
        source_reads.load(Ordering::SeqCst) > 0,
        "the entry must come from reading the connected source"
    );
    let (status, citation) = http(
        &server,
        "GET",
        &format!("/evidence/{evidence}"),
        &owner,
        None,
    )
    .await;
    assert_eq!(status, 200, "the citation must open: {citation}");
    assert!(
        citation.contains(SUBJECT),
        "the citation must open the email: {citation}"
    );
}

/// The day page's cited entry, waiting for the scheduler to get there on its own.
async fn wait_for_cited_entry(server: &str, owner: &str, day: &str) -> i64 {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        let (status, page) = http(server, "GET", &format!("/day/{day}"), owner, None).await;
        assert_eq!(status, 200, "day page: {page}");
        if let Some(evidence) = cited_entry(&page) {
            return evidence;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no cited entry on the day page in time: {page}"
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

fn cited_entry(page: &str) -> Option<i64> {
    let (_, episodes) = page.split_once(r#"id="episodes""#)?;
    let (episodes, _) = episodes.split_once(r#"id="tasks""#)?;
    let (_, link) = episodes.split_once(r#"href="/evidence/"#)?;
    link.split_once('"')?.0.parse().ok()
}

fn hx_server_command(cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hx-server"));
    // Nothing from the developer's environment or real home reaches the server.
    command
        .env_clear()
        .env("HOME", cwd)
        .env("PATH", "/usr/bin:/bin")
        .env("HX_STATE_DIR", cwd.join("state"))
        .current_dir(cwd)
        .kill_on_drop(true);
    command
}

async fn start_server(cli: &impl Fn() -> Command, scratch: &Path) -> (String, Child) {
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = probe.local_addr().unwrap().to_string();
    drop(probe);
    let credential = scratch.join("synthetic-auth.json");
    std::fs::write(&credential, "{}").unwrap();
    let running = cli()
        .args(["serve", "--listen", &address])
        .env("HX_PROVIDER", "fx")
        .env("HX_FX_BINARY", write_scripted_fx(scratch))
        .env("HX_FX_CREDENTIAL", &credential)
        .env("HX_INTERPRET_SETTLE_S", "0")
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..150 {
        if TcpStream::connect(&address).await.is_ok() {
            return (address, running);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("hx-server did not start listening");
}

/// Stands in for the model: cites the email when the window holds it, else reports nothing.
fn write_scripted_fx(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = r#"#!/usr/bin/python3
import json,os,pathlib,re,sys,urllib.request
if '--version' in sys.argv:
 print('FX_VERSION');sys.exit(0)
start,end=re.search(r'window (\S+) to (\S+) \(UTC\)',sys.stdin.read()).groups()
server=json.loads((pathlib.Path(os.environ['HOME'])/'.fx/mcp.json').read_text())['mcpServers']['hyperthymesia']
def rpc(method,params):
 req=urllib.request.Request(server['url'],data=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+os.environ['HX_MCP_TOKEN']})
 return json.load(urllib.request.urlopen(req))['result']
rpc('initialize',{})
overview=json.loads(rpc('tools/call',{'name':'window_overview','arguments':{'from':start,'to':end}})['content'][0]['text'])
answer={'episodes':[],'tasks':[],'summary':''}
for e in overview['evidence']:
 if 'SUBJECT' in e['text']:
  answer['episodes'].append({'id':'synthetic-email','intervals':[{'start':e['at'],'end':e['at']}],'title':'Received the synthetic invoice email','description':'','claims':[{'actor':'owner','action':'received','object':'synthetic invoice email','outcome':'unknown','basis':'observed','evidence':[e['id']],'outcome_evidence':[]}],'evidence':[e['id']],'confidence':0.8})
print(json.dumps({'exit_code':0,'model':'FX_MODEL','steps':2,'final_output':json.dumps(answer),'tool_calls':[{'name':'mcp_hyperthymesia_window_overview','status':'success'}]}))
"#;
    let binary = dir.join("scripted-fx");
    std::fs::write(
        &binary,
        script
            .replace("FX_VERSION", hx_server::fx::FX_VERSION)
            .replace("FX_MODEL", hx_server::fx::MODEL)
            .replace("SUBJECT", SUBJECT),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    binary
}

/// A tiny mail API holding one made-up email, readable only with the source key.
async fn start_fake_source(sent_at: DateTime<Utc>) -> (String, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = reads.clone();
    let app = Router::new().route(
        "/messages",
        get(move |headers: HeaderMap| async move {
            let expected = format!("Bearer {SOURCE_KEY}");
            if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&expected) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Json::<Value>(json!({"messages":[{
                "id": "synthetic-message-1",
                "from": "dana@example.test",
                "to": "owner@example.test",
                "subject": SUBJECT,
                "body": "Made-up test mail. The synthetic greenhouse invoice is attached.",
                "sent_at": sent_at,
            }]})))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, reads)
}

/// One plain HTTP/1.1 exchange, enough for this test without another dependency.
async fn http(
    server: &str,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (u16, String) {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut stream = TcpStream::connect(server).await.unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {server}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok());
    (status.expect("HTTP status"), body.to_owned())
}
