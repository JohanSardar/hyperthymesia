//! Codex session rollouts with persisted context and decompressed byte offsets.
use anyhow::Result;
use hx_connector_sdk::{self as sdk, Request, Scan, agent, digest, link, timestamp};
use serde_json::{Value, json};
use std::path::Path;

pub fn main() -> Result<()> {
    sdk::run("codex", "~/.codex", &["agent.message"], scan)
}

pub fn parse(
    req: &Request,
    v: &Value,
    ctx: &mut Value,
    fallback: &str,
) -> Option<hx_schema::Envelope> {
    let typ = v["type"].as_str()?;
    let p = &v["payload"];
    if typ == "session_meta" {
        ctx["session"] = p["session_id"]
            .as_str()
            .or_else(|| p["id"].as_str())
            .map(|s| json!(s))
            .unwrap_or_else(|| json!(fallback));
        for k in ["cwd", "originator", "source"] {
            ctx[k] = p[k].clone();
        }
        return None;
    }
    if typ == "turn_context" {
        ctx["turn"] = p["turn_id"].clone();
        for k in ["cwd", "model"] {
            ctx[k] = p[k].clone();
        }
        return None;
    }
    if typ == "event_msg" && p["type"] == "thread_settings_updated" {
        ctx["model"] = p["thread_settings"]["model"].clone();
        return None;
    }
    let sub = p["type"].as_str().unwrap_or(typ);
    // event_msg user/assistant echoes duplicate the authoritative response_item messages.
    if typ == "event_msg"
        && matches!(
            sub,
            "user_message"
                | "agent_message"
                | "item_completed"
                | "token_count"
                | "agent_reasoning"
                | "agent_reasoning_raw_content"
        )
    {
        return None;
    }
    if typ != "response_item" && typ != "event_msg" {
        return None;
    }
    if sub == "task_started" {
        ctx["turn"] = p["turn_id"].clone();
    }
    let session = ctx["session"].as_str().unwrap_or(fallback);
    let id = p["id"]
        .as_str()
        .or_else(|| p["call_id"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}:{}", ctx["turn"].as_str().unwrap_or_default(), digest(v)));
    let role = match p["role"].as_str() {
        Some("user") => "user",
        Some("assistant") => "assistant",
        _ if sub.contains("call") || sub.contains("tool") => "tool",
        _ => "lifecycle",
    };
    let mut e = agent(
        req,
        &format!("{session}:{id}"),
        &digest(p),
        json!({"provider":"codex","thread_id":session,"thread_title":ctx["title"],"turn_id":p["turn_id"].as_str().or_else(||ctx["turn"].as_str()),"role":role,"text":sdk::text(p),"cwd":ctx["cwd"],"model":ctx["model"],"event_type":sub,"originator":ctx["originator"],"native":p}),
        timestamp(&v["timestamp"]),
    );
    link(&mut e, "session", session);
    if ctx["t3_thread"].is_string()
        || ctx["originator"]
            .as_str()
            .is_some_and(|s| s.to_lowercase().contains("t3"))
    {
        link(
            &mut e,
            "same_as",
            ctx["t3_thread"].as_str().unwrap_or_default(),
        );
        e.payload["claim_scope"] = json!("provider_provenance");
    }
    Some(e)
}

fn t3_thread(db: &sdk::rusqlite::Connection, session: &str) -> Option<String> {
    for query in [
        "SELECT thread_id FROM orchestration_v2_projection_provider_threads WHERE provider_session_id=?1 OR json_extract(payload_json,'$.nativeThreadRef.nativeId')=?1 LIMIT 1",
        "SELECT thread_id FROM projection_thread_sessions WHERE provider_session_id=?1 OR provider_thread_id=?1 LIMIT 1",
        "SELECT thread_id FROM orchestration_v2_projection_provider_session_bindings WHERE provider_session_id=?1 LIMIT 1",
    ] {
        if let Ok(thread) = db.query_row(query, [session], |r| r.get::<_, String>(0)) {
            return Some(thread);
        }
    }
    None
}
pub fn scan(req: &Request, state: &Value) -> Result<Scan> {
    let root = Path::new(&req.source);
    let mut positions = sdk::tail_state(state)?;
    let mut out = Scan::new(state.clone());
    let sessions = if root.is_file() {
        root.to_path_buf()
    } else {
        root.join("sessions")
    };
    let files = sdk::files(&sessions, 12, &|p| {
        p.file_name().is_some_and(|s| {
            s.to_string_lossy().starts_with("rollout-")
                && (s.to_string_lossy().ends_with(".jsonl")
                    || s.to_string_lossy().ends_with(".jsonl.zst"))
        })
    })?;
    let threads = if root.join("state_5.sqlite").exists() {
        Some(sdk::readonly(&root.join("state_5.sqlite"))?)
    } else {
        None
    };
    let t3_path = sdk::home().join(".t3/userdata/statev2.sqlite");
    let t3 = if t3_path.exists() {
        Some(sdk::readonly(&t3_path)?)
    } else {
        None
    };
    if files.is_empty() {
        return Ok(Scan::unknown(state.clone(), "no Codex rollout files found"));
    }
    for path in files {
        if out.envelopes.len() >= req.limit {
            out.complete = false;
            break;
        }
        let key = path.to_string_lossy().to_string();
        let old = positions.get(&key).cloned().unwrap_or_default();
        let (lines, mut cursor, complete) =
            sdk::tail(&path, &old, req.limit - out.envelopes.len())?;
        out.complete &= complete;
        if !cursor.context.is_object() {
            cursor.context = json!({});
        }
        for line in lines {
            let v = line.value;
            if v["type"] == "session_meta" {
                let session = v["payload"]["session_id"]
                    .as_str()
                    .or_else(|| v["payload"]["id"].as_str())
                    .unwrap_or_default();
                if let Some(db) = &threads
                    && let Ok((title, cwd, model)) = db.query_row(
                        "SELECT title,cwd,model FROM threads WHERE id=?",
                        [session],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, Option<String>>(2)?,
                            ))
                        },
                    )
                {
                    cursor.context["title"] = json!(title);
                    cursor.context["cwd"] = json!(cwd);
                    cursor.context["model"] = json!(model);
                }
                if let Some(db) = &t3
                    && let Some(thread) = t3_thread(db, session)
                {
                    cursor.context["t3_thread"] = json!(thread);
                }
            }
            if let Some(e) = parse(
                req,
                &v,
                &mut cursor.context,
                &format!("{}:{}", cursor.generation, line.offset),
            ) && sdk::after_since(req, &e)
            {
                out.push(e);
            }
        }
        positions.insert(key, cursor);
    }
    out.next = serde_json::to_value(positions)?;
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
        include_str!("../../../fixtures/sources/codex/input.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    #[test]
    fn fixtures_validate_replay_and_updates() {
        let run = || {
            let mut ctx = json!({"t3_thread":"h_t3"});
            fixture()
                .iter()
                .filter_map(|v| parse(&req(), v, &mut ctx, "h_fallback"))
                .collect::<Vec<_>>()
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), 2);
        for (x, y) in a.iter().zip(&b) {
            x.validate().unwrap();
            assert_eq!(x.dedup_key(), y.dedup_key());
            assert!(x.links.iter().any(|l| l.rel == "same_as"));
        }
        assert_eq!(a[0].source_key, a[1].source_key);
        assert_ne!(a[0].revision, a[1].revision);
    }
    #[test]
    fn partial_line_sync_preserves_session_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sessions")).unwrap();
        let path = dir.path().join("sessions/rollout-test.jsonl");
        let rows = fixture();
        let mut content = rows[..3]
            .iter()
            .map(|v| format!("{v}\n"))
            .collect::<String>();
        let last = rows[3].to_string();
        let half = last.len() / 2;
        content.push_str(&last[..half]);
        std::fs::write(&path, content).unwrap();
        let mut r = req();
        r.source = dir.path().to_string_lossy().into();
        let a = scan(&r, &Value::Null).unwrap();
        assert_eq!(a.envelopes.len(), 1);
        assert!(!a.complete);
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new().append(true).open(path).unwrap(),
            "{}",
            &last[half..]
        )
        .unwrap();
        let b = scan(&r, &a.next).unwrap();
        assert_eq!(b.envelopes.len(), 1);
        assert_eq!(a.envelopes[0].source_key, b.envelopes[0].source_key);
        assert!(scan(&r, &b.next).unwrap().envelopes.is_empty());
    }
}
