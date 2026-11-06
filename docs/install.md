# Install Hyperthymesia

The installer starts a loopback user service without sudo. Supported: Apple
Silicon macOS and Linux x86_64/aarch64 (glibc 2.35+). Intel Macs are refused.
State must live on a local disk; SQLite WAL is unsuitable for network storage.

The first public tag requires the owner's approval. Until then, extract the
`hyperthymesia-release` artifact from a successful PR or `workflow_dispatch`
release workflow and set `HX_RELEASE_BASE=file:///absolute/artifact/directory`.
Testing needs no tag or release. After an owner-approved release exists:

```sh
curl -fsSL https://github.com/JohanSardar/hyperthymesia/releases/latest/download/install.sh |
  sh -s -- --provider codex
```

Options: `--provider codex|grok|gateway`, `--model M`, `--effort E`,
`--state-dir /absolute/local/path`, `--server-only`, `--app-only`, `--dry-run`,
`--uninstall`, `--start-fresh`. Defaults explicitly select **fx / codex / gpt-6.1-sol / xhigh**.
Grok/gateway users should select their supported model. Real logins beyond
Codex still need owner validation; see [unknowns](specs/unknowns.md).

Dry runs verify downloads and list files/service contents without initializing
state or contacting service managers. Release archives use SHA256SUMS; private
fx 0.0.16 uses hashes pinned in the script. Your own fx is kept.

## State and service

Versions live at `~/.local/share/hyperthymesia/<version>` with a `current`
symlink and `~/.local/bin/hx-server`. Add `~/.local/bin` to PATH. macOS also
installs `~/Applications/Hyperthymesia.app` unless `--server-only` is supplied.
The installer never opens the app or requests capture permissions.
`--app-only` cannot switch a managed server to a different version; use the
full installer for that upgrade.

The single active state folder is:

- macOS: `~/Library/Application Support/Hyperthymesia/server`
- Linux: `~/.local/state/hyperthymesia`
- Or `--state-dir`, remembered on reruns and never silently changed. Choose a
  normalized path outside (and not enclosing) the program prefix.

State is 0700; `core.sqlite`, `agent.sqlite`, `server.env` and `server.log`
are 0600. Blobs and locks stay inside it. `Store::create` enforces privacy
independently of umask. Automatic GC stays **off**; read
[GC safety](gc-safety.md) before running `hx-server gc`.

macOS uses the LaunchAgent `com.hyperthymesia.server` in `gui/$(id -u)`.
A desktop login is required; it restarts at login, not boot. Linux uses
`systemctl --user` and `hyperthymesia.service`, with `Restart=always`.
A working user bus and linger are required. If missing, installation fails
before destination changes and prints `loginctl enable-linger USER` for an
administrator. It never runs that command or sudo. After provisioning, log in
again and rerun. Service stops use SIGTERM; crashes restart automatically.
The wrapper loads `server.env`, logs inside state, and executes
`hx-server --state-dir "$STATE" serve --listen 127.0.0.1:7377`.

## First run

Installation migrates, starts the service, waits for `/healthz`, and runs doctor.
Missing credentials leave the service running with an explicit setup message;
doctor fails until configured. Missing logins defer jobs without spending an
attempt. Installation never invokes login, pairing, interpretation or a paid
model call. Background work follows existing admission and paid sweep rules.

Run the printed private-fx command for Codex/Grok:

```sh
FX_AUTO_UPGRADE=0 FX_DISABLE_KEYCHAIN=1 \
  "$HOME/.local/share/hyperthymesia/tools/fx-0.0.16/fx" login codex
```

Doctor requires a 0600 login file with one hard link. The login and its parents
below HOME must not be symlinks; a symlinked HOME or HOME ancestor is allowed.
Defaults: `~/.fx/chatgpt-auth.json` or `~/.fx/grok-auth.json`. A login delayed
past window end +24h leaves old windows on the one-window-per-day paid sweep.

For gateway, supply `AI_GATEWAY_API_KEY` in the installer's environment or edit
it privately in `server.env`. Never paste that file into logs/issues. The
installer never prints it and **keeps the whole file unchanged on rerun**,
including custom model/credential settings. Passing provider/model/effort
again does not replace it. Restart the service after environment edits.

Load the environment before provider-dependent CLI commands; choose the state
path above:

```sh
STATE="$HOME/.local/state/hyperthymesia"
set -a
. "$STATE/server.env"
set +a
hx-server --state-dir "$STATE" doctor
hx-server --state-dir "$STATE" status
```

`status` prints JSON with `running_interpret_leases` and `retained` copies
(path and logical bytes, or a per-copy read error). This includes pre-upgrade
backups and sibling `server.failed-*` folders; sizes include both databases and
blobs, so repeated failed attempts can use substantial disk space. Nothing is
automatically pruned. `--require-idle` fails for
a live lease. It never migrates, revives jobs or calls a provider.
`doctor --state-only` checks privacy, exact schema and both SQLite databases
without requiring login. Doctor without state retains provider-only checks.

