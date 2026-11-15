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

/// Starts the setup guide again for the permissions that are off; the running
/// app picks the request up within a second.
pub fn request_guide(dir: &Path, steps: &[&str]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let request = serde_json::json!({
        "run": format!("menu-{}", ulid::Ulid::generate()),
        "steps": steps,
        "signin_title": null,
    });
    let tmp = dir.join(".request.json.menu");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&request)?)?;
    std::fs::rename(tmp, dir.join("request.json"))?;
    Ok(())
}
