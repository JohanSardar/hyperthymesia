//! Scale fixtures are generated in temporary directories and contain no local history.
use hx_connector_sdk::{
    self as sdk, Cli, Command, Request, Scan,
    serde_json::{Value, json},
};
use std::{
    io::{BufRead, Read, Write},
    net::TcpListener,
    path::Path,
    time::Instant,
};

pub fn catch_up(
    source: &Path,
    scan: impl Fn(&Request, &Value) -> anyhow::Result<Scan>,
) -> (usize, usize) {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = done.clone();
    let handle = std::thread::spawn(move || {
        let mut keys = std::collections::BTreeSet::new();
        let mut batches = 0;
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "catch-up timed out");
            let (mut socket, _) = match listener.accept() {
                Ok(socket) => socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("content-length:") {
                    len = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                }
            }
            let mut bytes = vec![0; len];
            reader.read_exact(&mut bytes).unwrap();
            let batch: sdk::hx_schema::UploadBatch =
                serde_json::from_slice(&sdk::zstd::decode_all(bytes.as_slice()).unwrap()).unwrap();
            batches += 1;
            for event in &batch.envelopes {
                let n: usize = event.payload["native"]["n"].as_u64().unwrap() as usize;
                assert!((49_000..50_000).contains(&n));
                assert!(keys.insert(n), "duplicate uploaded record");
                assert!(
                    event.time.source_start.unwrap()
                        >= "2000-01-02T00:00:00Z"
                            .parse::<sdk::chrono::DateTime<sdk::chrono::Utc>>()
                            .unwrap()
                );
            }
            let ack = json!({"batch_id":batch.batch_id,"accepted":batch.envelopes.len(),"duplicates":0,"rejected":[]}).to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",ack.len(),ack).unwrap();
        }
        assert_eq!(keys.len(), 1000);
        batches
    });
    let started = Instant::now();
    let scans = std::cell::Cell::new(0);
    let completed = std::cell::Cell::new(false);
    sdk::run_cli(
        Cli {
            command: Command::Sync {
                dry_run: false,
                limit: 1000,
                since: Some("2000-01-02T00:00:00Z".parse().unwrap()),
            },
            server: Some(server),
            token: Some("synthetic".into()),
            instance: Some("scale".into()),
            source: Some(source.to_string_lossy().into()),
            state_dir: Some(dir.path().into()),
            root: vec![],
            depth: 3,
            apple_token: None,
        },
        "scale",
        "",
        &[],
        |r, s| {
            scans.set(scans.get() + 1);
            let result = scan(r, s)?;
            completed.set(result.complete);
            if result.complete {
                assert!(result.envelopes.is_empty());
            }
            Ok(result)
        },
    )
    .unwrap();
    // The state only contains synthetic records and paths under a temporary directory.
    let bytes = std::fs::read(dir.path().join("scale.json")).unwrap();
    let state: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(state["pending"].is_null());
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let batches = handle.join().unwrap();
    assert!(completed.get());
    let scans = scans.get();
    assert_eq!(scans, batches);
    eprintln!(
        "catch-up: {scans} batches, {:?}, {} state bytes",
        started.elapsed(),
        bytes.len()
    );
    assert_eq!(batches, 2);
    assert!(started.elapsed().as_secs() < 30);
    (batches, bytes.len())
}
