# Architecture

Hyperthymesia keeps captured evidence separate from derived interpretations.
It is a Cargo workspace with a loopback server, optional Mac capture app,
source connectors and browser/editor/shell companions. No database service is
needed for a new installation.

## Data flow

```text
Mac collectors / companions       source connectors / configured pulls
              |                                  |
       durable local outbox               connector checkpoints
              +---------------+------------------+
                              |
               authenticated REST ingest + blob upload
                              |
              core.sqlite + blobs (commit before acknowledgement)
                              |
               leased jobs: normalize / OCR / compile
                              |
                deterministic four-hour interval index
                              |
             model interpretation + citation/claim validation
                              |
              immutable revisions → REST API / day dashboard
```

Normalization failures quarantine derived work without deleting raw evidence.
Deduplication uses connector/source/revision identity. Queue leases fence
completion, and generation checks reject stale publication. Missing capture
is unknown, not proof that no activity occurred. Foreground exposure is not
the same as human attention.

## Module boundaries

| Location | Owns |
| --- | --- |
| `crates/hx-schema` + `schema/` | Envelope, time, identity, policy and transport contracts; checked JSON schemas |
| `crates/hx-store` | SQLite schema/migrations, state privacy, evidence persistence/search, tokens, backups and restore |
| `crates/hx-jobs` | Durable scheduling, lease ownership, heartbeat and completion fencing through store transactions |
| `crates/hx-compile` | Deterministic interval/coverage/exposure projection, independent of model calls |
| `crates/hx-interpret` | Fact sheet, work sessions and their add-up check, provider contract, legacy claims and citation validation |
| `crates/hx-visual` | Image sampling/cache and OCR helper; native Vision on a Mac |
| `crates/hx-sources` | API, CLI and MCP source transport, bounded output and command/process lifecycle |
| `crates/hx-server` | CLI, REST/dashboard/auth, disclosure, processing coordinator, source setup/pulls and fx broker |
| `crates/hx-connector-sdk`, `connectors/` | Producer protocol, resumable checkpoints and source-specific adapters |
| `crates/hx-macos-sys` | Narrow native macOS FFI boundary, including capture and permission APIs |
| `crates/hx-agent`, `crates/hx-agent-app` | Capture policy/state, private outbox, retries, pairing and menu-bar application |
| `companions/` | Semantic browser/editor/shell producers; no direct writes to the server database |
| `crates/hx-move` | Explicit legacy PostgreSQL cutover utility, not a runtime dependency of the server |

Core schema/compiler crates do not depend on the server, UI or a model runner.
The server coordinates those libraries rather than making collectors interpret
their own evidence. macOS FFI is isolated; other workspace Rust forbids unsafe
code. Integration tests live beside each crate in `tests/`; unit tests stay near
their code. `fixtures/` contains synthetic/anonymized inputs, not user captures.

## State and lifecycle

An explicit local state directory holds `core.sqlite`, `agent.sqlite`, `blobs/`
and `locks/`; source keys live in `secrets/`. `core.sqlite` owns evidence,
indices, jobs and interpretation revisions. `agent.sqlite` is separate storage
for agent-created tables. SQLite WAL requires local disk, not a network share.

`migrate` explicitly initializes/upgrades state. Ordinary `serve` and read
commands do not silently create missing history. Backups snapshot both databases
and published blobs but intentionally omit source keys and server.env. The
installer retains versions, pre-upgrade backups and failed-generation state,
stops the service before switching, and guards upgrades against active work.
The lifecycle recovery record survives a killed installer.

## Trust boundaries and non-goals

- REST authenticates tokens and enforces scopes/namespaces. The listener stays
  loopback-only; private remote transport is external.
- Disclosure rules govern provider packets and reader access. They are not an
  OS sandbox. The fx agent's host bash can access files, network, keys and CLI
  logins under the server account. `HX_HOST_BASH=off` removes that tool, not
  configured source-command execution.
- Captured/source text is untrusted data. Citation validation is not proof
  against every prompt injection or model error.
- State and backups are not encrypted at rest. Switching a source off retains
  its data/keys. Automatic GC and manual GC deletion are disabled.
- macOS permissions, real-provider authentication, capture accuracy and an
  eight-hour soak need separate acceptance evidence; synthetic tests cannot
  establish them.

See the [server contract](crates/hx-server/README.md),
[connector protocol](crates/hx-connector-sdk/README.md), and
[installation/recovery guide](docs/install.md) for operational details.
