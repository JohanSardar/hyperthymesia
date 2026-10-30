#[path = "../../../crates/hx-connector-sdk/tests/support/catch_up.rs"]
mod support;
use hx_connector_sdk::serde_json::json;
use std::io::Write;
use support::catch_up;

#[test]
fn codex_thousand_rollouts_uploads_exact_recent_thousand_with_compact_state() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    for file in 0..1000 {
        let mut out =
            std::fs::File::create(sessions.join(format!("rollout-{file:04}.jsonl"))).unwrap();
        writeln!(
            out,
            "{}",
            json!({"type":"session_meta","payload":{"id":format!("session{file}")}})
        )
        .unwrap();
        for line in 0..50 {
            let n = file * 50 + line;
            let at = if n < 49_000 {
                "2000-01-01T00:00:00Z"
            } else {
                "2000-01-02T00:00:00Z"
            };
            writeln!(out,"{}",json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"user","id":format!("m{n}"),"n":n,"content":"synthetic"}})).unwrap();
        }
    }
    let (_, size) = catch_up(dir.path(), hx_connector_codex::scan);
    assert!(size < 100_000, "cursor map should remain compact: {size}");
}

#[test]
fn codex_compressed_old_prefix_does_not_consume_the_upload_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-large.jsonl.zst");
    let mut out = hx_connector_sdk::zstd::stream::write::Encoder::new(
        std::fs::File::create(&path).unwrap(),
        1,
    )
    .unwrap();
    writeln!(
        out,
        "{}",
        json!({"type":"session_meta","payload":{"id":"synthetic_session"}})
    )
    .unwrap();
    for n in 0..50_000 {
        let at = if n < 49_000 {
            "2000-01-01T00:00:00Z"
        } else {
            "2000-01-02T00:00:00Z"
        };
        writeln!(out,"{}",json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"user","id":format!("m{n}"),"n":n,"content":"synthetic"}})).unwrap();
    }
    out.finish().unwrap();
    catch_up(&path, hx_connector_codex::scan);
}
