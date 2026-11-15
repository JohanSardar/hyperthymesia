use hx_agent::{
    config::Config,
    controls::Controls,
    envelope::Producer,
    outbox::Outbox,
    policy::{Policy, persist},
};
use hx_schema::{Completeness, Operation, kinds::*};

#[test]
fn s2_never_record_site_does_not_persist_text_when_ax_url_is_unavailable() {
    let scratch = tempfile::tempdir().unwrap();
    let controls = Controls::load(scratch.path());
    controls.adopt(&serde_json::json!({
        "changed_at": "2026-11-15T00:00:00Z",
        "paused_until": null,
        "never_record": {"apps": [], "sites": ["mail.google.com"]},
        "notices": {}
    }));
    // AX URL reads are bounded and may return None. A Gmail window title
    // (unlike its address) need not contain mail.google.com.
    let focus = hx_macos_sys::Focus {
        bundle_id: "com.google.Chrome".into(),
        name: "Google Chrome".into(),
        pid: 1,
        title: Some("Inbox (1) - alice@gmail.com - Gmail".into()),
        electron: false,
        url: None,
    };
    let text = "Private message body";
    let envelope = Producer::new("review".into()).unwrap().core(
        "private-mail",
        "0",
        Operation::Observe,
        CorePayload::TextSnapshot(TextSnapshot {
            surface: focus.bundle_id.clone(),
            source: TextSource::Accessibility,
            text: text.into(),
            text_sha256: hx_schema::sha256_hex(text.as_bytes()),
            completeness: Completeness::Partial,
            visible: None,
        }),
    );
    envelope.validate().unwrap();
    let policy = Policy::new(&Config::default()).unwrap();
    let mut outbox = Outbox::open(&scratch.path().join("outbox.sqlite"), 100_000, 8).unwrap();
    persist(&mut outbox, &policy, Some(&focus), false, &envelope, &[]).unwrap();
    let stored = outbox.batch().unwrap();
    assert!(
        stored.is_none(),
        "never-record site content reached the durable upload batch: {stored:?}"
    );
}
