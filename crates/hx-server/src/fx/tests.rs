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
