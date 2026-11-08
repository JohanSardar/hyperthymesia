use super::*;
/// Re-exec keeps environment selection realistic without mutating Rust's shared env.
#[test]
#[cfg(unix)]
fn configured_fx_is_isolated_and_rejects_bad_logins_before_launch() {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(case) = std::env::var("HX_FX_TEST_CASE") {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
                let config = Config::from_env();
                if case == "default" {
                    let window = Interval {
                        start: "2026-01-01T00:00:00Z".parse().unwrap(),
                        end: "2026-01-01T04:00:00Z".parse().unwrap(),
                    };
                    let policy = DisclosurePolicy::default();
                    let sensitive = SensitiveEgress::from_env();
                    let namespaces = vec!["synthetic:".into()];
                    // Verbatim legacy shape, with literal defaults: serialized bytes are the contract.
                    let legacy = json!({"window":window,"timeline_hash":"synthetic-hash","input_generation":7,"provider":"fx","fx_version":"0.0.16","model":"gpt-6.1-sol","effort":"xhigh","fast":true,"prompt_version":PROMPT_VERSION,"prompt_sha256":hx_store::hash(PROMPT.as_bytes()),"sensitive_egress":sensitive,"disclosure_policy":{"digest":policy.digest(),"policy":policy},"input_namespaces":namespaces});
                    let manifest = Selection::from_env().unwrap().input_manifest(window, "synthetic-hash", 7, &policy, sensitive, &namespaces);
                    assert_eq!(serde_json::to_vec(&manifest).unwrap(), serde_json::to_vec(&legacy).unwrap());
                }
                let pool = sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_lazy("sqlite:///nonexistent-fx-fixture.sqlite")
                    .unwrap();
                let broker = Broker::new(
                    Store::new(pool, std::env::var_os("HOME").unwrap()),
                    DisclosurePolicy::default(),
                    SensitiveEgress::from_env(),
                    Interval {
                        start: "2026-01-01T00:00:00Z".parse().unwrap(),
                        end: "2026-01-01T04:00:00Z".parse().unwrap(),
                    },
                );
                let result = run(&config, &broker, "synthetic instruction").await;
                if let Some(reason) = case.strip_prefix("bad-") {
                    let error = format!("{:#}", result.unwrap_err());
                    assert!(error.contains(reason.split(':').next().unwrap()), "{case}: {error}");
                    assert!(!config.binary.with_extension("called").exists());
                } else {
                    let result = result.unwrap_or_else(|error| panic!("{error:#}: {}", std::fs::read_to_string(config.binary.with_extension("error")).unwrap_or_default()));
                    assert_eq!(result["model"], std::env::var("HX_FX_MODEL").unwrap_or_else(|_| MODEL.into()));
                }
            });
        return;
    }
    let mut failures = vec![];
    for case in [
        "default",
        "codex",
        "grok",
        "gateway",
        "bad-AI_GATEWAY_API_KEY",
        "bad-AI_GATEWAY_API_KEY:missing",
        "bad-credential",
        "bad-credential:missing",
        "bad-credential:symlink",
        "bad-provider",
        "bad-HX_FX_MODEL",
        "bad-HX_FX_EFFORT",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("fake-fx");
        let script = r#"#!/usr/bin/python3
import json, os, pathlib, stat, sys, traceback
sys.excepthook = lambda kind, error, tb: pathlib.Path(__file__).with_suffix('.error').write_text(''.join(traceback.format_exception(kind, error, tb)))
pathlib.Path(__file__).with_suffix('.called').touch()
if '--version' in sys.argv:
 print('0.0.16'); sys.exit(0)
home = pathlib.Path(os.environ['HOME'])
assert home != pathlib.Path(__file__).parent
assert pathlib.Path.cwd().resolve() == (home/'work').resolve()
assert not {'VERCEL_OIDC_TOKEN','FX_AI_GATEWAY_API_KEY','UNRELATED_SECRET','HX_FX_MODEL'} & os.environ.keys()
assert os.environ['FX_DISABLE_KEYCHAIN'] == '1'
provider = os.environ['FX_PROVIDER']; model = os.environ['FX_MODEL']
assert provider == 'EXPECTED_PROVIDER'
settings_bytes = (home/'.fx/settings.json').read_bytes()
settings = json.loads(settings_bytes)
assert settings['provider'] == provider and settings['models'] == {provider: model}
assert sys.argv[sys.argv.index('--model')+1] == model
assert sys.argv[sys.argv.index('--effort')+1] == settings['effort']
assert settings['permission']['*'] == 'deny' and settings['context'] is False
if model == 'gpt-6.1-sol':
 legacy = {"provider":"codex","models":{"codex":"gpt-6.1-sol"},"context":False,"max_agent_steps":120,"effort":"xhigh","fast_mode":True,"max_tool_result_bytes":65536,"auto_upgrade":False,"session_titles":False,"permission_mode":"ask","permission":{"*":"deny","capability_search":"allow","mcp_select_tool":"allow","mcp_hyperthymesia_*":"allow"},"prompt_history":{"enabled":False},"notifications":{"turn_end":False,"attention_required":False}}
 assert settings_bytes == json.dumps(legacy, sort_keys=True, separators=(',', ':')).encode()
else:
 assert settings['effort'] == 'low'
if provider == 'gateway':
 assert os.environ['AI_GATEWAY_API_KEY'] == 'synthetic-key'
 assert not list((home/'.fx').glob('*auth.json'))
else:
 assert 'AI_GATEWAY_API_KEY' not in os.environ
 name = 'grok-auth.json' if provider == 'grok' else 'chatgpt-auth.json'
 login = home/'.fx'/name
 assert stat.S_IMODE(login.stat().st_mode) == 0o600 and login.stat().st_nlink == 1
 assert json.loads(login.read_text()) == {}
 login.write_text('{"synthetic_refreshed":true}')
print(json.dumps({'exit_code':0,'model':model,'steps':2,'final_output':'synthetic answer','tool_calls':[]}))
"#;
        let provider = if case.contains("gateway") || case.contains("AI_GATEWAY") {
            "gateway"
        } else if case == "grok" {
            "grok"
        } else {
            "codex"
        };
        std::fs::write(&binary, script.replace("EXPECTED_PROVIDER", provider)).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let profile = dir.path().join(".fx");
        std::fs::create_dir(&profile).unwrap();
        let name = if provider == "grok" {
            "grok-auth.json"
        } else {
            "chatgpt-auth.json"
        };
        let credential = profile.join(name);
        std::fs::write(
            &credential,
            if case == "bad-credential" {
                "not json"
            } else {
                "{}"
            },
        )
        .unwrap();
        if case == "bad-credential:missing" {
            std::fs::remove_file(&credential).unwrap();
        } else if case == "bad-credential:symlink" {
            let target = profile.join("synthetic-target.json");
            std::fs::rename(&credential, &target).unwrap();
            std::os::unix::fs::symlink(target, &credential).unwrap();
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "fx::tests::configured_fx_is_isolated_and_rejects_bad_logins_before_launch",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", dir.path())
            .env("HX_FX_TEST_CASE", case)
            .env("HX_FX_BINARY", &binary)
            .env("UNRELATED_SECRET", "synthetic-canary")
            .env("VERCEL_OIDC_TOKEN", "synthetic-canary")
            .env("FX_AI_GATEWAY_API_KEY", "synthetic-canary")
            .env(
                "AI_GATEWAY_API_KEY",
                if case == "bad-AI_GATEWAY_API_KEY" {
                    " "
                } else {
                    "synthetic-key"
                },
            );
        if case == "bad-AI_GATEWAY_API_KEY:missing" {
            command.env_remove("AI_GATEWAY_API_KEY");
        }
        if case != "default" {
            command
                .env(
                    "HX_FX_PROVIDER",
                    if case == "bad-provider" {
                        "unknown"
                    } else {
                        provider
                    },
                )
                .env(
                    "HX_FX_MODEL",
                    if case == "bad-HX_FX_MODEL" {
                        " "
                    } else {
                        "synthetic-model"
                    },
                )
                .env(
                    "HX_FX_EFFORT",
                    if case == "bad-HX_FX_EFFORT" {
                        " "
                    } else {
                        "low"
                    },
                );
        }
        let output = command.output().unwrap();
        if !output.status.success() {
            failures.push(format!(
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }
        if matches!(case, "default" | "codex" | "grok") {
            assert_eq!(
                std::fs::read_to_string(credential).unwrap(),
                "{\"synthetic_refreshed\":true}"
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn repair_excerpt_keeps_the_header_and_nearby_lines_only() {
    let window = Interval {
        start: "2026-01-01T20:00:00Z".parse().unwrap(),
        end: "2026-01-02T00:00:00Z".parse().unwrap(),
    };
    let digest = "window header\nFormat line\n\n\
            [1] 20:00:00-20:05:00 Editor | a\n  screen text one\n\
            [2] 21:00:00-21:10:00 Browser | b\n  screen text two\n\
            [3] 23:50:00-00:00:00 Mail | c\n\n\
            Unknown stretches (capture not verified):\n\
            [4] 20:06:00-20:07:00\n[5] 22:00:00-22:01:00\n";
    let around = [Interval {
        start: "2026-01-01T21:12:00Z".parse().unwrap(),
        end: "2026-01-01T21:13:00Z".parse().unwrap(),
    }];
    let excerpt = digest_excerpt(digest, window, &around);
    assert!(excerpt.starts_with("window header\nFormat line\n\n"));
    assert!(excerpt.contains("[2] 21:00:00-21:10:00") && excerpt.contains("screen text two"));
    assert!(!excerpt.contains("[1]") && !excerpt.contains("screen text one"));
    assert!(excerpt.contains("Unknown stretches") && !excerpt.contains("[4]"));
    let late = [Interval {
        start: "2026-01-01T23:55:00Z".parse().unwrap(),
        end: "2026-01-02T00:00:00Z".parse().unwrap(),
    }];
    assert!(digest_excerpt(digest, window, &late).contains("[3] 23:50:00-00:00:00"));
}
#[test]
fn long_source_spans_only_support_the_start_but_meetings_keep_their_span() {
    let window = Interval {
        start: "2026-01-01T10:00:00Z".parse().unwrap(),
        end: "2026-01-01T14:00:00Z".parse().unwrap(),
    };
    for (connector, start, hours, extra_seconds, point) in [
        ("source:mail", window.start, 4, 1, true),
        ("source:mail", window.start, 4, 0, false),
        ("source:mail", window.start, 1, 0, false),
        (
            "source:mail",
            window.start - chrono::Duration::hours(1),
            6,
            0,
            true,
        ),
        ("editor:synthetic", window.start, 6, 0, false),
    ] {
        let end = start + chrono::Duration::hours(hours) + chrono::Duration::seconds(extra_seconds);
        let envelope = serde_json::from_value(json!({
            "envelope_version":1, "observation_id":"01J0000000000000000000000A",
            "connector_instance":connector, "source_key":"synthetic-item", "revision":"1",
            "operation":"upsert", "kind":"source.email", "schema_version":1,
            "time":{"source_start":start,"source_end":end,"observed_at":end,"resolution_ms":1},
            "links":[], "sensitivity":"normal", "payload":{"title":"Synthetic meeting"}, "blobs":[]
        }))
        .unwrap();
        let observations = [hx_compile::Observation {
            id: 1,
            received_at: end,
            envelope,
        }];
        let timeline = hx_compile::compile(&observations, window);
        let digest = hx_interpret::build_digest(
            &timeline,
            &observations,
            &[],
            hx_interpret::DigestConfig::default(),
        );
        let mut support = citation_support(&timeline, &observations, window);
        for (id, spans) in digest.support() {
            support.entry(id).or_default().extend(spans);
        }
        let expected = if point {
            Interval {
                start,
                end: start + chrono::Duration::seconds(1),
            }
        } else {
            Interval { start, end }.intersect(&window).unwrap()
        };
        if window.contains(start) {
            assert_eq!(
                digest.lines,
                vec![(expected, vec![1])],
                "{connector} {hours}h +{extra_seconds}s"
            );
            assert!(digest.text.contains(&format!(
                "{}-{} source.email",
                expected.start.format("%H:%M:%S"),
                expected.end.format("%H:%M:%S")
            )));
        } else {
            assert!(
                digest.lines.is_empty(),
                "an out-of-window start must not be clamped into the window"
            );
        }
        if point {
            assert!(
                support
                    .values()
                    .flatten()
                    .all(|s| s.start == start && s.end <= start + chrono::Duration::seconds(1)),
                "{support:?}"
            );
        } else {
            assert!(support[&1].contains(&expected));
        }
        let mut packet = hx_interpret::build_packet(
            &timeline,
            &[hx_interpret::EvidenceItem::classified(
                1,
                start,
                "normal:source.email",
                "Synthetic meeting",
                hx_schema::Sensitivity::Normal,
                false,
            )
            .unwrap()],
            &[],
            &[],
        );
        let spans: Vec<_> = support.values().flatten().collect();
        packet.timeline_text = format!(
            "time_support={}\ntime_support_by_evidence={}\n",
            serde_json::to_string(&spans).unwrap(),
            serde_json::to_string(&support).unwrap()
        );
        let claimed = if point { window } else { expected };
        let interpretation: hx_interpret::Interpretation = serde_json::from_value(json!({
                "window":window,"episodes":[{"id":"synthetic","intervals":[claimed],"title":"Synthetic meeting","description":"Source reported a meeting",
                "claims":[{"actor":"source","action":"reported","object":"meeting","outcome":"unknown","basis":"reported_by_source","evidence":[1]}],"evidence":[1],"confidence":0.5}],
                "tasks":[],"summary":"Synthetic meeting","check_failures":[],"provenance":{"input_digest":packet.input_digest,"compiler_version":timeline.compiler_version,"prompt_version":hx_interpret::PROMPT_VERSION,"model":"synthetic","created_at":window.start}
            })).unwrap();
        assert_eq!(
            hx_interpret::check(&interpretation, &packet).is_empty(),
            !point
        );
    }
}

#[test]
fn citations_support_raw_wall_times_clipped_to_the_window() {
    // A focus span opened before the window whose monotonic duration ends 3 ms
    // after its declared wall end, as real macOS spans do.
    let envelope: hx_schema::Envelope = serde_json::from_value(json!({
            "envelope_version": 1,
            "observation_id": "01J0000000000000000000000A",
            "connector_instance": "macos-agent:synthetic",
            "source_key": "focus:synthetic",
            "revision": "1",
            "operation": "upsert",
            "kind": "focus.span",
            "schema_version": 1,
            "time": {
                "source_start": "2026-01-01T09:58:00Z",
                "source_end": "2026-01-01T10:05:00Z",
                "resolution_ms": 0,
                "observed_at": "2026-01-01T10:05:00Z",
                "mono_start": {"domain": "mach_continuous", "boot_id": "synthetic-boot", "nanos": 0},
                "mono_end": {"domain": "mach_continuous", "boot_id": "synthetic-boot", "nanos": 420_003_000_000u64}
            },
            "links": [],
            "sensitivity": "normal",
            "payload": {"app_name": "Synthetic Browser", "app_bundle_id": "com.example.browser", "window_title": "Synthetic page", "pid": 1, "display": null},
            "blobs": []
        }))
        .unwrap();
    let window = Interval {
        start: "2026-01-01T10:00:00Z".parse().unwrap(),
        end: "2026-01-01T14:00:00Z".parse().unwrap(),
    };
    let observations = vec![hx_compile::Observation {
        id: 7,
        received_at: window.start,
        envelope,
    }];
    let timeline = hx_compile::compile(&observations, window);
    let support = citation_support(&timeline, &observations, window);
    let declared = Interval {
        start: window.start,
        end: "2026-01-01T10:05:00Z".parse().unwrap(),
    };
    assert!(support[&7].contains(&declared), "{:?}", support[&7]);
    assert!(support[&7].iter().all(|s| s.start >= window.start));

    // With no declared time and no compiled span, keep the broker's
    // observed_at point. This must not acquire a duration or use received_at.
    let mut untimed = observations[0].clone();
    untimed.id = 8;
    untimed.envelope.time.source_start = None;
    untimed.envelope.time.source_end = None;
    untimed.envelope.time.mono_start = None;
    untimed.envelope.time.mono_end = None;
    let support = citation_support(&timeline, std::slice::from_ref(&untimed), window);
    assert_eq!(
        support[&8],
        vec![Interval {
            start: untimed.envelope.time.observed_at,
            end: untimed.envelope.time.observed_at,
        }]
    );
    untimed.envelope.time.observed_at = window.start - chrono::Duration::seconds(1);
    assert!(!citation_support(&timeline, &[untimed], window).contains_key(&8));
}
#[test]
fn isolated_credential_refresh_survives_cleanup_and_serializes_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("synthetic-auth.json");
    std::fs::write(&source, b"{}").unwrap();
    let isolated = dir.path().join("synthetic-isolated.json");
    let lease = CredentialLease::acquire(&source, isolated.clone()).unwrap();
    let busy = CredentialLease::acquire(&source, dir.path().join("other.json"))
        .err()
        .unwrap();
    assert!(matches!(
        busy.downcast_ref::<hx_jobs::Deferred>(),
        Some(hx_jobs::Deferred::ProviderBusy)
    ));
    std::fs::write(&isolated, b"{\"synthetic\":true}").unwrap();
    drop(lease);
    assert_eq!(std::fs::read(&source).unwrap(), b"{\"synthetic\":true}");
    let _next = CredentialLease::acquire(&source, isolated).unwrap();
}
