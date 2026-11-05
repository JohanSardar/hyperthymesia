#![cfg(unix)]

use axum::{body::Body, http::Request};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use hx_schema::{Envelope, Interval};
use hx_server::{disclosure::DisclosurePolicy, processing};
use hx_store::{Store, Token, UploadBatch, test_support::Fixture};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio};
use tokio::process::Command;
use tower::ServiceExt;

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

async fn seed(store: &Store, old: bool) -> Interval {
    let w = hx_store::window_at(Utc::now() - Duration::hours(if old { 240 } else { 8 }));
    let e: Envelope = serde_json::from_value(json!({
        "envelope_version":1,"observation_id":ulid::Ulid::generate(),
        "connector_instance":"test:service","source_key":"synthetic","revision":"1",
        "operation":"observe","kind":"web.visit","schema_version":1,
        "time":hx_schema::ObservationTime::point(w.start),
        "sensitivity":"normal","payload":{"url":"https://example.invalid","title":"Synthetic service fixture"}
    })).unwrap();
    store
        .ingest(
            UploadBatch {
                batch_id: ulid::Ulid::generate(),
                envelopes: vec![e],
            },
            &Token {
                prefixes: vec!["test:".into()],
                scopes: vec!["ingest".into()],
            },
        )
        .await
        .unwrap();
    store.normalize_window(w).await.unwrap();
    let (_, g) = processing::compile_window(store, w).await.unwrap();
    store.finish_all_jobs().await.unwrap();
    store.settle_inputs().await.unwrap();
    if old {
        store.schedule_daily_sweep(Utc::now()).await.unwrap();
    } else {
        store.request_interpretation(w, g, false).await.unwrap();
        store.make_jobs_due("ready", None).await.unwrap();
    }
    w
}

#[tokio::test]
async fn missing_login_defers_without_spending_attempt() {
    const CASE: &str = "HX_SYNTHETIC_MISSING_LOGIN";
    let Ok(case) = std::env::var(CASE) else {
        for case in ["primary", "sweep", "missing-parent", "grok", "gateway"] {
            let scratch = tempfile::tempdir().unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "missing_login_defers_without_spending_attempt",
                    "--nocapture",
                ])
                .env(CASE, case)
                .env(
                    "HX_FX_PROVIDER",
                    if matches!(case, "grok" | "gateway") {
                        case
                    } else {
                        "codex"
                    },
                )
                .env_remove("AI_GATEWAY_API_KEY")
                .env(
                    "HX_FX_CREDENTIAL",
                    scratch.path().join(if case == "missing-parent" {
                        "absent/login.json"
                    } else {
                        "login.json"
                    }),
                )
                .env("HX_FX_BINARY", scratch.path().join("must-not-launch"))
                .env("HX_INTERPRET_SETTLE_S", "0")
                .status()
                .await
                .unwrap();
            assert!(status.success(), "{case}");
        }
        return;
    };
    let f = Fixture::new().await;
    seed(&f.store, case != "primary").await;
    let runner = processing::Runner {
        store: f.store.clone(),
        provider: processing::AnyProvider::Fx,
    };
    let result = hx_jobs::run_one(&f.store, &runner).await;
    let job = f
        .store
        .job_states()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.kind == "interpret")
        .unwrap();
    // Read the public queue contract through its transaction API: the retry is
    // unavailable now, even though the paid attempt has been refunded.
    let ready = hx_jobs::claim(&f.store, 120).await.unwrap();
    let sweeps = f.store.sweep_reservations().await.unwrap();
    // The typed cause also promises a five-minute delay, rather than a busy loop.
    let before = Utc::now();
    let broker = hx_server::mcp::Broker::new(
        f.store.clone(),
        DisclosurePolicy::default(),
        processing::SensitiveEgress::Full,
        hx_store::window_at(before),
    );
    let error = hx_server::fx::run(&hx_server::fx::Config::from_env(), &broker, "synthetic")
        .await
        .unwrap_err();
    let after = Utc::now();
    let deferred_until = match error.downcast_ref::<hx_jobs::Deferred>() {
        Some(hx_jobs::Deferred::Until(at)) => Some(*at),
        _ => None,
    };
    f.close().await;
    assert!(result.is_ok(), "missing login must defer: {result:?}");
    assert_eq!((job.state.as_str(), job.attempts), ("ready", 0));
    assert!(ready.is_none(), "deferred work must not spin");
    assert!(
        sweeps.iter().all(|s| !s.2),
        "missing login must release the sweep"
    );
    let until = deferred_until.expect("missing login must return Deferred::Until");
    assert!(until >= before + Duration::minutes(5));
    assert!(until <= after + Duration::minutes(5));
}

#[tokio::test]
async fn healthz_is_public_and_only_reports_status() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite:///nonexistent-healthz-fixture.sqlite")
        .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let response = hx_server::router(
        Store::new(pool, scratch.path()),
        DisclosurePolicy::default(),
    )
    .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "ok\n"
    );
}

