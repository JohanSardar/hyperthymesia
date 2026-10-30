//! T3's populated application feed; projections enrich, but never replace, event history.
use anyhow::{Result, ensure};
use hx_connector_sdk::{self as sdk, Request, Scan, agent, digest, link, timestamp};
use hx_schema::Operation;
use serde_json::{Value, json};
use std::path::Path;

pub fn main() -> Result<()> {
    sdk::run(
        "t3code",
        "~/.t3/userdata/statev2.sqlite",
        &["agent.message"],
        scan,
    )
}

pub fn parse(
    req: &Request,
    sequence: u64,
    event_id: &str,
    event_type: &str,
    at: &str,
    payload: &Value,
    context: &Value,
) -> Option<hx_schema::Envelope> {
    if event_type.starts_with("project.") {
        return None;
    }
    let message = event_type.starts_with("message.");
    // Message-shaped turn items duplicate message events; preserve other tool/lifecycle items.
    if event_type.starts_with("turn-item.") && payload["messageId"].is_string() {
        return None;
    }
    let key = payload["id"]
        .as_str()
        .or_else(|| payload["messageId"].as_str())
        .or_else(|| payload["threadId"].as_str())
        .unwrap_or(event_id);
    let thread = payload["threadId"]
        .as_str()
        .or_else(|| payload["appThreadId"].as_str())
        .or_else(|| context["thread_id"].as_str())
        .unwrap_or(key);
    let role = match payload["role"].as_str() {
        Some("user") => "user",
        Some("assistant") => "assistant",
        Some("tool") => "tool",
        _ if event_type.starts_with("turn-item.") => "tool",
        _ => "lifecycle",
    };
    let mut e = agent(
        req,
        &format!("{}:{key}", event_type.split('.').next().unwrap_or("event")),
        &format!("{sequence:020}"),
        json!({
            "provider":"t3code", "thread_id":thread, "thread_title":context["title"],
            "turn_id":payload["turnId"].as_str().or_else(|| payload["providerTurnId"].as_str()).or_else(|| payload["runId"].as_str()),
            "role":role, "text":payload["text"].as_str().unwrap_or(if message { "" } else { event_type }),
            "cwd":context["cwd"], "model":context["model"], "event_type":event_type,
            "native_event_id":event_id,"native":payload,
        }),
        timestamp(&json!(at)),
    );
    e.payload["native_digest"] = json!(digest(payload));
    e.sequence = Some(sequence);
    if event_type.ends_with(".deleted") {
        e.operation = Operation::Delete;
    }
    link(
        &mut e,
        "session",
        context["session"].as_str().unwrap_or_default(),
    );
    if context["session"].is_null() {
        link(
            &mut e,
            "session",
            payload["providerSessionId"].as_str().unwrap_or_default(),
        );
    }
    Some(e)
}

