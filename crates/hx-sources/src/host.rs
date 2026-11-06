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
        self.block_at(command, &self.work_dir, 0)
    }

    fn block_at(&self, source: &str, cwd: &Path, depth: usize) -> Option<&'static str> {
        if depth > 16 {
            return None;
        }
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .ok()?;
        let tree = parser.parse(source, None)?;
        // Bash may execute complete commands before a later syntax error.
        // Inspect the recovered tree without rejecting syntax errors themselves.
        self.inspect_node(tree.root_node(), source, &mut cwd.to_path_buf(), depth)
    }

    fn inspect_node(
        &self,
        node: tree_sitter::Node<'_>,
        source: &str,
        cwd: &mut PathBuf,
        depth: usize,
    ) -> Option<&'static str> {
        // Heredoc contents are data, including text that looks like commands.
        if matches!(
            node.kind(),
            "heredoc_body" | "heredoc_start" | "heredoc_end" | "comment"
        ) {
            return None;
        }
        if node.kind() == "function_definition" {
            let name = node
                .child_by_field_name("name")?
                .utf8_text(source.as_bytes())
                .ok()?;
            let body = node
                .child_by_field_name("body")?
                .utf8_text(source.as_bytes())
                .ok()?;
            let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.contains(&format!("{name}|{name}&")) {
                return Some("fork bomb");
            }
        }
        if node.kind() == "file_redirect" {
            let destination = node.child_by_field_name("destination")?;
            let path = self.resolve_path_at(&shell_word(destination, source), cwd);
            if path.starts_with(self.absolute_path(&self.state_dir)) {
                return Some("Hyperthymesia state folder");
            }
            let writes = node
                .children(&mut node.walk())
                .any(|child| matches!(child.kind(), ">" | ">>" | "&>" | "&>>" | ">|"));
            let owner = node.parent().and_then(|parent| {
                if parent.kind() == "command" {
                    Some(parent)
                } else {
                    parent
                        .child_by_field_name("body")
                        .filter(|body| body.kind() == "command")
                }
            });
            let dd_redirect = owner.is_some_and(|owner| {
                let words = command_words(owner, source);
                unwrap_command(&words)
                    .first()
                    .is_some_and(|name| program_name(name) == "dd")
            }) && path.starts_with("/dev")
                && !is_output_device(&path);
            if writes && (is_disk(&path) || dd_redirect) {
                return Some("write device");
            }
        }
        if node.kind() == "variable_assignment"
            && let Some(value) = node.child_by_field_name("value")
        {
            let value = shell_word(value, source);
            if self
                .resolve_path_at(&value, cwd)
                .starts_with(self.absolute_path(&self.state_dir))
            {
                return Some("Hyperthymesia state folder");
            }
        }
        if node.kind() == "command" {
            let words = command_words(node, source);
            for word in &words {
                let value = word
                    .split_once('=')
                    .map_or(word.as_str(), |(_, value)| value);
                if self
                    .resolve_path_at(value, cwd)
                    .starts_with(self.absolute_path(&self.state_dir))
                {
                    return Some("Hyperthymesia state folder");
                }
            }
            if let Some(reason) = self.inspect_command(&words, cwd, depth) {
                return Some(reason);
            }
        }
        // A cwd change in a subshell, pipeline or function must not leak into
        // subsequent commands in its parent shell.
        if matches!(
            node.kind(),
            "subshell"
                | "command_substitution"
                | "process_substitution"
                | "function_definition"
                | "pipeline"
        ) {
            let mut local = cwd.clone();
            for child in node.named_children(&mut node.walk()) {
                let mut child_cwd = local.clone();
                let target = if node.kind() == "pipeline" {
                    &mut child_cwd
                } else {
                    &mut local
                };
                if let Some(reason) = self.inspect_node(child, source, target, depth) {
                    return Some(reason);
                }
            }
        } else {
            for child in node.named_children(&mut node.walk()) {
                if let Some(reason) = self.inspect_node(child, source, cwd, depth) {
                    return Some(reason);
                }
            }
        }
        None
    }

    fn inspect_command(
        &self,
        words: &[String],
        cwd: &mut PathBuf,
        depth: usize,
    ) -> Option<&'static str> {
        let words = unwrap_command(words);
        let (program, args) = words.split_first()?;
        let program = program_name(program);
        if program == "mkfs" || program.starts_with("mkfs.") || program.starts_with("newfs_") {
            return Some("format filesystem");
        }
        if ["shutdown", "reboot", "halt", "poweroff"].contains(&program) {
            return Some("shutdown");
        }
        if program == "diskutil" {
            let args = if args.first().is_some_and(|s| s == "quiet") {
                &args[1..]
            } else {
                args
            };
            if args.first().is_some_and(|s| {
                s.starts_with("erase")
                    || ["secureErase", "reformat", "partitionDisk"].contains(&s.as_str())
            }) || (args.first().is_some_and(|s| s == "apfs")
                && args.get(1).is_some_and(|s| s == "deleteContainer"))
            {
                return Some("erase disk");
            }
        }
        if program == "dd"
            && args.iter().any(|s| {
                s.strip_prefix("of=").is_some_and(|p| {
                    let path = self.resolve_path_at(p, cwd);
                    path.starts_with("/dev") && !is_output_device(&path)
                })
            })
        {
            return Some("write device");
        }
        if program == "rm" {
            let flags: Vec<_> = args
                .iter()
                .take_while(|s| s.as_str() != "--")
                .filter(|s| s.starts_with('-'))
                .collect();
            let recursive = flags.iter().any(|s| {
                s.as_str() == "--recursive" || (!s.starts_with("--") && s.contains(['r', 'R']))
            });
            let force = flags
                .iter()
                .any(|s| s.as_str() == "--force" || (!s.starts_with("--") && s.contains('f')));
            if recursive
                && force
                && args.iter().filter(|s| !s.starts_with('-')).any(|s| {
                    let path = self.resolve_path_at(s.trim_end_matches('*'), cwd);
                    path == Path::new("/") || path == self.absolute_path(&self.home)
                })
            {
                return Some("recursive removal of root or home");
            }
        }
        if ["bash", "sh"].contains(&program) && args.first().is_some_and(|s| s == "-c") {
            return args
                .get(1)
                .and_then(|script| self.block_at(script, cwd, depth + 1));
        }
        if program == "cd" {
            *cwd = args
                .first()
                .map(|s| self.resolve_path_at(s, cwd))
                .unwrap_or_else(|| self.home.clone());
        }
        None
    }

    fn resolve_path_at(&self, value: &str, cwd: &Path) -> PathBuf {
        let value = ["${HOME}", "$HOME", "~"]
            .iter()
            .find_map(|prefix| {
                value
                    .strip_prefix(prefix)
                    .filter(|rest| rest.is_empty() || rest.starts_with('/'))
                    .map(|rest| self.home.join(rest.trim_start_matches('/')))
            })
            .unwrap_or_else(|| PathBuf::from(value));
        self.absolute_path(&cwd.join(value))
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

    pub(crate) fn extended_path(&self) -> Result<OsString, HostError> {
        let mut paths = Vec::new();
        if self.home.is_absolute() {
            paths.extend([self.home.join(".local/bin"), self.home.join(".cargo/bin")]);
        }
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
        for path in std::env::split_paths(&self.path).filter(|p| p.is_absolute()) {
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
    /// A double-forked `setsid` process can outlive the deadline; there is no
    /// simple fix on macOS. The work folder must sit outside the state folder.
    pub async fn run(
        &self,
        command: &str,
        source_env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<Output, HostError> {
        let deadline = Instant::now() + timeout.min(Duration::from_secs(180));
        let mut child = self.command(command, source_env)?.spawn()?;
        // Created before the next await: cancellation cannot leak descendants.
        let group = ProcessGroup::for_child(child.id().expect("spawned child"));
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let budget = AtomicUsize::new(0);
        let pipes_done = tokio::sync::Notify::new();
        let result = tokio::try_join!(
            async {
                let status = timeout_at(deadline, child.wait())
                    .await
                    .map_err(|_| HostError::Deadline)??;
                // Background children can inherit the pipes after bash exits.
                tokio::select! {
                    _ = pipes_done.notified() => {},
                    _ = tokio::time::sleep(Duration::from_millis(100)) => group.kill(),
                }
                Ok::<_, HostError>(status)
            },
            async {
                let output = tokio::try_join!(
                    read_pipe(stdout, deadline, &budget),
                    read_pipe(stderr, deadline, &budget),
                );
                pipes_done.notify_one();
                output
            },
        );
        drop(group);
        let (status, (stdout, stderr)) = result?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    pub(crate) fn command(
        &self,
        command: &str,
        source_env: &BTreeMap<String, String>,
    ) -> Result<Command, HostError> {
        if let Some(reason) = self.block_reason(command) {
            return Err(HostError::Blocked(reason));
        }
        let mut command_builder = Command::new("/bin/bash");
        command_builder
            .args(["--noprofile", "--norc", "-c", command])
            .current_dir(&self.work_dir)
            .env_clear()
            .envs(source_env.iter().filter(|(key, _)| {
                !key.starts_with("HX_")
                    && !key.starts_with("FX_")
                    && !key.starts_with("LD_")
                    && !key.starts_with("DYLD_")
                    && !["DATABASE_URL", "BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS"]
                        .contains(&key.as_str())
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
        Ok(command_builder)
    }
}

pub(crate) struct ProcessGroup(Pid);
impl ProcessGroup {
    pub(crate) fn for_child(id: u32) -> Self {
        Self(Pid::from_raw(id as i32))
    }
    fn kill(&self) {
        // ESRCH is normal when all members have already exited.
        let _ = killpg(self.0, Signal::SIGKILL);
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
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

/// Unquote a single syntax-tree word without evaluating shell expansions.
fn shell_word(node: tree_sitter::Node<'_>, source: &str) -> String {
    let text = &source[node.byte_range()];
    shell_words::split(text)
        .ok()
        .filter(|words| words.len() == 1)
        .and_then(|mut words| words.pop())
        .unwrap_or_else(|| text.to_owned())
}

fn command_words(node: tree_sitter::Node<'_>, source: &str) -> Vec<String> {
    node.children(&mut node.walk())
        .enumerate()
        .filter(|(index, _)| {
            matches!(
                node.field_name_for_child(*index as u32),
                Some("name" | "argument")
            )
        })
        .map(|(_, child)| shell_word(child, source))
        .collect()
}

fn program_name(word: &str) -> &str {
    // Relative executables such as ./halt are ordinary user programs.
    if Path::new(word).is_absolute() {
        Path::new(word)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(word)
    } else {
        word
    }
}

fn unwrap_command(mut words: &[String]) -> &[String] {
    while let Some(word) = words.first() {
        let program = program_name(word);
        if !["command", "exec", "sudo", "env", "time", "xargs"].contains(&program) {
            break;
        }
        words = &words[1..];
        while let Some(option) = words.first() {
            if program == "command" && ["-v", "-V"].contains(&option.as_str()) {
                return &[]; // Lookup only: its arguments are not executed.
            }
            if option == "--" {
                words = &words[1..];
                break;
            }
            let takes_value = match program {
                "sudo" => [
                    "-u", "-g", "-h", "-p", "-C", "-T", "-R", "-D", "--user", "--group",
                ]
                .contains(&option.as_str()),
                "env" => ["-u", "--unset", "-C", "--chdir"].contains(&option.as_str()),
                "xargs" => ["-I", "-n", "-P", "-s", "-d", "-E", "-L"].contains(&option.as_str()),
                _ => false,
            };
            if takes_value {
                words = words.get(2..).unwrap_or_default();
            } else if option.starts_with('-') || (program == "env" && option.contains('=')) {
                words = &words[1..];
            } else {
                break;
            }
        }
    }
    words
}

fn is_disk(path: &Path) -> bool {
    path.strip_prefix("/dev").ok().is_some_and(|rest| {
        rest.to_str()
            .is_some_and(|name| name.starts_with("disk") || name.starts_with("rdisk"))
    })
}

fn is_output_device(path: &Path) -> bool {
    ["/dev/null", "/dev/stdout", "/dev/stderr"]
        .iter()
        .any(|device| path == Path::new(device))
        || path.starts_with("/dev/fd")
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
        let (_dir, runner) = fixture();
        let deadline = Duration::from_millis(300);
        let start = Instant::now();
        let outcome = runner
            .run(
                "sleep 4 & echo $! > grandchild.pid; wait",
                &BTreeMap::new(),
                deadline,
            )
            .await;
        assert!(matches!(outcome, Err(HostError::Deadline)), "{outcome:?}");
        assert_process_exits(read_pid(&runner, "grandchild.pid")).await;
        assert!(start.elapsed() <= deadline + Duration::from_secs(2));
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
    fn ordinary_shell_commands_are_allowed() {
        let (_dir, runner) = fixture();
        // Decision tests only; never execute candidate commands.
        let failures: Vec<_> = [
            "dd if=a of=b 2>/dev/null",
            "dd if=a of=/dev/null",
            "dd if=a of=/dev/stdout",
            "dd if=a of=/dev/stderr",
            "dd if=a of=/dev/fd/1",
            "dd if=a >/dev/null",
            "dd if=a >/dev/stdout",
            "dd if=a >/dev/stderr",
            "dd if=a >/dev/fd/1",
            "cat <<EOF\nit's a note\nEOF",
            "cat <<EOF\nshutdown\nEOF",
            "cat <<'EOF'\n:(){ :|:& };:\nEOF",
            "cat <<-EOF\n\tshutdown\n\tEOF",
            "echo \"$(printf \"it's\")\"",
            "echo hi > reboot",
            "sort < halt",
            "echo x > mkfs.log",
            "command -v shutdown",
            "command -V shutdown",
            "./halt",
            "echo 'unterminated",
        ]
        .into_iter()
        .filter_map(|command| runner.block_reason(command).map(|reason| (command, reason)))
        .collect();
        assert!(
            failures.is_empty(),
            "ordinary commands blocked: {failures:?}"
        );
    }

    #[test]
    fn common_accidents_are_blocked() {
        let (_dir, runner) = fixture();
        // Decision tests only; never execute candidate commands.
        let misses: Vec<_> = [
            "(rm -rf /)",
            "if true; then rm -rf /; fi",
            "bash -c 'rm -rf /'",
            "sudo -u root rm -rf /",
            "time rm -rf /",
            "xargs rm -rf /",
            "cd ~ && rm -rf .",
            "newfs_apfs /dev/disk99",
            "newfs_hfs /dev/disk99",
            "diskutil secureErase 0 /dev/disk99",
            "diskutil reformat /dev/disk99",
            "diskutil partitionDisk /dev/disk99 GPT APFS Empty 100%",
            "diskutil apfs deleteContainer /dev/disk99",
            "diskutil quiet secureErase 0 /dev/disk99",
            "diskutil quiet reformat /dev/disk99",
            "diskutil quiet partitionDisk /dev/disk99 GPT APFS Empty 100%",
            "diskutil quiet apfs deleteContainer /dev/disk99",
            "echo x >/dev/disk99",
            "echo x 2>>/dev/disk99",
            "echo x >/dev/rdisk99",
        ]
        .into_iter()
        .filter(|command| runner.block_reason(command).is_none())
        .collect();
        assert!(misses.is_empty(), "common accidents allowed: {misses:?}");
    }

    #[tokio::test]
    async fn exited_shell_preserves_output_and_cleans_up_background_child() {
        let (_dir, runner) = fixture();
        let start = Instant::now();
        let output = runner
            .run(
                "echo useful; sleep 30 & echo $! > grandchild.pid",
                &BTreeMap::new(),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"useful\n");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_process_exits(read_pid(&runner, "grandchild.pid")).await;
    }

    #[tokio::test]
    async fn path_is_absolute_without_home_and_extreme_timeout_is_safe() {
        let (_dir, mut runner) = fixture();
        runner.home = PathBuf::new(); // Same value as from_env with HOME unset.
        runner.path = ":.:relative/bin:/absolute/bin:/usr/bin".into();
        let path = runner.extended_path().unwrap();
        let relative: Vec<_> = std::env::split_paths(&path)
            .filter(|path| !path.is_absolute())
            .collect();
        let result = tokio::spawn(async move {
            runner
                .run("printf safe", &BTreeMap::new(), Duration::MAX)
                .await
        })
        .await;
        assert!(
            relative.is_empty() && result.is_ok(),
            "relative PATH entries: {relative:?}; extreme timeout: {result:?}"
        );
        let output = result.unwrap().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"safe");
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
