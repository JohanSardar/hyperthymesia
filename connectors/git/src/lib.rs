//! Repository discovery and immutable commits; removed repositories imply no deletions.
use anyhow::{Result, ensure};
use hx_connector_sdk::{self as sdk, Request, Scan, envelope};
use hx_schema::Operation;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};
pub fn main() -> Result<()> {
    sdk::run("git", "~", &["code.commit"], scan)
}
fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    ensure!(out.status.success(), "git read command failed");
    Ok(out.stdout)
}
fn discover(path: &Path, depth: usize, repos: &mut BTreeSet<PathBuf>, complete: &mut bool) {
    if path.join(".git").exists() || (path.join("HEAD").is_file() && path.join("objects").is_dir())
    {
        repos.insert(path.to_path_buf());
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        *complete = false;
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            *complete = false;
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | "Library" | "node_modules" | "target" | ".Trash" | ".cache"
        ) {
            continue;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            discover(&entry.path(), depth - 1, repos, complete);
        }
    }
}
pub fn parse(
    req: &Request,
    commit: &hx_schema::kinds::CodeCommit,
    at: &str,
) -> hx_schema::Envelope {
    let mut e = envelope(
        req,
        &format!(
            "{}:{}",
            hx_schema::sha256_hex(commit.repo.as_bytes()),
            commit.sha
        ),
        &commit.sha,
        "code.commit",
        json!(commit),
        sdk::timestamp(&json!(at)),
    );
    e.operation = Operation::Observe;
    e.time.resolution_ms = 1000;
    e
}
pub fn scan(req: &Request, state: &Value) -> Result<Scan> {
    let mut seen: BTreeSet<String> =
        serde_json::from_value(state["seen"].clone()).unwrap_or_default();
    let mut out = Scan::new(state.clone());
    let mut repos = BTreeSet::new();
    let roots = if req.roots.is_empty() {
        vec![PathBuf::from(&req.source)]
    } else {
        req.roots.clone()
    };
    for root in roots {
        discover(&root, req.depth, &mut repos, &mut out.complete);
    }
    let since = req
        .since
        .map(|s| s.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    'repos: for repo in repos {
        let log = match git(
            &repo,
            &[
                "log",
                "--all",
                "--format=%H%x00%aI%x00%an <%ae>%x00%B%x00",
                "--since",
                &since,
            ],
        ) {
            Ok(v) => v,
            Err(_) => {
                out.complete = false;
                out.reason = Some("one or more repositories could not be read".into());
                continue;
            }
        };
        let log = String::from_utf8_lossy(&log);
        let fields: Vec<_> = log.split('\0').collect();
        for record in fields.as_chunks::<4>().0 {
            let sha = record[0].trim();
            let key = format!(
                "{}:{sha}",
                hx_schema::sha256_hex(repo.to_string_lossy().as_bytes())
            );
            if seen.contains(&key) {
                continue;
            }
            if out.envelopes.len() >= req.limit {
                out.complete = false;
                break 'repos;
            }
            let branches = String::from_utf8(git(
                &repo,
                &[
                    "branch",
                    "-a",
                    "--contains",
                    sha,
                    "--format=%(refname:short)",
                ],
            )?)?;
            let files = git(
                &repo,
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    "-z",
                    sha,
                ],
            )?
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .count() as u32;
            out.push(parse(
                req,
                &hx_schema::kinds::CodeCommit {
                    repo: repo.to_string_lossy().into(),
                    sha: sha.into(),
                    message: record[3].trim_end().into(),
                    author: Some(record[2].into()),
                    branch: branches.lines().next().map(str::to_owned),
                    files_changed: Some(files),
                },
                record[1],
            ));
            seen.insert(key);
        }
    }
    out.next = json!({"seen":seen});
    if !out.complete && out.reason.is_none() {
        out.reason = Some("scan limited or a discovery directory was unreadable".into());
    }
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
        include_str!("../../../fixtures/sources/git/input.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    #[test]
    fn fixture_validates_and_has_stable_commit_identity() {
        let v = &fixture()[0];
        let run = || {
            parse(
                &req(),
                &serde_json::from_value(v.clone()).unwrap(),
                v["at"].as_str().unwrap(),
            )
        };
        let a = run();
        a.validate().unwrap();
        assert_eq!(a.dedup_key(), run().dedup_key());
        assert_eq!(
            a.source_key.as_str().split(':').next_back().unwrap(),
            a.revision.as_str()
        );
    }
    #[test]
    fn discovery_and_checkpoint_do_not_repeat_commits() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(out.status.success());
        };
        run(&["init", "-q"]);
        std::fs::write(dir.path().join("h_file"), "xxxxxxxx").unwrap();
        run(&["add", "h_file"]);
        run(&[
            "-c",
            "user.name=h_author",
            "-c",
            "user.email=h_author@invalid.test",
            "commit",
            "-qm",
            "xxxxxxxx",
        ]);
        let mut r = req();
        r.source = dir.path().to_string_lossy().into();
        let a = scan(&r, &Value::Null).unwrap();
        assert_eq!(a.envelopes.len(), 1);
        assert_eq!(a.envelopes[0].payload["files_changed"], 1);
        assert!(scan(&r, &a.next).unwrap().envelopes.is_empty());
    }
}
