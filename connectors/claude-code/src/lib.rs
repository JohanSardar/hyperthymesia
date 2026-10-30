//! Claude project transcripts and history, including sidechain provenance.
use anyhow::Result;
use hx_connector_sdk::{self as sdk, Request, Scan, agent, digest, link, timestamp};
use serde_json::{Value, json};
use std::path::Path;
pub fn main() -> Result<()> {
    sdk::run("claude-code", "~/.claude", &["agent.message"], scan)
}
pub fn parse(
    req: &Request,
    v: &Value,
    ctx: &mut Value,
    fallback: &str,
) -> Option<hx_schema::Envelope> {
    if let Some(title) = v["aiTitle"].as_str() {
        ctx["title"] = json!(title);
        return None;
    }
    if v["sessionId"].is_string() {
        ctx["session"] = v["sessionId"].clone();
    }
    if v["cwd"].is_string() {
        ctx["cwd"] = v["cwd"].clone();
    }
    let session = ctx["session"].as_str().unwrap_or(fallback);
    let typ = v["type"].as_str().unwrap_or("history");
    let content = if typ == "history" {
        v["display"].clone()
    } else {
        v["message"]["content"].clone()
    };
    let is_tool = content.as_array().is_some_and(|a| {
        a.iter()
            .any(|b| matches!(b["type"].as_str(), Some("tool_result" | "tool_use")))
    });
    let role = match typ {
        "user" if is_tool => "tool",
        "user" | "history" => "user",
        "assistant" if is_tool => "tool",
        "assistant" => "assistant",
        _ => "lifecycle",
    };
    let native = v["uuid"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| digest(v));
    let mut e = agent(
        req,
        &format!("{session}:{native}"),
        &digest(v),
        json!({"provider":"claude-code","thread_id":session,"thread_title":ctx["title"],"turn_id":v["promptId"],"role":role,"text":sdk::text(&content),"cwd":ctx["cwd"],"model":v["message"]["model"],"is_sidechain":v["isSidechain"],"agent_id":v["agentId"],"event_type":typ,"native":v}),
        timestamp(&v["timestamp"]),
    );
    link(
        &mut e,
        "parent",
        v["parentUuid"].as_str().unwrap_or_default(),
    );
    link(&mut e, "session", session);
    // History is an index/provenance record; it cannot establish a second human action.
    if typ == "history" {
        e.payload["claim_scope"] = json!("history_index");
    }
    Some(e)
}
pub fn scan(req: &Request, state: &Value) -> Result<Scan> {
    let root = Path::new(&req.source);
    let mut positions = sdk::tail_state(state)?;
    let mut out = Scan::new(state.clone());
    let mut files = if root.is_file() {
        vec![root.to_path_buf()]
    } else {
        sdk::files(&root.join("projects"), 12, &|p| {
            p.extension().is_some_and(|e| e == "jsonl")
        })?
    };
    if root.join("history.jsonl").exists() {
        files.push(root.join("history.jsonl"));
    }
    if files.is_empty() {
        return Ok(Scan::unknown(
            state.clone(),
            "no Claude transcript files found",
        ));
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
            if let Some(e) = parse(
                req,
                &line.value,
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
        include_str!("../../../fixtures/sources/claude-code/input.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    #[test]
    fn fixtures_validate_replay_sidechain_and_tools() {
        let run = || {
            let mut ctx = json!({});
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
        }
        assert_eq!(a[0].payload["is_sidechain"], true);
        assert_eq!(a[1].payload["role"], "tool");
        assert_eq!(a[0].payload["thread_title"], "h_title");
    }
    #[test]
    fn partial_line_is_not_lost_or_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.jsonl");
        let rows = fixture();
        let first = format!("{}\n{}\n", rows[0], rows[1]);
        let last = rows[2].to_string();
        let half = last.len() / 2;
        std::fs::write(&path, format!("{first}{}", &last[..half])).unwrap();
        let mut r = req();
        r.source = path.to_string_lossy().into();
        let a = scan(&r, &Value::Null).unwrap();
        assert_eq!(a.envelopes.len(), 1);
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new().append(true).open(path).unwrap(),
            "{}",
            &last[half..]
        )
        .unwrap();
        let b = scan(&r, &a.next).unwrap();
        assert_eq!(b.envelopes.len(), 1);
        assert!(scan(&r, &b.next).unwrap().envelopes.is_empty());
    }
}
