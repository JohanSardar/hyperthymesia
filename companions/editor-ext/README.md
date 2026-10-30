# VS Code / Cursor companion

From this directory:

```sh
~/.bun/bin/bun install --frozen-lockfile
~/.bun/bin/bun run build
~/.bun/bin/bun run typecheck
~/.bun/bin/bun test
```

For development, launch VS Code or Cursor with
`--extensionDevelopmentPath="$PWD"` (from this directory), or open this directory
and use the editor's Extension Development Host. To distribute, package this
folder with the standard VS Code VSIX packager, then use **Extensions: Install
from VSIX** in VS Code/Cursor. `dist/extension.js` must exist before packaging;
it and any VSIX are build artifacts and are not committed.

The UI extension runs on the local machine and connects to
`$HOME/Library/Application Support/Hyperthymesia/agent.sock` with Node's Unix
socket API. No listener is opened. It emits non-core `editor.focus` observations
on file, workspace or window focus changes and `editor.debug` observations for
session start, terminate and active-session changes. Focus payloads include
workspace, file_path, language, git_branch (null if the built-in Git API has no
branch), and focused. Debug payloads add action, session_id and debug_type.
No file contents, terminal contents, debug expressions or launch arguments are
read. Untrusted workspaces are suppressed. Non-file document paths are null;
remote workspaces are not claimed as local filesystem coverage.

Delivery is best effort, with a 250 ms deadline and silent socket errors; no
persistent queue/ack or complete coverage is claimed for editor observations.
Actual VS Code/Cursor UI activation and native-agent delivery have not been
exercised together. Tests cover activation callbacks with mocked VS Code APIs;
the browser companion's contract suite also validates editor event envelopes and
real socket transport against a temporary listener.
