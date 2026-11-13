# Web app serving and live entry log evidence

Verified on 2026-11-13 with a real `hx-server` binary copied outside the checkout,
a fresh temporary state directory, an automatically selected loopback port, and
synthetic API credentials and entries. No live service or owner state was used.
The UI source is not present in this ticket, so the embedded page is the allowed
placeholder; the runtime build override used synthetic HTML and JavaScript.

Checks passed:

- `cargo fmt --all -- --check`
- `cargo clippy --locked -p hx-server -p hx-store -p hx-agent -p hx-agent-app -p hx-macos-sys --all-targets -- -D warnings`
- `cargo check --locked -p hx-server -p hx-store -p hx-agent -p hx-agent-app -p hx-macos-sys --all-targets`
- `cargo build --locked -p hx-server`
- `python3 scripts/check-store-transactions.py`
- `bash -n scripts/build-web.sh scripts/build-release.sh`
- `scripts/build-web.sh` correctly selected the placeholder before UI source exists.
- Regenerated the source-derived route/environment inventories using the existing
  reference generator; local documentation links passed.

The one-off API client opened SSE before posting entries, then exercised the
following behavior against the running server. Its output was:

```text
PASS relocated binary serves embedded placeholder and SPA route; missing assets return 404
PASS unauthenticated=401, ingest-only=403, invalid cursor/offset=400
PASS open SSE received three arrivals from Mac and connector namespaces: [1, 2, 3]
PASS duplicate upload adds no entries; namespace boundary excludes proof-extra from proof token
Today: {"active_sources": 3, "end": "2026-11-14T06:00:00Z", "entries_today": 3, "last_pull_at": null, "start": "2026-11-13T06:00:00Z"}
PASS Last-Event-ID wins over query; replay caught up in 100 + 3 entries with ascending IDs
PASS restart retained 106 entries and saved cursor resumed with exactly the new entry
PASS last_pull_at reflects source pull metadata and remains namespace scoped
PASS token revocation closes an open stream and rejects reconnect
PASS runtime web build, JS MIME, SPA fallback, live asset changes, symlink escape refusal, cookie login redirect to /app
```

The last-pull check seeded only synthetic source metadata in the temporary
store. The Mac menu action was compiled and linted, but the installed recording
app was not launched or replaced. The full release/signing/installer lifecycle
was not run: assets are embedded in the existing server executable, so release
and rollback use the existing binary packaging path. No routine tests were added.
