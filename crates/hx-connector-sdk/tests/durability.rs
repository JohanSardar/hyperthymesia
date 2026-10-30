use hx_connector_sdk::{serde_json::json, *};
use std::{
    io::{BufRead, Read, Write},
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
};

fn req() -> Request {
    Request {
        instance: "test".into(),
        source: "synthetic".into(),
        limit: 20,
        since: None,
        roots: vec![],
        depth: 3,
        apple_token: None,
    }
}
fn cli(dir: &Path, server: String) -> Cli {
    Cli {
        command: Command::Sync {
            dry_run: false,
            limit: 20,
            since: None,
        },
        server: Some(server),
        token: Some("synthetic".into()),
        instance: Some("test".into()),
        source: Some("synthetic".into()),
        state_dir: Some(dir.into()),
        root: vec![],
        depth: 3,
        apple_token: None,
    }
}
fn server(
    status: u16,
    count: usize,
) -> (
    String,
    Arc<Mutex<Vec<hx_schema::UploadBatch>>>,
    std::thread::JoinHandle<()>,
) {
    server_with_ack(status, count, true)
}
fn server_with_ack(
    status: u16,
    count: usize,
    matching: bool,
) -> (
    String,
    Arc<Mutex<Vec<hx_schema::UploadBatch>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let batches = Arc::new(Mutex::new(vec![]));
    let captured = batches.clone();
    let handle = std::thread::spawn(move || {
        for _ in 0..count {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
            let mut headers = String::new();
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if line.to_lowercase().starts_with("content-length:") {
                    len = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                }
                headers.push_str(&line);
            }
            assert!(headers.starts_with("POST /v1/events "));
            assert!(headers.to_lowercase().contains("content-encoding: zstd"));
            assert!(
                headers
                    .to_lowercase()
                    .contains("authorization: bearer synthetic")
            );
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let batch: hx_schema::UploadBatch =
                serde_json::from_slice(&zstd::decode_all(body.as_slice()).unwrap()).unwrap();
            let body = if status == 200 {
                serde_json::to_string(&hx_schema::UploadAck {
                    batch_id: batch.batch_id,
                    accepted: batch.envelopes.len() as u32,
                    duplicates: 0,
                    rejected: vec![],
                })
                .unwrap()
            } else {
                "{}".into()
            };
            let body = if matching || status != 200 {
                body
            } else {
                let mut ack: serde_json::Value = serde_json::from_str(&body).unwrap();
                ack["batch_id"] = json!("00000000000000000000000000");
                ack.to_string()
            };
            captured.lock().unwrap().push(batch);
            write!(socket,"HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    (addr, batches, handle)
}
#[test]
fn failed_upload_retains_cursor_and_identical_pending_batch_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, batches, handle) = server(503, 3);
    let scan = |_: &Request, _: &serde_json::Value| {
        let mut s = Scan::new(json!({"position":1}));
        s.push(envelope(
            &req(),
            "h_1",
            "1",
            "agent.message",
            json!({"text":"xxxxxxxx"}),
            None,
        ));
        Ok(s)
    };
    assert!(run_cli(cli(dir.path(), addr), "test", "", &[], scan).is_err());
    handle.join().unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("test.json")).unwrap()).unwrap();
    assert!(saved["cursor"].is_null());
    assert_eq!(saved["pending"]["next"]["position"], 1);
    let captured = batches.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert!(captured.iter().all(|b| b == &captured[0]));
    let old = captured[0].clone();
    drop(captured);
    let (addr, batches, handle) = server(200, 2);
    run_cli(cli(dir.path(), addr), "test", "", &[], |_, state| {
        assert_eq!(state["position"], 1);
        Ok(Scan::new(json!({"position":2})))
    })
    .unwrap();
    handle.join().unwrap();
    assert_eq!(batches.lock().unwrap()[0], old);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("test.json")).unwrap()).unwrap();
    assert_eq!(saved["cursor"]["position"], 2);
    assert!(saved["pending"].is_null());
}
#[test]
fn partial_last_line_resumes_once_and_rotation_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.jsonl");
    std::fs::write(&path, b"{\"n\":1}\n{\"n\":").unwrap();
    let (lines, cursor, complete) = tail(&path, &TailCursor::default(), 20).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(!complete);
    assert_eq!(cursor.offset, 8);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"2}\n")
        .unwrap();
    let (lines, cursor, _) = tail(&path, &cursor, 20).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].value["n"], 2);
    assert!(tail(&path, &cursor, 20).unwrap().0.is_empty());
    std::fs::rename(&path, dir.path().join("old")).unwrap();
    std::fs::write(&path, b"{\"n\":3}\n").unwrap();
    assert_eq!(tail(&path, &cursor, 20).unwrap().0[0].value["n"], 3);
}
#[test]
fn compressed_tail_uses_decoded_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.jsonl.zst");
    std::fs::write(
        &path,
        zstd::encode_all(b"{\"n\":1}\n{\"n\":2}\n".as_slice(), 1).unwrap(),
    )
    .unwrap();
    let (lines, cursor, _) = tail(&path, &TailCursor::default(), 1).unwrap();
    assert_eq!(lines.len(), 1);
    let (lines, _, _) = tail(&path, &cursor, 20).unwrap();
    assert_eq!(lines[0].value["n"], 2);
}
#[test]
fn dry_run_leaves_no_state_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent");
    let mut cli = cli(&path, String::new());
    cli.command = Command::Sync {
        dry_run: true,
        limit: 1,
        since: None,
    };
    run_cli(cli, "test", "", &[], |_, _| Ok(Scan::new(json!({})))).unwrap();
    assert!(!path.exists());
}
#[test]
fn tool_text_is_bounded_and_sensitive() {
    let e = agent(
        &req(),
        "h_1",
        "1",
        json!({"text":"é".repeat(TEXT_CAP),"role":"tool"}),
        None,
    );
    assert!(e.payload["text"].as_str().unwrap().len() <= TEXT_CAP);
    assert_eq!(e.payload["text_truncated"], true);
    assert_eq!(e.sensitivity, hx_schema::Sensitivity::Sensitive);
}

