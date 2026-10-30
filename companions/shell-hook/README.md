# Interactive zsh hook

Requires zsh and Python 3 on PATH. Keep `hyperthymesia.zsh` and `emit.py` together.
From the repository root, run `source "$PWD/companions/shell-hook/hyperthymesia.zsh"`
to enable it for the current interactive shell. To install persistently, add a
`source` line pointing to that same file to your `.zshrc`, then start a new shell.
Sourcing twice does not register duplicate hooks. Existing hooks are preserved.

The socket defaults to `$HOME/Library/Application Support/Hyperthymesia/agent.sock`.
`HYPERTHYMESIA_SOCKET` can override it for testing. Each preexec/precmd pair records
one redacted command, cwd at execution, exit code, timestamps and duration.
Only interactive commands are covered: scripts, agent commands and other
noninteractive shells are **not** claimed as observed. A command that never
returns to precmd is not emitted.

The prompt disowns a background process; it does not wait for Python or the
socket. Missing sockets silently drop the observation. Connect/write operations
have a 200 ms socket timeout. There is no persistent shell outbox and no delivery
acknowledgment; a failed agent connection is a coverage gap. Durations use zsh's
wall clock and can be inaccurate across clock steps. No raw command is written
to disk or passed in process argv. Secret-bearing commands are entirely redacted
before transport rather than attempting unsafe shell-token reconstruction.
The denylist covers secret/token/password names, `*_KEY=`, bearer headers, URLs
with credentials and recognizable token prefixes. Arbitrary unnamed secrets
cannot be recognized; do not treat this as a full shell DLP policy. Cwd is sent
as metadata; configure the agent's shared policy for sensitive directories.

From the repository root:

```sh
~/.bun/bin/bun test companions/shell-hook
zsh -n companions/shell-hook/hyperthymesia.zsh
```

Tests use synthetic commands and temporary Unix sockets, including measured
missing-socket latency. They do not install or modify your shell startup files.
