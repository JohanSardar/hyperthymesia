#[path = "../../../crates/hx-connector-sdk/tests/support/catch_up.rs"]
mod support;
use hx_connector_sdk::{self as sdk, serde_json::json};
use support::catch_up;

#[test]
fn t3_fifty_thousand_events_uploads_exact_recent_thousand_in_one_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite");
    let mut db = sdk::rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE orchestration_events(sequence INTEGER PRIMARY KEY,event_id TEXT,event_type TEXT,occurred_at TEXT,payload_json TEXT,stream_id TEXT,application_event_version INTEGER);").unwrap();
    let tx = db.transaction().unwrap();
    {
        let mut stmt = tx.prepare("INSERT INTO orchestration_events VALUES(?1,?2,'message.updated',?3,?4,'synthetic',2)").unwrap();
        for n in 0..50_000 {
            let at = if n < 49_000 {
                "2000-01-01T00:00:00Z"
            } else {
                "2000-01-02T00:00:00Z"
            };
            stmt.execute(sdk::rusqlite::params![
                n + 1,
                format!("e{n}"),
                at,
                json!({"id":format!("m{n}"),"n":n}).to_string()
            ])
            .unwrap();
        }
    }
    tx.commit().unwrap();
    catch_up(&path, hx_connector_t3code::scan);
}
