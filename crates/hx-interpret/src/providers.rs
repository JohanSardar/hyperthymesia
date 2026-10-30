use crate::{InterpretError, Provider};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

fn error(e: impl std::fmt::Display) -> InterpretError {
    InterpretError::Provider(e.to_string())
}

pub struct ClaudeCliProvider {
    pub model: String,
}
impl Provider for ClaudeCliProvider {
    fn name(&self) -> String {
        format!("claude-cli/{}", self.model)
    }
    async fn complete(&self, system: &str, user: &str) -> Result<String, InterpretError> {
        let mut child = tokio::process::Command::new("claude")
            .args([
                "-p",
                "--no-session-persistence",
                "--output-format",
                "json",
                "--model",
                &self.model,
                "--system-prompt",
                system,
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--permission-prompts",
                "none",
            ])
            // `--bare` would require ANTHROPIC_API_KEY and refuse subscription auth.
            // An empty cwd keeps project CLAUDE.md files out of the interpreter context.
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(error)?;
        child
            .stdin
            .take()
            .ok_or_else(|| error("missing stdin"))?
            .write_all(user.as_bytes())
            .await
            .map_err(error)?;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            child.wait_with_output(),
        )
        .await
        .map_err(error)?
        .map_err(error)?;
        if !output.status.success() {
            return Err(error(format!("claude exited {}", output.status)));
        }
        #[derive(Deserialize)]
        struct Response {
            result: Option<String>,
            #[serde(default)]
            is_error: bool,
        }
        let response: Response = serde_json::from_slice(&output.stdout).map_err(error)?;
        if response.is_error {
            return Err(error("claude returned an error"));
        }
        response
            .result
            .ok_or_else(|| error("claude response missing result"))
    }
}

/// Local, explicit fixture replay. Recording never calls a live provider implicitly.
pub struct RecordedProvider {
    root: PathBuf,
    digest: String,
}
impl RecordedProvider {
    pub fn new(root: impl Into<PathBuf>, input_digest: &str) -> Result<Self, InterpretError> {
        if input_digest.len() != 64 || !input_digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error("invalid fixture digest"));
        }
        Ok(Self {
            root: root.into(),
            digest: input_digest.into(),
        })
    }
    pub fn fixtures(input_digest: &str) -> Result<Self, InterpretError> {
        Self::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm"),
            input_digest,
        )
    }
    pub fn record(&self, response: &str) -> Result<(), InterpretError> {
        let value: serde_json::Value = serde_json::from_str(response).map_err(error)?;
        std::fs::create_dir_all(&self.root).map_err(error)?;
        std::fs::write(
            self.root.join(format!("{}.json", self.digest)),
            serde_json::to_vec_pretty(&value).map_err(error)?,
        )
        .map_err(error)
    }
}
impl Provider for RecordedProvider {
    fn name(&self) -> String {
        "recorded/synthetic".into()
    }
    async fn complete(&self, _system: &str, _user: &str) -> Result<String, InterpretError> {
        tokio::fs::read_to_string(self.root.join(format!("{}.json", self.digest)))
            .await
            .map_err(error)
    }
}
