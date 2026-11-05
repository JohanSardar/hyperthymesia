//! Bounded host commands and a lexical accident guard for one source.

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Component, Path, PathBuf},
    process::{Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{Instant, timeout_at},
};

/// Combined stdout and stderr budget, in bytes. Output is not yet scrubbed;
/// callers must scrub the whole result before storing or exposing it.
pub const OUTPUT_LIMIT: usize = 1024 * 1024;

/// The non-secret host context, supplied explicitly so tests need no real home.
#[derive(Clone, Debug)]
pub struct HostRunner {
    pub home: PathBuf,
    pub work_dir: PathBuf,
    pub state_dir: PathBuf,
    pub path: OsString,
    pub user: OsString,
    pub lang: OsString,
    pub tmp_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("command blocked: {0}")]
    Blocked(&'static str),
    #[error("host command deadline exceeded")]
    Deadline,
    #[error("host command output exceeds 1 MB")]
    OutputLimit,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl HostRunner {
    /// Convenience for production callers. Only these host basics are sampled;
    /// source credentials must be supplied separately to each `run` invocation.
    pub fn from_env(state_dir: PathBuf, work_dir: PathBuf) -> Self {
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            work_dir,
            state_dir,
            path: std::env::var_os("PATH").unwrap_or_default(),
            user: std::env::var_os("USER").unwrap_or_default(),
            lang: std::env::var_os("LANG").unwrap_or_else(|| "en_US.UTF-8".into()),
            tmp_dir: std::env::temp_dir(),
        }
    }

    /// A conservative guard against common accidents, NOT a security boundary.
    /// Full host bash can bypass it, for example `tool=rm; $tool -rf /`.
    /// We intentionally do not evaluate shell expansions, follow symlinks, or
    /// inspect scripts. The owner-facing UI must disclose this limitation.
    pub fn block_reason(&self, command: &str) -> Option<&'static str> {
        let compact: String = command.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some((prefix, body)) = compact.split_once("(){") {
            let name = prefix.rsplit([';', '&', '|']).next().unwrap_or(prefix);
            if !name.is_empty() && body.contains(&format!("{name}|{name}&")) {
                return Some("fork bomb");
            }
        }
        let segments = shell_segments(command);
        for (index, segment) in segments.iter().enumerate() {
            let Ok(words) = shell_words::split(segment) else {
                return Some("unparseable shell command");
            };
            // Reject direct references to the state folder, including read and
            // redirection targets. This is intentionally lexical, not a sandbox.
            for word in &words {
                let value = word.split_once('=').map_or(word.as_str(), |(_, v)| v);
                if self
                    .resolve_path(value)
                    .starts_with(self.absolute_path(&self.state_dir))
                {
                    return Some("Hyperthymesia state folder");
                }
            }
            let mut args = words.as_slice();
            while let Some(word) = args.first() {
                if word.contains('=')
                    || ["command", "exec", "sudo", "env"].contains(&word.as_str())
                    || word.starts_with('-')
                {
                    args = &args[1..];
                } else {
                    break;
                }
            }
            let Some(program) = args
                .first()
                .and_then(|s| Path::new(s).file_name())
                .and_then(|s| s.to_str())
            else {
                continue;
            };
            let args = &args[1..];
            if program == "mkfs" || program.starts_with("mkfs.") {
                return Some("format filesystem");
            }
            if ["shutdown", "reboot", "halt", "poweroff"].contains(&program) {
                return Some("shutdown");
            }
            if program == "diskutil" && args.first().is_some_and(|s| s.starts_with("erase")) {
                return Some("erase disk");
            }
            if program == "dd" {
                let output_arg = args.iter().any(|s| {
                    s.strip_prefix("of=")
                        .is_some_and(|p| self.resolve_path(p).starts_with("/dev"))
                });
                let redirects = segments[index + 1..]
                    .split(|s| matches!(*s, ";" | "&" | "|" | "\n"))
                    .next()
                    .unwrap_or_default();
                let output_redirect = redirects.windows(2).any(|pair| {
                    pair[0] == ">"
                        && shell_words::split(pair[1])
                            .ok()
                            .and_then(|words| words.into_iter().next())
                            .is_some_and(|p| self.resolve_path(&p).starts_with("/dev"))
                });
                if output_arg || output_redirect {
                    return Some("write device");
                }
            }
            if program == "rm" {
                let flags = args
                    .iter()
                    .take_while(|s| s.as_str() != "--")
                    .filter(|s| s.starts_with('-'))
                    .collect::<Vec<_>>();
                let recursive = flags.iter().any(|s| {
                    s.as_str() == "--recursive" || (!s.starts_with("--") && s.contains(['r', 'R']))
                });
                let force = flags
                    .iter()
                    .any(|s| s.as_str() == "--force" || (!s.starts_with("--") && s.contains('f')));
                if recursive
                    && force
                    && args.iter().filter(|s| !s.starts_with('-')).any(|s| {
                        let path = self.resolve_path(s.trim_end_matches('*'));
                        path == Path::new("/") || path == self.absolute_path(&self.home)
                    })
                {
                    return Some("recursive removal of root or home");
                }
            }
        }
        None
    }

    fn resolve_path(&self, value: &str) -> PathBuf {
        let value = ["${HOME}", "$HOME", "~"]
            .iter()
            .find_map(|prefix| {
                value
                    .strip_prefix(prefix)
                    .filter(|rest| rest.is_empty() || rest.starts_with('/'))
                    .map(|rest| self.home.join(rest.trim_start_matches('/')))
            })
            .unwrap_or_else(|| PathBuf::from(value));
        self.absolute_path(&value)
    }

    fn absolute_path(&self, value: &Path) -> PathBuf {
        let absolute = if value.is_absolute() {
            value.to_path_buf()
        } else {
            self.work_dir.join(value)
        };
        let mut result = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::ParentDir => {
                    result.pop();
                }
                Component::CurDir => {}
                part => result.push(part.as_os_str()),
            }
        }
        result
    }

    fn extended_path(&self) -> Result<OsString, HostError> {
        let mut paths = vec![self.home.join(".local/bin"), self.home.join(".cargo/bin")];
        paths.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
            ]
            .map(PathBuf::from),
        );
        for path in std::env::split_paths(&self.path).filter(|p| !p.as_os_str().is_empty()) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        std::env::join_paths(paths)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e).into())
    }

    /// Run one source's bash command. A nonzero exit is returned normally.
    /// Timeout, output overflow, I/O failure and cancellation all kill the group.
    /// A killed server process itself cannot run Drop; this is not an OS sandbox.
    pub async fn run(
        &self,
        command: &str,
        source_env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<Output, HostError> {
        if let Some(reason) = self.block_reason(command) {
            return Err(HostError::Blocked(reason));
        }
        let deadline = Instant::now() + timeout;
        let mut command_builder = Command::new("/bin/bash");
        command_builder
            .args(["--noprofile", "--norc", "-c", command])
            .current_dir(&self.work_dir)
            .env_clear()
            .envs(source_env.iter().filter(|(key, _)| {
                !key.starts_with("HX_") && !key.starts_with("FX_") && key.as_str() != "DATABASE_URL"
            }))
            .env("HOME", &self.home)
            .env("PATH", self.extended_path()?)
            .env("USER", &self.user)
            .env("LANG", &self.lang)
            .env("TMPDIR", &self.tmp_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = command_builder.spawn()?;
        // Created before the next await: aborting the caller can never leave a
        // successfully spawned process without a group guard. Tokio reaps the
        // direct child on drop; kill_on_drop alone would miss its grandchildren.
        let group = ProcessGroup(Pid::from_raw(child.id().expect("spawned child") as i32));
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let budget = AtomicUsize::new(0);
        let result = tokio::try_join!(
            async {
                timeout_at(deadline, child.wait())
                    .await
                    .map_err(|_| HostError::Deadline)?
                    .map_err(HostError::Io)
            },
            read_pipe(stdout, deadline, &budget),
            read_pipe(stderr, deadline, &budget),
        );
        // Even successful shells must not leave background work behind. Each
        // pipe has its own deadline: shell exit is not evidence of pipe EOF.
        drop(group);
        let (status, stdout, stderr) = result?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

struct ProcessGroup(Pid);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // ESRCH is normal when all members have already exited.
        let _ = killpg(self.0, Signal::SIGKILL);
    }
}

async fn read_pipe(
    mut pipe: impl AsyncRead + Unpin,
    deadline: Instant,
    budget: &AtomicUsize,
) -> Result<Vec<u8>, HostError> {
    timeout_at(deadline, async {
        let mut output = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            let n = pipe.read(&mut chunk).await?;
            if n == 0 {
                return Ok(output);
            }
            if budget.fetch_add(n, Ordering::Relaxed) + n > OUTPUT_LIMIT {
                return Err(HostError::OutputLimit);
            }
            output.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .map_err(|_| HostError::Deadline)?
}

/// Split shell operators only outside quotes. Word quoting is then handled by
/// shell-words. This recognizes common direct commands without evaluating bash.
fn shell_segments(command: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, c) in command.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() {
            if c == '\'' || c == '"' {
                quote = Some(c);
            } else if matches!(c, ';' | '&' | '|' | '\n' | '<' | '>') {
                parts.push(&command[start..index]);
                parts.push(&command[index..index + c.len_utf8()]);
                start = index + c.len_utf8();
            }
        }
    }
    parts.push(&command[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::{sys::signal::kill, unistd::Pid};
    use std::time::Instant;

    fn fixture() -> (tempfile::TempDir, HostRunner) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let work_dir = dir.path().join("work");
        let state_dir = dir.path().join("state folder");
        let tmp_dir = dir.path().join("tmp");
        for path in [&home, &work_dir, &state_dir, &tmp_dir] {
            std::fs::create_dir(path).unwrap();
        }
        let runner = HostRunner {
            home,
            work_dir,
            state_dir,
            tmp_dir,
            path: "/usr/bin:/bin".into(),
            user: "synthetic-owner".into(),
            lang: "C".into(),
        };
        (dir, runner)
    }

    #[tokio::test]
    async fn environment_is_only_host_basics_and_this_sources_vars() {
        let (_dir, runner) = fixture();
        let source = BTreeMap::from([
            ("SOURCE_TOKEN".into(), "planted-only-for-this-source".into()),
            ("HX_SECRET".into(), "must-not-leak".into()),
            ("FX_TOKEN".into(), "must-not-leak".into()),
            ("DATABASE_URL".into(), "must-not-leak".into()),
            ("HOME".into(), "/must-not-override-home".into()),
        ]);
        let output = runner
            .run("/usr/bin/env", &source, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let vars: BTreeMap<_, _> = text
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        assert_eq!(vars.get("HOME"), Some(&runner.home.to_str().unwrap()));
        assert_eq!(vars.get("USER"), Some(&"synthetic-owner"));
        assert_eq!(vars.get("LANG"), Some(&"C"));
        assert_eq!(vars.get("TMPDIR"), Some(&runner.tmp_dir.to_str().unwrap()));
        assert_eq!(
            vars.get("SOURCE_TOKEN"),
            Some(&"planted-only-for-this-source")
        );
        for key in vars.keys() {
            assert!(
                [
                    "HOME",
                    "PATH",
                    "USER",
                    "LANG",
                    "TMPDIR",
                    "SOURCE_TOKEN",
                    "PWD",
                    "SHLVL",
                    "_"
                ]
                .contains(key),
                "unexpected environment key: {key}"
            );
        }
        let path = std::env::split_paths(vars["PATH"]).collect::<Vec<_>>();
        assert!(path.contains(&runner.home.join(".local/bin")));
        assert!(path.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(path.contains(&PathBuf::from("/usr/bin")));
    }

    #[tokio::test]
    async fn grandchild_holding_stdout_dies_within_deadline_plus_two_seconds() {
        for command in [
            "sleep 4 & echo $! > grandchild.pid; wait",
            "sleep 4 & echo $! > grandchild.pid; exit 0",
        ] {
            let (_dir, runner) = fixture();
            let deadline = Duration::from_millis(300);
            let start = Instant::now();
            let outcome = runner.run(command, &BTreeMap::new(), deadline).await;
            assert!(matches!(outcome, Err(HostError::Deadline)), "{outcome:?}");
            let pid = read_pid(&runner, "grandchild.pid");
            assert_process_exits(pid).await;
            assert!(start.elapsed() <= deadline + Duration::from_secs(2));
        }
    }

    #[tokio::test]
    async fn aborting_the_caller_kills_the_process_group() {
        let (_dir, runner) = fixture();
        let copy = runner.clone();
        let task = tokio::spawn(async move {
            copy.run(
                "sleep 4 & echo $! > grandchild.pid; wait",
                &BTreeMap::new(),
                Duration::from_secs(10),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !std::fs::read_to_string(runner.work_dir.join("grandchild.pid"))
                .is_ok_and(|text| text.trim().parse::<i32>().is_ok())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid = read_pid(&runner, "grandchild.pid");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_process_exits(pid).await;
    }

    fn read_pid(runner: &HostRunner, name: &str) -> Pid {
        Pid::from_raw(
            std::fs::read_to_string(runner.work_dir.join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
        )
    }

    async fn assert_process_exits(pid: Pid) {
        tokio::time::timeout(Duration::from_millis(800), async {
            while kill(pid, None).is_ok() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the command's grandchild is still alive");
    }

    #[tokio::test]
    async fn combined_stdout_and_stderr_are_capped_at_one_megabyte() {
        let (_dir, runner) = fixture();
        for command in [
            "head -c 1048577 /dev/zero",
            "head -c 600000 /dev/zero; head -c 600000 /dev/zero >&2",
        ] {
            let result = runner
                .run(command, &BTreeMap::new(), Duration::from_secs(2))
                .await;
            assert!(
                matches!(result, Err(HostError::OutputLimit)),
                "expected output limit"
            );
        }
        let output = runner
            .run(
                "printf stdout; printf stderr >&2; exit 7",
                &BTreeMap::new(),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"stderr");
    }

    #[test]
    fn accident_block_table_and_known_bypass() {
        let (_dir, runner) = fixture();
        // Decisions only: none of these command strings are ever executed.
        let blocked = [
            "rm -rf /".to_owned(),
            "rm -fr -- /".into(),
            "/bin/rm -r -f /".into(),
            "rm --recursive --force /".into(),
            "rm -rf ~".into(),
            "rm -rf \"$HOME\"".into(),
            "rm -rf ${HOME}/".into(),
            format!("rm -rf '{}'", runner.home.display()),
            "echo ok; mkfs.ext4 /dev/example".into(),
            "mkfs /dev/example".into(),
            "dd if=input of=/dev/example".into(),
            "dd if=input >/dev/example".into(),
            "diskutil eraseDisk APFS Empty /dev/example".into(),
            ":(){ :|:& };:".into(),
            "shutdown -h now".into(),
            "reboot".into(),
            "halt".into(),
            "poweroff".into(),
            format!("cat '{}/core.sqlite'", runner.state_dir.display()),
            format!("rm -rf '{}'", runner.state_dir.display()),
            "cat ../'state folder'/core.sqlite".into(),
        ];
        for command in blocked {
            assert!(
                runner.block_reason(&command).is_some(),
                "should block: {command}"
            );
        }
        for command in [
            "printf hello",
            "rm -rf ./scratch",
            "dd if=/dev/zero of=./scratch count=1",
            "diskutil list",
            "echo shutdown",
        ] {
            assert!(
                runner.block_reason(command).is_none(),
                "should allow: {command}"
            );
        }
        // Deliberate, decision-only bypass: indirect shell expansion defeats the
        // accident guard. It is not a sandbox or a security boundary.
        assert!(runner.block_reason("tool=rm; $tool -rf /").is_none());
    }
}
