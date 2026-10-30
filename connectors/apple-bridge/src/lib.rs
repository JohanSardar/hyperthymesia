//! Read-only REST snapshots. App-day estimates remain aggregates, never activity spans.
use anyhow::{Context, Result, ensure};
use hx_connector_sdk::{self as sdk, Request, Scan, digest, envelope, string};
use hx_schema::Operation;
use serde_json::{Value, json};
const COLLECTIONS: [&str; 3] = ["usage.app_day", "messages.message", "calendar.event"];
pub fn main() -> Result<()> {
    sdk::run(
        "apple-bridge",
        "http://localhost:7780",
        &["usage.aggregate", "message.event", "calendar.event"],
        scan,
    )
}
pub fn parse(req: &Request, v: &Value) -> Result<Option<hx_schema::Envelope>> {
    let d = &v["data"];
    let collection = v["collection"]
        .as_str()
        .context("bridge record missing collection")?;
    let id = v["id"].as_str().context("bridge record missing id")?;
    let deleted = v["deleted"].as_bool().unwrap_or(false);
    let (kind, payload) = match collection {
        "usage.app_day" => (
            "usage.aggregate",
            json!({"device_label":d["device_id"],"app":d["bundle_id"],"day":d["day"].as_str().unwrap_or_default().split('@').next().unwrap_or_default(),"seconds_estimated":d["seconds_estimated"].as_f64().unwrap_or(0.0).max(0.0).floor() as u64,"note":format!("{}; sessions capped at four hours; aggregate only",d["uncertainty"].as_str().unwrap_or("focus estimates; settings parity unproven; unclosed sessions omitted; remote batches may lag")),"source_day":d["day"],"provenance":d["provenance"],"decoder_version":d["decoder_version"]}),
        ),
        "messages.message" => (
            "message.event",
            json!({"service":d["service"].as_str().unwrap_or("unknown"),"conversation":d["chat_id"].as_str().map(str::to_owned).unwrap_or_else(||d["chat_id"].to_string()),"direction":if d["is_from_me"]==true{"sent"}else{"received"},"text":d["text"]}),
        ),
        "calendar.event" => (
            "calendar.event",
            json!({"title":d["title"].as_str().unwrap_or_default(),"planned":{"start":d["start"],"end":d["end"]},"attendees":[],"location":d["location"]}),
        ),
        _ => return Ok(None),
    };
    let revision = v["version"]
        .as_u64()
        .map(|n| format!("{n:020}"))
        .unwrap_or_else(|| digest(v));
    let mut e = envelope(
        req,
        &format!("{collection}:{id}"),
        &revision,
        kind,
        payload,
        sdk::timestamp(&d["sent_at"]).or_else(|| sdk::timestamp(&d["start"])),
    );
    if deleted {
        e.operation = Operation::Delete;
        e.payload = json!({"collection":collection,"tombstone_reason":v["tombstone_reason"]});
    }
    if kind == "calendar.event"
        && !deleted
        && (sdk::timestamp(&d["start"]).is_none() || sdk::timestamp(&d["end"]).is_none())
    {
        e.payload
            .as_object_mut()
            .expect("object payload")
            .remove("planned");
        e.payload["planned_local"] = json!({"start":d["start"],"end":d["end"],"time_zone":d["time_zone"],"all_day":d["all_day"]});
        e.payload["timing_unknown"] =
            json!("floating civil dates cannot establish an absolute planned interval");
        e.time.source_start = None;
    }
    e.payload["native"] = v.clone();
    if kind == "usage.aggregate" && !deleted {
        // A civil-day rollup has no source point or span. Keep the source's zone in source_day.
        e.time.source_start = None;
        e.time.resolution_ms = 86_400_000;
        e.payload["device_label"] = json!(
            d["device_id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| d["device_id"].to_string())
        );
    }
    Ok(Some(e))
}
pub fn scan(req: &Request, state: &Value) -> Result<Scan> {
    match online(req, state) {
        Ok(out) => Ok(out),
        Err(error) => {
            let reason = if let Some(http) = error.downcast_ref::<sdk::reqwest::Error>() {
                if http.is_connect() {
                    "Apple Bridge connection unavailable; bridge may not be running".to_owned()
                } else if http.is_timeout() {
                    "Apple Bridge request timed out".to_owned()
                } else {
                    "Apple Bridge returned an unreadable response".to_owned()
                }
            } else if error.to_string().starts_with("Apple Bridge HTTP ") {
                error.to_string()
            } else {
                "Apple Bridge returned an unsupported record or pagination response".to_owned()
            };
            Ok(Scan::unknown(state.clone(), &reason))
        }
    }
}
fn online(req: &Request, state: &Value) -> Result<Scan> {
    let client = sdk::client()?;
    let mut out = Scan::new(state.clone());
    let mut seen = state["seen"].as_object().cloned().unwrap_or_default();
    let mut index = state["collection"].as_u64().unwrap_or(0) as usize;
    let mut after = string(state, "after");
    while index < COLLECTIONS.len() {
        if out.envelopes.len() >= req.limit {
            out.complete = false;
            break;
        }
        let mut request=client.post(format!("{}/v1/op/records.query",req.source.trim_end_matches('/'))).json(&json!({"collection":COLLECTIONS[index],"include_deleted":true,"limit":(req.limit-out.envelopes.len()).min(1000),"after":after}));
        if let Some(token) = &req.apple_token {
            request = request.bearer_auth(token);
        }
        let response = request.send()?;
        ensure!(
            response.status().is_success(),
            "Apple Bridge HTTP {}",
            response.status().as_u16()
        );
        let page: Value = response.json()?;
        let records = page["records"]
            .as_array()
            .context("bridge response missing records")?;
        for record in records {
            if let Some(e) = parse(req, record)? {
                let key = e.source_key.as_str().to_owned();
                let revision = e.revision.as_str();
                if seen.get(&key).and_then(Value::as_str) != Some(revision)
                    && sdk::after_since(req, &e)
                {
                    out.push(e.clone());
                    seen.insert(key, json!(revision));
                }
            }
        }
        let next = string(&page, "next");
        ensure!(
            next.is_empty() || next != after,
            "bridge pagination did not advance"
        );
        if next.is_empty() {
            index += 1;
            after.clear();
        } else {
            after = next;
        }
    }
    if index == COLLECTIONS.len() {
        index = 0;
        after.clear();
    }
    out.next = json!({"collection":index,"after":after,"seen":seen});
    // API snapshot completeness does not establish full upstream Apple coverage.
    out.coverage = hx_schema::Completeness::Partial;
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
        include_str!("../../../fixtures/sources/apple-bridge/input.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    #[test]
    fn fixtures_validate_replay_revisions_and_aggregate_time() {
        let run = || {
            fixture()
                .iter()
                .map(|v| parse(&req(), v).unwrap().unwrap())
                .collect::<Vec<_>>()
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), 5);
        for (x, y) in a.iter().zip(&b) {
            x.validate().unwrap();
            assert_eq!(x.dedup_key(), y.dedup_key());
            if x.operation != Operation::Delete {
                x.core_payload().unwrap().unwrap();
            }
        }
        assert_eq!(a[0].source_key, a[1].source_key);
        assert!(a[0].revision < a[1].revision);
        assert!(a[0].time.source_start.is_none());
        assert!(a[0].time.source_end.is_none());
        assert_eq!(a[4].operation, Operation::Delete);
    }
    #[test]
    fn offline_is_unknown_and_does_not_change_cursor() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut r = req();
        r.source = format!("http://{}", socket.local_addr().unwrap());
        drop(socket);
        let state = json!({"collection":1,"seen":{}});
        let out = scan(&r, &state).unwrap();
        assert!(!out.complete);
        assert_eq!(out.coverage, hx_schema::Completeness::Unknown);
        assert_eq!(out.next, state);
        assert!(out.reason.is_some());
    }
    #[test]
    fn floating_calendar_dates_are_preserved_without_inventing_instants() {
        let e=parse(&req(),&json!({"collection":"calendar.event","id":"h_c","version":1,"data":{"title":"h_title","start":"2000-01-01","end":"2000-01-02","all_day":true}})).unwrap().unwrap();
        e.validate().unwrap();
        assert!(e.time.source_start.is_none());
        assert!(e.payload.get("planned").is_none());
        assert_eq!(e.payload["planned_local"]["start"], "2000-01-01");
    }
    #[test]
    fn pagination_resumes_and_imports_new_versions_of_the_same_record() {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut r = req();
        r.source = format!("http://{}", listener.local_addr().unwrap());
        r.limit = 1;
        let rows = fixture();
        let server = std::thread::spawn(move || {
            for (i, row) in rows[..2].iter().enumerate() {
                let (mut socket, _) = listener.accept().unwrap();
                let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
                let mut len = 0;
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                assert!(first.starts_with("POST /v1/op/records.query "));
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if line.to_lowercase().starts_with("content-length:") {
                        len = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let query: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(query["collection"], "usage.app_day");
                assert_eq!(query["include_deleted"], true);
                assert_eq!(query["after"], if i == 0 { "" } else { "h_page2" });
                let body =
                    json!({"records":[row],"next":if i==0 {"h_page2"} else {""}}).to_string();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let a = scan(&r, &Value::Null).unwrap();
        assert_eq!(a.envelopes.len(), 1);
        assert!(!a.complete);
        assert_eq!(a.next["after"], "h_page2");
        let b = scan(&r, &a.next).unwrap();
        assert_eq!(b.envelopes.len(), 1);
        assert_eq!(a.envelopes[0].source_key, b.envelopes[0].source_key);
        assert!(a.envelopes[0].revision < b.envelopes[0].revision);
        server.join().unwrap();
    }
}
