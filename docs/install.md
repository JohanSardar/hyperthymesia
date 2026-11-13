# Install Hyperthymesia

For a source build and foreground server, start with the
[source guide](setup.md). This page describes the
managed-service installer. Both use SQLite and a single local state folder;
neither needs Docker or a database service.

The installer starts a loopback user service without sudo. Supported: Apple
Silicon macOS and Linux x86_64/aarch64 (glibc 2.35+). Intel Macs are refused.
State must live on a local disk; SQLite WAL is unsuitable for network storage.

Requires curl and Python 3. The public launcher pins a release version and the
SHA-256 of its checksum manifest. It verifies that manifest, then the lifecycle
installer, before executing downloaded code:

```sh
curl -fsSL https://johansardar.github.io/hyperthymesia/install | sh
```

To forward options, replace `| sh` with, for example,
`| sh -s -- --provider grok --model YOUR_MODEL`. Never substitute a mutable
latest-release installer for the reviewed pin. For workflow-artifact testing,
run its local install.sh with `HX_RELEASE_BASE=file:///absolute/artifact/directory`;
that developer path is not the independent public pin or clean-machine proof.

Options: `--provider vercel|codex|grok`, `--model M`, `--effort E`,
`--state-dir /absolute/local/path`, `--server-only`, `--app-only`, `--dry-run`,
`--uninstall`, `--start-fresh`. No provider, model or effort is preselected.
Choose them in the app, or pass both provider and model explicitly. Effort is
optional and depends on the model. See [provider limitations](provider-limitations.md).

Dry runs verify downloads and list files/service contents without executing
downloaded binaries, initializing state or contacting service managers. Release
archives use SHA256SUMS from the same GitHub release. The public launcher's
separate reviewed manifest pin binds all platforms and install.sh; it is not a
publisher signature or OS sandbox. Private fx 0.0.16 uses hashes pinned in
the script. Your own fx is kept. A normal install runs downloaded programs as
your user; the version probes use an empty environment and scratch home, not an
OS sandbox.

## State and service

Versions live at `~/.local/share/hyperthymesia/<version>` with a `current`
symlink and `~/.local/bin/hx-server`. The installer appends an idempotent PATH
block to .zshrc for zsh (or an existing .zshrc), otherwise the first existing
.bash_profile, .bash_login, .profile or .bashrc, defaulting to .profile.
It refuses symlink/non-regular startup files before installing. Open a new shell
or use the README's guarded PATH setup. Uninstall retains that block. macOS also
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
independently of umask. Automatic GC stays **off**, and `hx-server gc` is
disabled because deleting old chunks is not yet safe. See [GC safety](gc-safety.md).

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

Choose a provider and model in the app. Vercel AI Gateway accepts an API key;
subscription providers use their own sign-in. The guide names the provider you
chose. No account with any particular provider is required.

For CLI installations, use the private fx binary with `login PROVIDER`, replacing
`PROVIDER` with your chosen subscription provider. Doctor requires private login
files; the login and its parents below HOME must not be symlinks.

Gateway keys can be saved through the app's Settings API. The server stores them
privately under `secrets/`, never returns them, and applies changes on the next
run without restarting. `AI_GATEWAY_API_KEY` in `server.env` remains supported
for CLI installations. The installer keeps that file unchanged on rerun.
See the [app API](reference/app-api.md) for the model, Settings and Runs contract.

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
backups and sibling `<state>.failed-*` folders; sizes include both databases and
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

Create a dashboard token yourself; it is returned once, so store it privately.
To manage sources, it needs `read_raw` and the `source:` prefix:

```sh
hx-server --state-dir "$STATE" token create --name dashboard \
  --prefix source: --prefix macos-agent:DEVICE --prefix macos-media:DEVICE \
  --scopes read_raw,read_l3
```

Dashboard tokens are prefix-bound. Add `--prefix` for every git, shell or other
connector namespace you intend to see; otherwise its revisions/citations stay
hidden. Pairing grants ingestion, not read access. Installation never prints a
dashboard secret into unattended logs. Open `/login` on the server and paste the
token, then use **Connect sources**. `read_raw` also permits reading retained raw
evidence; only give this administrative token to people you trust with that data.

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
   requests return 503 while either the guard lock or durable `pending-upgrade`
   record blocks admission; agents retain retryable uploads. If the installer dies
   after the candidate starts, the candidate shuts down without admitting work.
   Its manager restarts through recovery before reopening state or accepting uploads.
4. Keeps the previous version and backup. A failed step after stop attempts to
   restore both databases and blobs, preserve `server.env`, and move source keys
   from the failed-generation state back into the restored state. It never merges
   two key directories. If that move fails, it keeps the recovery record and
   reports an error; inspect the retained state before restarting.
   Failed-generation state stays beside the active directory as `<state>.failed-*`
   (`server.failed-*` for the default macOS state).
   A private `<prefix>/.upgrade.*` recovery directory is published before stopping.
   Its non-executable guard anchor stays beside state on the same filesystem;
   the recovery executable stays in the prefix, including with a `noexec` data volume.
   The outer guard recovers even if the lifecycle shell receives SIGKILL, killing
   its remaining migration/copy processes before restoring. The lifecycle inherits
   the guard lock, so killing an installer ancestor cannot admit worker jobs.
   Wrapper/unit refreshes and path records use atomic replacement, including
   same-version reruns.
   Filesystem recovery errors are reported and the recovery program is retained.
   Incomplete recovery does not restart the old service. If atomic link
   replacement fails, it attempts a portable unlink/link while stopped.

`migrate` applies every missing version, including a lower number arriving later.
It rejects newer/unknown, dirty or changed checksums before writable access.
Missing migrations commit together; failed DDL rolls them back. `serve` never
migrates. For manual migration stop, back up, migrate, then start the service.
Rollback requires restoring a pre-upgrade backup into an absent/empty directory;
pointing an older binary at a newer schema is insufficient. Restore creates the
root and all nested directories as 0700 and files as 0600, independently of the
caller's umask. Backups omit `server.env`; keep that file separately and private.

A registered install whose core disappears refuses before changing anything,
including when a custom volume is unmounted. Reconnect the volume first. Look
for the original state in sibling `<state>.failed-*` folders and verified copies
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

If the installer process tree stops, rerun the installer. The service wrapper
will refuse automatic rollback because it cannot prove the old server has stopped.
The installer attempts recovery only after stopping the managed service. If a
failed-state directory already exists, recovery refuses to guess whether it is
the right history: keep all copies and inspect them before manually restoring.
Recovery after a real power loss has not been tested. Keep independent backups
before upgrading.
Do not delete `pending-upgrade`, its private `.upgrade.*` directory or the
`.hx-upgrade-lock-*` anchor beside state while recovery
is pending. Retained binaries, backups and failed state are never pruned automatically.
A server SIGKILL also closes fx's lifetime pipe: an independent watcher kills its
process group, including descendants, without relying on service-manager cleanup.
`--uninstall` stops/disables the managed **server** service and unlinks entry
points; it never deletes state, credentials, backups or retained versions.
**It does not yet stop or unregister the separately running Mac capture agent.**
Do not rely on it to stop recording. Stop capture and disable its login startup
separately until this is fixed.

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

A separate, manual Linux container run for #35 exercises a real schema-changing
installed upgrade, failed and killed upgrades, pairing/device OCR citations,
permissive-umask restore, and SIGKILL orphan cleanup. See the
[recorded end-to-end evidence](install-e2e.md). No real login or paid model call
is needed for that check.
