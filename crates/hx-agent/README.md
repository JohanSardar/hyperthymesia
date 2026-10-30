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
- Health collector names are kind namespaces: `focus`, `presence`, `input`, `ui`,
  `text`, `media`, and `clock`. Heartbeats run every 45 seconds, leaving margin
  before the compiler's 60-second liveness expiry. Permission loss is explicit.
- `policy.gap` upserts carry the excluded interval in envelope source and monotonic
  start/end fields. An active gap extends only through observed time, updates
  every ten seconds, and closes on policy exit or orderly shutdown. Its payload
  contains no excluded application or window content. Suppression is also emitted
  for the media and companion installations sharing this device policy.
- Upload batches are persisted before transport and replayed unchanged until a
  matching complete acknowledgment. Blob PUTs are idempotent. Media eviction
  preserves every affected observation's manifest in durable loss records.

## Verification and limits

Run `cargo test -p hx-agent -p hx-macos-sys -p hx-agent-app`, then the corresponding
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` package checks.
Synthetic tests include policy-before-persistence, framing, shared-media eviction,
acknowledgments, restarts, a real uploader process kill after server commit, and
policy-gap/health integration with the actual `hx-compile` implementation.

Visual capture currently includes only the known foreground window on the main
display, with partial coverage explicitly labeled. Broad full-display capture
needs a continuously refreshed privacy map. Browsers without a trusted private
window signal are metadata-only at the native layer; Chromium/Electron AX is never
forced. Interrupted staging videos are detected and retained, but automatic
recovery of incomplete encoder files is not implemented. Authorized native capture,
menu interaction, sleep/wake behavior, an extended soak, and OCR legibility require
hardware qualification. UTC accuracy is unmeasured and labeled accordingly.

Real capture evidence and signing logs belong in the locally ignored `artifacts/`
directory. Do not publish them. Build output stays in ignored `dist/` and target
directories. The generated LaunchAgent plist is not installed automatically.
