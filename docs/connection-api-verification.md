# Connections API verification

Verified on macOS with a separately built `hx-server`, temporary state, loopback
source services and a scratch copy of the fx credential. The installed server
and its state were not used. All source records and source credentials were
synthetic. The setup worker used real fx; interpretation was disabled for this
isolated verification.

The API client connected a custom API with a private `X-API-Key` header. Without
further client setup, fx discovered the records, tested and saved a recipe, and
reported a successful setup. The recipe retry endpoint ingested a record and the
connection entries endpoint returned it. Source and recipe off switches blocked
retries. Restarting the server preserved the off switch and saved evidence.

Selected client output:

```text
Catalog: 26 entries; 12 categories
API connect: setting_up
Agent setup: succeeded; one recipe; no_pull: null
Pull cadence: 14400 seconds
Pulled entries: 1
Restart: off switch and evidence retained
Imported HTTP MCP: setting_up
CLI probe: setting_up
Refused key: needs_reconnect; replacement key: setup resumed
OAuth: PKCE URL, exchange, private credentials and replay rejection passed
PASS: owner auth, API setup/pull, retry, switches, restart, MCP import,
      CLI, validation, reconnect
STDIO MCP: imported config, expanded variable, private env, real fx setup,
           deterministic pull and redacted Agent response passed; entries: 1
```

OAuth used a local authorization-code issuer, not a personal third-party login.
The client also checked missing owner auth, missing import selection, unresolved
variables and a forbidden HTTP method. Source credential values were checked
against JSON responses. Catalog services that require personal accounts were
not individually signed in; their requirements are returned in catalog metadata.

Checks (with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0` to limit disk use):

- `cargo check --locked -p hx-server`: passed.
- `cargo clippy --locked -p hx-server -p hx-sources -p hx-store --all-targets -- -D warnings`: passed.
- `cargo build --locked -p hx-server`: passed.
- `cargo test --locked -p hx-sources`: 47 passed.
- `cargo fmt --all -- --check`: passed.
- `python3 scripts/check-store-transactions.py`: passed.
- `python3 scripts/check-tooling.py`: passed.
- `python3 scripts/reference.py`: regenerated references and validated local links.

An earlier verification attempt stopped after pulling an entry because the disk
filled. The successful run above used fresh state after disk space was restored.