Next: `hx-server --state-dir "$STATE" pair --url URL --name DEVICE`, then
`hx-agent pair '<link>'` or the app's pairing action. Installation prints the
next step and never claims to have paired a device. Open the app and grant
permissions.

For remote access use `tailscale serve --bg 7377` and its HTTPS URL, or maintain
`ssh -N -L 7377:127.0.0.1:7377 user@host` with `http://127.0.0.1:7377`.
The server itself always remains on loopback.

Create a dashboard token yourself; it is returned once, so store it privately:

```sh
hx-server --state-dir "$STATE" token create --name dashboard \
  --prefix macos-agent:DEVICE --prefix macos-media:DEVICE --scopes read_l3
```

Dashboard tokens are prefix-bound. Add `--prefix` for every git, shell or other
connector namespace you intend to see; otherwise its revisions/citations stay
hidden. Pairing grants ingestion, not read access. Installation never prints a
dashboard secret into unattended logs.

## Upgrade and recovery

Rerun with the same state directory. All assets are verified before destination
changes. Unmanaged owner paths cause **exit 3**: legacy `server-data`,
`agent.json`, an unmanaged plist/unit, binary, app, prefix or state directory.
Legacy paths are checked by existence only, never read. There is no override
or automatic mover; see the [legacy mover and its limits](../crates/hx-move/README.md).

An upgrade:

1. Checks state privacy, SQLite integrity, environment syntax and schema/checksums
   before stopping anything, and refuses live interpret leases. An exclusive
   guard closes the status/stop race. It waits up to five seconds for short
   worker ticks/requests to drain, checking for live interpret leases throughout;
   a live lease is refused immediately. Persistently busy state is refused.
2. Stops with SIGTERM and runs P5 `hx-server backup` into
   `<prefix>/backups/pre-upgrade.*` before switching or migrating, including
   reinstall after uninstall. The last installed binary remains available.
3. Switches `current`, explicitly migrates, starts and requires `/healthz`,
   state doctor to pass. Provider doctor only warns with the missing setup step;
   missing login never prevents a healthy upgrade. Workers stay idle and other HTTP
   requests return 503 until verification ends; agents retain retryable uploads.
4. Keeps the previous version and backup. A failed step after stop restores
   both databases and blobs, preserves `server.env`, switches back and restarts.
   Failed-generation state stays beside the active directory as `server.failed-*`.

`migrate` applies every missing version, including a lower number arriving later.
It rejects newer/unknown, dirty or changed checksums before writable access.
Missing migrations commit together; failed DDL rolls them back. `serve` never
migrates. For manual migration stop, back up, migrate, then start the service.
Rollback requires restoring a pre-upgrade backup into an absent/empty directory;
pointing an older binary at a newer schema is insufficient. Backups omit
`server.env`; keep that file separately and private.

A registered install whose core disappears refuses before changing anything,
including when a custom volume is unmounted. Reconnect the volume first. Look
for the original state in sibling `server.failed-*` folders and verified copies
in `<prefix>/backups/`; do not ingest into a replacement empty history. Stop the
user service, move any remaining partial state aside, and use a retained binary:

```sh
"$HOME/.local/share/hyperthymesia/VERSION/hx-server" --state-dir "$STATE" \
  restore --from "$HOME/.local/share/hyperthymesia/backups/pre-upgrade.COPY"
```

Restore `server.env` separately from a private saved copy, chmod it 0600, then
rerun the installer. To intentionally discard the active history, move it aside
and use `--start-fresh` with an absent or empty target. This never adopts an
unmanaged installation and never deletes retained evidence. Keep backups and
failed copies until recovery is verified; `status` reports their paths/sizes.

A hard interruption can leave the service stopped; retained binaries, state
and backup remain available for recovery. Diagnose before retrying and keep
backups/failed state until verified. Versions/backups are not pruned automatically.
`--uninstall` stops/disables the managed user service and unlinks entry points;
it never deletes state, credentials, backups or retained versions.

## Verification

`sh scripts/install.test.sh` uses scratch HOME and stubbed service managers for
owner refusal, private config, lease refusal and failed-upgrade recovery. Never
run real installer service steps on the owner's Mac.

`install-smoke` runs for PR/manual dispatch and owner-tagged releases on disposable GitHub-hosted Linux
and macOS runners: real release/pinned-fx verification, real service startup,
`kill -9` restart, login-free rerun under a symlinked HOME parent, fake fx model
isolation, an injected held lease, and upgrade
with identical seeded evidence and served citation. Its layout upgrade uses
`0.1.0` → `v0.1.0` from the same binary. Separate fail-first store tests exercise
out-of-order DDL, including the real `0002_sources.sql` upgrade from `[1,3]`
to `[1,2,3]` with evidence and pairing preserved, failed DDL rollback, and
newer/tampered-schema refusal. Table-rebuilding DDL is tested transactionally
with foreign keys enabled, matching `Store::create`. The smoke does not prove
a schema-changing upgrade on a running real service, cross-version binary
compatibility, real provider login or paid execution. Real reboot, headless login and app permissions
still need owner VPS/second-user checks.