#[test]
fn version_needs_no_database() {
    let version = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .arg("--version")
        .env_clear()
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        concat!("hx-server ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn doctor_needs_no_database_or_model() {
    for case in [
        "ok",
        "wrong-version",
        "stub",
        "missing",
        "mode",
        "symlink",
        "hardlink",
        "parent-link",
        "gateway",
        "gateway-missing",
    ] {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        let binary = root.join("fx");
        executable(
            &binary,
            &format!(
                "#!/bin/sh\n[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 97\nprintf '%s\\n' '{}'\n",
                if case == "wrong-version" {
                    "0.0.15"
                } else {
                    hx_server::fx::FX_VERSION
                }
            ),
        );
        let login = root.join("login.json");
        std::fs::write(&login, b"{\"synthetic\":true}").unwrap();
        std::fs::set_permissions(
            &login,
            std::fs::Permissions::from_mode(if case == "mode" { 0o644 } else { 0o600 }),
        )
        .unwrap();
        let credential = match case {
            "missing" => root.join("absent/login.json"),
            "symlink" => {
                let link = root.join("link");
                std::os::unix::fs::symlink(&login, &link).unwrap();
                link
            }
            "hardlink" => {
                let link = root.join("link");
                std::fs::hard_link(&login, &link).unwrap();
                link
            }
            "parent-link" => {
                let link = root.join("link");
                std::os::unix::fs::symlink(&root, &link).unwrap();
                link.join("login.json")
            }
            _ => login,
        };
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_hx-server"));
        command
            .arg("doctor")
            .env_clear()
            .env("HOME", &root)
            .env("DATABASE_URL", "not-a-database")
            .env("HX_FX_BINARY", binary)
            .env("HX_FX_CREDENTIAL", credential)
            .env("HX_PROVIDER", if case == "stub" { "stub" } else { "fx" })
            .env(
                "HX_FX_PROVIDER",
                if case.starts_with("gateway") {
                    "gateway"
                } else {
                    "codex"
                },
            );
        if case == "gateway" {
            command.env("AI_GATEWAY_API_KEY", "synthetic-test-key");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.success(),
            matches!(case, "ok" | "gateway"),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-test-key"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-test-key"));
    }
}

#[tokio::test]
async fn sigterm_during_fx_run_leaves_no_child() {
    let f = Fixture::new().await;
    seed(&f.store, false).await;
    let scratch = tempfile::tempdir().unwrap();
    let binary = scratch.path().join("fx");
    let pid_file = scratch.path().join("fx.pid");
    executable(
        &binary,
        &format!(
            "#!/usr/bin/python3\nimport os, pathlib, sys, time\nif sys.argv[1:] == ['--version']:\n print('{}'); sys.exit(0)\npathlib.Path({:?}).write_text(str(os.getpid()))\nwhile True: time.sleep(1)\n",
            hx_server::fx::FX_VERSION,
            pid_file
        ),
    );
    let login = scratch.path().join("login.json");
    std::fs::write(&login, b"{\"synthetic\":true}").unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .args(["serve", "--listen", "127.0.0.1:0"])
        .env_clear()
        .env("HOME", scratch.path())
        .env("HX_STATE_DIR", &f.store.data_dir)
        .env("HX_PROVIDER", "fx")
        .env("HX_FX_BINARY", binary)
        .env("HX_FX_CREDENTIAL", login)
        .env("HX_INTERPRET_SETTLE_S", "0")
        .env("HX_FX_TIMEOUT_S", "60")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let started = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_file)
                && pid.parse::<u32>().is_ok()
            {
                break pid;
            }
            assert!(
                server.try_wait().unwrap().is_none(),
                "scratch server exited before fx started"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("scripted fx must start");
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &server.id().unwrap().to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), server.wait()).await;
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if !Command::new("/bin/kill")
                .args(["-0", &started])
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap()
                .success()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    // Always clean up the synthetic process, including the fail-first run.
    if !stopped {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &started])
            .status()
            .await;
    }
    if status.is_err() {
        server.kill().await.unwrap();
    }
    f.close().await;
    assert!(stopped, "SIGTERM orphaned the scripted fx child");
    assert!(
        status
            .expect("server must shut down promptly")
            .unwrap()
            .success(),
        "SIGTERM must take the graceful shutdown path"
    );
}

#[tokio::test]
async fn default_stub_warns_even_when_logging_is_disabled() {
    use tokio::io::AsyncBufReadExt;
    let f = Fixture::new().await;
    let scratch = tempfile::tempdir().unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_hx-server"))
        .args(["serve", "--listen", "127.0.0.1:0"])
        .env_clear()
        .env("HOME", scratch.path())
        .env("HX_STATE_DIR", &f.store.data_dir)
        .env("RUST_LOG", "off")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(server.stderr.take().unwrap()).lines();
    let warning = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line()).await;
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &server.id().unwrap().to_string()])
        .status()
        .await;
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), server.wait()).await;
    if status.is_err() {
        server.kill().await.unwrap();
    }
    f.close().await;
    let warning = warning.unwrap().unwrap().unwrap();
    assert!(
        warning.contains("WARNING")
            && warning.contains("stub")
            && warning.contains("no real interpretation"),
        "{warning}"
    );
    assert!(status.unwrap().unwrap().success());
}
