# Native capture agent

`hx-agent` keeps AppKit on the main thread, performs AX sampling on bounded worker
queues, and hands observations to a Tokio I/O worker. Native calls and Objective-C
lifecycle code live in `hx-macos-sys`. Content is restricted by default. One capture
policy gates text, click context, companions, media encoding, and durable outbox
writes; Secure Input pauses content while metadata continues.

```sh
source ~/.cargo/env
export CARGO_TARGET_DIR="$PWD/.target-native"
cargo run -p hx-agent -- status
cargo run -p hx-agent -- run --dry-run --duration-s 62
scripts/build-app.sh
```

Configuration is read from `~/Library/Application Support/Hyperthymesia/agent.json`;
missing fields use the defaults in `src/config.rs`. An empty token disables upload.
Normal capture uses a WAL SQLite outbox and `agent.sock` in the same directory.
`run --dry-run` writes envelopes to stdout without creating an outbox or uploading.
Only the explicit `onboard` command requests permissions. `install-nm-host
--extension-id <id>` writes the Chrome manifest and a launcher; it does not install
an extension or start capture.

## Timeline contract

- `focus.span` and `presence.span` are upserts: revision 0 opens the span and a later
  revision closes it. Native notifications retain capture-time monotonic stamps.
  Durable checkpoints survive upload acknowledgments. At startup, intervals left
  open by an interrupted run close at that run's last durable observation; a
  content-free `capture_interrupted` gap bounds the unknown time through restart.
  Across reboots, downtime uses UTC rather than subtracting unrelated clocks.
- Health collector names are kind namespaces: `focus`, `presence`, `input`, `ui`,
  `text`, `media`, and `clock`. Heartbeats run every 45 seconds, leaving margin
  before the compiler's 60-second liveness expiry. Screen Recording permission
  changes bypass the heartbeat timer, and the main loop reports detected loss
  even while the media worker is finalizing an encoder.
- `policy.gap` upserts carry the excluded interval in envelope source and monotonic
  start/end fields. An active gap extends only through observed time, updates
  every ten seconds, and closes on policy exit or orderly shutdown. Its payload
  contains no excluded application or window content. Unverifiable native browser
  privacy state opens the same gap and redacts the focused-window metadata.
  Content-free companion gap copies pass the persistence gate without requiring
  a public-window signal. Suppression is also emitted
  for the media and companion installations sharing this device policy.
- Upload batches are persisted before transport and replayed unchanged until a
  matching complete acknowledgment. Blob PUTs are idempotent. Media eviction
  preserves every affected observation's manifest in durable loss records.
- Video intervals end at the last accepted frame's capture stamp, never at encoder
  finalization. Queued frames are checked again for permission and focus before
  encoding.
- Event-budget, oversized-media, pinned-media and eviction-loss-budget rejections
  write content-free, deliverable `collector.health` loss summaries in 64 KiB of
  additional outbox headroom reserved for losses and restart reconciliation.
  Repeated losses coalesce by installation and reason with a count, time range,
  and first/latest observation IDs; pending upload batches remain immutable.
  Budget rejection leaves metadata capture and upload alive where capacity permits.
  Exhausted recovery headroom or other storage errors still stop capture explicitly.
  The in-memory handoff remains volatile until the I/O worker commits it.

## Verification and limits

Run `cargo test -p hx-agent -p hx-macos-sys -p hx-agent-app`, then the corresponding
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` package checks.
Synthetic tests include policy-before-persistence, framing, shared-media eviction,
acknowledgments, restarts, a real uploader process kill after server commit, and
policy-gap/health integration with the actual `hx-compile` implementation.

Visual capture currently includes only the known foreground window on the main
display, with partial coverage explicitly labeled. Broad full-display capture
needs a continuously refreshed privacy map. Browser windows, including private
and incognito windows, are captured under the same policy as other apps.
Chromium/Electron AX is enabled once per PID on capture workers with short
messaging timeouts. Secure text fields, Secure Input, user exclusions, pause,
and lock-screen gates remain in effect. Interrupted staging videos are detected and retained, but automatic
recovery of incomplete encoder files is not implemented. Authorized native capture,
menu interaction, sleep/wake behavior, an extended soak, and OCR legibility require
hardware qualification. UTC accuracy is unmeasured and labeled accordingly.

Real capture evidence and signing logs belong in the locally ignored `artifacts/`
directory. Do not publish them. Build output stays in ignored `dist/` and target
directories. Building the bundle does not register or launch it. On first manual
launch, certificate-signed builds register their bundled agent with SMAppService;
ad-hoc builds install `~/Library/LaunchAgents/com.hyperthymesia.agent.plist` and
bootstrap only the current user's GUI domain. Both launch at login and restart
failed exits (throttled to 30 seconds). Menu Quit exits successfully and stays quit
until the next login or manual launch. Reopen the bundle after moving an ad-hoc
build so its absolute executable path is refreshed. A disabled modern service
stays disabled; the app reports the Login Items setting rather than bypassing it.

The menu shows `Hx !` with a plain-language explanation for screen capture errors.
Media health reaches the menu in memory before disk/network delivery, so slow
uploads cannot hide a writer failure. The menu refreshes every second and detects
a stalled running heartbeat after 55 seconds. A successful frame clears the error;
Pause hides it immediately. Failed writer construction removes its partial file,
and each retry recreates a missing staging directory. Zero-byte interrupted MP4s
are removed; nonempty interrupted files remain available for manual inspection.

Complete per-record upload rejections go to `dead_letters` in `outbox.sqlite`,
with the original envelope, batch, reason and rejection time. Referenced blobs
remain pinned through `dead_letter_refs`. Accepted and duplicate records leave the
outbox in the same transaction, so subsequent batches continue. Incomplete,
mismatched, duplicate-ID or unknown-ID acknowledgements preserve the batch, as do
HTTP authentication/overload failures. `health.json` and `hx-agent status` expose
the cumulative refusal count under `upload.dropped`. Dead letters require manual
inspection/removal and retained blobs count against the local media budget.