#[test]
fn readonly_sqlite_sees_wal_and_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite");
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE evidence(n INTEGER); INSERT INTO evidence VALUES(7);").unwrap();
    let reader = readonly(&path).unwrap();
    assert_eq!(
        reader
            .query_row("SELECT n FROM evidence", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert!(
        reader
            .execute("INSERT INTO evidence VALUES(8)", [])
            .is_err()
    );
}

#[test]
fn sync_checkpoints_each_batch_before_scanning_the_next_and_stops_without_progress() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, batches, handle) = server(200, 4);
    run_cli(cli(dir.path(), addr), "test", "", &[], |_, cursor| {
        let n = cursor["n"].as_u64().unwrap_or(0);
        if n > 0 {
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("test.json")).unwrap())
                    .unwrap();
            assert_eq!(&state["cursor"], cursor);
            assert!(state["pending"].is_null());
        }
        let mut scan = Scan::new(json!({"n": (n+1).min(3)}));
        scan.complete = false;
        Ok(scan)
    })
    .unwrap();
    handle.join().unwrap();
    assert_eq!(batches.lock().unwrap().len(), 4);
}

#[test]
fn compact_tail_state_reads_legacy_and_preserves_offsets_and_context() {
    let state = json!({"synthetic": {"generation":"1:2", "offset":42,"anchor":"h_anchor", "context":{"session":"h_session","turn":"h_turn"}}});
    let legacy = tail_state(&state).unwrap();
    let encoded = compact_tail_state(&legacy).unwrap();
    assert_eq!(
        serde_json::to_value(tail_state(&encoded).unwrap()).unwrap(),
        state
    );
    assert!(tail_state(&json!({"zstd_tail_v1":"0"})).is_err());
    assert!(tail_state(&json!({"zstd_tail_v1":"zz"})).is_err());
}

#[test]
fn mismatched_ack_never_commits_or_scans_the_next_batch() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _, handle) = server_with_ack(200, 3, false);
    let scans = std::cell::Cell::new(0);
    assert!(
        run_cli(cli(dir.path(), addr), "test", "", &[], |_, cursor| {
            assert!(cursor.is_null());
            scans.set(scans.get() + 1);
            let mut scan = Scan::new(json!({"n":1}));
            scan.complete = false;
            Ok(scan)
        })
        .is_err()
    );
    handle.join().unwrap();
    assert_eq!(scans.get(), 1);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("test.json")).unwrap()).unwrap();
    assert!(saved["cursor"].is_null());
    assert_eq!(saved["pending"]["next"]["n"], 1);
}

#[test]
fn filtered_tail_caps_raw_work_and_can_resume_without_losing_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.jsonl");
    std::fs::write(&path, "{}\n".repeat(100_001)).unwrap();
    let (cursor, complete) = tail_each(&path, &TailCursor::default(), 1, |_, ctx, _| {
        *ctx = json!({"synthetic":"h_context"});
        Ok(false)
    })
    .unwrap();
    assert!(!complete);
    assert_eq!(cursor.offset, 300_000);
    let (end, complete) = tail_each(&path, &cursor, 1, |_, ctx, _| {
        assert_eq!(ctx["synthetic"], "h_context");
        Ok(false)
    })
    .unwrap();
    assert!(complete);
    assert_eq!(end.offset, 300_003);
}