pub fn scan(req: &Request, state: &Value) -> Result<Scan> {
    let path = Path::new(&req.source);
    let db = sdk::readonly(path)?;
    let generation: String = db.query_row(
        "SELECT event_id FROM orchestration_events ORDER BY sequence LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    let mut position = if state["generation"] == generation {
        state["sequence"].as_i64().unwrap_or(0)
    } else {
        0
    };
    let mut out = Scan::new(state.clone());
    let mut stmt = db.prepare("SELECT sequence,event_id,event_type,occurred_at,payload_json,stream_id,application_event_version FROM orchestration_events WHERE sequence > ? ORDER BY sequence LIMIT ?")?;
    let rows = stmt
        .query_map(sdk::rusqlite::params![position, req.limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    out.complete = rows.len() < req.limit;
    for (seq, id, typ, at, raw, stream, version) in rows {
        ensure!(
            version == 1 || version == 2,
            "unsupported T3 application event version"
        );
        let payload: Value = serde_json::from_str(&raw)?;
        let thread = payload["threadId"]
            .as_str()
            .or_else(|| payload["appThreadId"].as_str())
            .unwrap_or(&stream);
        let mut context = json!({"thread_id":thread});
        let enriched = db.query_row("SELECT t.title,COALESCE(l.worktree_path,p.workspace_root),t.payload_json FROM orchestration_v2_projection_threads t LEFT JOIN projection_threads l ON l.thread_id=t.thread_id LEFT JOIN projection_projects p ON p.project_id=t.project_id WHERE t.thread_id=?", [thread], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?)));
        if let Ok((title,cwd,raw)) = enriched {
            let t: Value = serde_json::from_str(&raw)?;
            context["title"] = json!(title); context["cwd"] = json!(cwd); context["model"] = t["modelSelection"]["model"].clone();
        } else if let Ok((title,cwd,model)) = db.query_row("SELECT t.title,COALESCE(t.worktree_path,p.workspace_root),t.model_selection_json FROM projection_threads t LEFT JOIN projection_projects p ON p.project_id=t.project_id WHERE t.thread_id=?", [thread], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))) {
            context["title"] = json!(title); context["cwd"] = json!(cwd);
            context["model"] = model.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_default()["model"].clone();
        }
        if let Ok(session) = db.query_row("SELECT COALESCE(provider_session_id,json_extract(payload_json,'$.nativeThreadRef.nativeId')) FROM orchestration_v2_projection_provider_threads WHERE thread_id=? LIMIT 1", [thread], |r| r.get::<_,Option<String>>(0)) { context["session"] = json!(session); }
        if context["session"].is_null() && let Ok(session) = db.query_row("SELECT COALESCE(provider_thread_id,provider_session_id) FROM projection_thread_sessions WHERE thread_id=?", [thread], |r| r.get::<_,Option<String>>(0)) { context["session"] = json!(session); }
        if let Some(e) = parse(req, seq as u64, &id, &typ, &at, &payload, &context)
            && sdk::after_since(req, &e)
        {
            out.push(e);
        }
        position = seq;
    }
    out.next = json!({"generation":generation,"sequence":position});
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn fixture() -> Vec<Value> {
        include_str!("../../../fixtures/sources/t3code/input.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    #[test]
    fn fixtures_validate_replay_and_update_identity() {
        let run = || {
            fixture()
                .iter()
                .filter_map(|v| {
                    parse(
                        &req(),
                        v["sequence"].as_u64().unwrap(),
                        v["event_id"].as_str().unwrap(),
                        v["event_type"].as_str().unwrap(),
                        v["occurred_at"].as_str().unwrap(),
                        &v["payload"],
                        &json!({}),
                    )
                })
                .collect::<Vec<_>>()
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), 3);
        for (x, y) in a.iter().zip(&b) {
            x.validate().unwrap();
            assert_eq!(x.dedup_key(), y.dedup_key());
        }
        assert_eq!(a[0].source_key, a[1].source_key);
        assert!(a[0].revision < a[1].revision);
        assert_eq!(a[2].operation, Operation::Delete);
    }
    #[test]
    fn message_turn_items_are_not_double_claimed() {
        assert!(
            parse(
                &req(),
                1,
                "h_e",
                "turn-item.updated",
                "2000-01-01T00:00:00Z",
                &json!({"id":"h_i","messageId":"h_m"}),
                &json!({})
            )
            .is_none()
        );
    }
    #[test]
    fn ordered_sqlite_feed_resumes_without_replacing_message_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.sqlite");
        let db = sdk::rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE orchestration_events(sequence INTEGER PRIMARY KEY,event_id TEXT,event_type TEXT,occurred_at TEXT,payload_json TEXT,stream_id TEXT,application_event_version INTEGER);").unwrap();
        for row in fixture() {
            db.execute(
                "INSERT INTO orchestration_events VALUES(?1,?2,?3,?4,?5,'h_t1',2)",
                sdk::rusqlite::params![
                    row["sequence"].as_i64().unwrap(),
                    row["event_id"].as_str().unwrap(),
                    row["event_type"].as_str().unwrap(),
                    row["occurred_at"].as_str().unwrap(),
                    row["payload"].to_string()
                ],
            )
            .unwrap();
        }
        let mut r = req();
        r.source = path.to_string_lossy().into();
        r.limit = 2;
        let a = scan(&r, &Value::Null).unwrap();
        assert_eq!(a.envelopes.len(), 2);
        assert_eq!(a.next["sequence"], 2);
        assert!(!a.complete);
        let b = scan(&r, &a.next).unwrap();
        assert_eq!(b.envelopes.len(), 1);
        assert_eq!(b.envelopes[0].operation, Operation::Delete);
        assert!(b.complete);
        assert_eq!(a.envelopes[0].source_key, b.envelopes[0].source_key);
        assert!(scan(&r, &b.next).unwrap().envelopes.is_empty());
    }
}
