use hx_connector_sdk::{files, serde_json};
use std::path::Path;
#[test]
fn fixtures_have_no_private_paths_emails_urls_or_prose() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sources");
    let patterns = [
        r"(?i)/users/",
        r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}",
        r"(?i)(https?://|www\.)",
        r"[A-Za-z]{2,}(?:[ \t]+[A-Za-z]{2,}){5,}",
    ];
    for path in files(&root, 3, &|_| true).unwrap() {
        let body = std::fs::read_to_string(&path).unwrap();
        for p in patterns {
            assert!(
                !regex::Regex::new(p).unwrap().is_match(&body),
                "fixture privacy violation: {}",
                path.display()
            );
        }
        if path.extension().is_some_and(|s| s == "ndjson") {
            for line in body.lines() {
                let e: hx_schema::Envelope = serde_json::from_str(line).unwrap();
                e.validate().unwrap();
            }
        }
    }
}
#[test]
fn anonymizer_is_deterministic_and_deny_by_default() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/extract-fixtures.sh");
    let input = r#"{"connector_instance":"private","source_key":"secret","revision":"1","observation_id":"random","kind":"agent.message","payload":{"text":"This private natural language must disappear forever.","new_unknown_field":"https://private.invalid","cwd":"/Users/private/repo"}}"#;
    let run = || {
        let mut child = Command::new(&script)
            .arg("--anonymize")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let a = run();
    assert_eq!(a, run());
    assert!(!a.contains("private"));
    assert!(!a.contains("/Users/"));
    assert!(!a.contains("https://"));
    assert!(a.contains("agent.message"));
}
