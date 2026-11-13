# Hyperthymesia

A searchable memory of your Mac activity and connected sources, with day pages
that cite the original evidence. Connect an API, CLI or MCP server by describing
how to read it.

## Install

```sh
curl -fsSL https://johansardar.github.io/hyperthymesia/install | sh
```

Linux x86_64/arm64 or Apple Silicon macOS 14+. Requires curl, Python 3 and a
working systemd user service (Linux) or desktop login (macOS). No Rust, Docker or
database service is needed. The installer verifies pinned release checksums and
refuses unrelated destinations. See [options and recovery](docs/install.md).

Then setup runs by itself. It pairs the Mac app, opens the dashboard already
signed in, and shows you only what a person has to do: switch on three macOS
permissions and sign in to your model provider, each guided beside the screen
where you do it. "Not now" always works. Run `hx-server setup` again any time; it
only shows what is left. Asking an AI agent to set it up? Point it at
[these instructions](docs/setup-for-agents.md).

**Experimental:** capture can have gaps and model-written pages can be wrong.
Cloud processing can cost money and the agent is not sandboxed.
Read [the safety notes](#before-you-collect-data) before pairing or connecting.

## 30-second check

After installation, verify the real service without an account or paid request:

```sh
set -e
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH";; esac
IFS= read -r HX_STATE_DIR < "$HOME/.local/share/hyperthymesia/.state-dir"
export HX_STATE_DIR
hx-server --version
hx-server doctor --state-only
hx-server status
curl -fsS --retry 10 --retry-connrefused --retry-delay 1 --max-time 2 http://127.0.0.1:7377/healthz
```

Expect the installed version, healthy private state, JSON status and `ok`.
This proves storage/service readiness, not capture or a working model login.
For cloud processing, follow [provider setup](docs/install.md#first-run).

## What it does

- Captures Mac activity with explicit permissions and buffers uploads locally.
- Stores evidence in SQLite on your disk; missing capture is unknown.
- Connects APIs, CLIs and MCP services, with owner-controlled off switches.
- Builds searchable timelines and cloud-model day pages with evidence citations.
- Backs up, upgrades and restores state without silently replacing history.

[Documentation](docs/README.md) · [Architecture](ARCHITECTURE.md) ·
[Contributing and checks](CONTRIBUTING.md) · [Security](SECURITY.md)

Original code and documentation are **AGPL-3.0-only**. See [LICENSE](LICENSE).
Third-party dependencies retain their own licenses.

## Before you collect data

The Mac app can record focused-window context, accessibility text, input activity
and screenshots. The unreleased screenshot-only build removes video capture;
published 0.1.3 and earlier builds also record video. It buffers uploads locally.
The server stores evidence,
saved source answers and day pages on your disk. Your configured **cloud model**
can receive captured window titles, URLs, accessibility text, screenshot OCR,
fuller saved evidence from broker lookups, and source responses during setup and
live lookups. Model use can cost money.

The agent has full bash on the server, with access to files, existing CLI logins,
credentials and the network. It is not sandboxed. Setting `HX_HOST_BASH=off`
removes its bash tool, but commands a source names still run on the server under
your account. Source keys are private files, but **not hidden from the agent**.
“Keep it local” is an instruction, not an isolation boundary. Use only a host and
credentials you are willing to grant that access.

Switching a source off stops its broker requests and automatic pulls, including
running exchanges. It retains keys, logins, settings, agent tables, evidence and
citations. Full host bash can still access it. There is no source removal or
automatic erasure of history/backups. Databases and source keys are **not encrypted
at rest**. [Garbage collection is disabled](docs/gc-safety.md).

## Connect a source

Follow [the source guide](docs/setup.md#connect-a-source). Connecting starts a
paid setup job. Review what the agent configured and check its citations.

## Add the Mac app

The installer places `~/Applications/Hyperthymesia.app` without opening it.
[Pair deliberately and grant permissions](docs/mac-pairing.md) when ready;
the managed server's default URL is `http://127.0.0.1:7377`.
Ad-hoc signing is not Developer ID notarization.

## Install from source

Use the [source build and foreground setup guide](docs/setup.md).
For a login-free development smoke test, see [task guides](docs/guides.md).
