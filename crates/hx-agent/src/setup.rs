//! Files shared with `hx-server setup` and the bundled setup guide.
use std::path::Path;

/// A request exists and its run has not finished (new, running or interrupted).
pub fn guide_wanted(dir: &Path) -> bool {
    let Some(request) = read(&dir.join("request.json")) else {
        return false;
    };
    match read(&dir.join("state.json")) {
        Some(state) if state["run"] == request["run"] => state["status"] != "finished",
        _ => true,
    }
}

fn read(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
