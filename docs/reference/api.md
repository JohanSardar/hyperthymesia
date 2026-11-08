# API contracts

The server binds loopback only. Base URL defaults to `http://127.0.0.1:7377`.
Use HTTPS through a private proxy for another machine. This is a 0.x API;
additive fields may appear. Do not depend on dashboard form internals.

## Authentication and errors

Use `Authorization: Bearer TOKEN`, or the dashboard's `hx_token` cookie.
`read_raw` also permits reader operations. All reader operations enforce literal
connector namespaces. A dashboard login verifies the supplied token and sets an
HttpOnly, SameSite Strict cookie for twelve hours. Token creation, listing and
revocation are CLI commands, not HTTP endpoints.

Errors normally contain `{"error":"message"}`: 400 invalid input, 401 missing or
invalid credentials, 403 missing scope, 404 absent/out-of-namespace evidence.
Pairing deliberately returns uniform 410 refusals and empty 503 on internal
failure. Size-limit errors use 413. Upgrade guards can temporarily return 503.
Framework query/JSON extraction errors can be plain-text 400/422 rather than
the application's JSON error object. Default request bodies are capped at
64 MiB, with smaller route-specific limits below; oversized decoded zstd event
batches are rejected with 400.
Do not put tokens in URLs, browser links, logs or issue reports.

## Evidence and readers

| Endpoint | Input and permission | Result |
| --- | --- | --- |
| GET `/healthz` | No token | Plain `ok` plus newline; not provider readiness |
| POST `/v1/pair` | JSON `code`, `device_id`; unexpired single-use code, 4 KiB limit | JSON `token`; no-store; replay/refusal 410 |
| POST `/v1/events` | `ingest`; [UploadBatch schema](../../schema/); optional zstd | Batch acknowledgement; raw data commits before acknowledgement, normalization is asynchronous |
| PUT `/v1/blobs/{sha256}` | `ingest`; raw bytes, verified hash | 201 new, 200 already published; no prefix/quota check on paired uploads |
| GET `/v1/blobs/{sha256}` | `read_raw`, namespace-authorized referencing evidence | Attachment bytes; 410 expired |
| GET `/v1/device` | `ingest` | Negotiated OCR capability |
| PUT `/v1/ocr/{sha}` | `ingest`; paired image namespace; OCR result JSON, 1 MiB limit | 200 receipt; 409 when server OCR is selected or there is no authorized still; lineage is validated |
| GET `/v1/timeline` | `from`, `to` RFC3339; positive range ≤31 days; `level=raw|lanes|episodes|tasks` (default episodes) | Coverage, watermarks, revisions; raw observations or compiled lanes for those levels |
| GET `/v1/now` | `read_l3` (or raw) | Last four hours of episode-level timeline |
| GET `/v1/search` | `q`; `read_l3` (or raw) | `results`; namespace/sensitivity filtered FTS |
| GET `/v1/evidence/{id}` | `read_l3`; raw required for non-normal sensitivity | Envelope/evidence; unauthorized namespaces are not found |
| GET `/v1/entities` | `read_l3` (or raw) | `entities`, filtered by namespace and sensitivity |

Raw/lanes timeline levels require `read_raw`. For other levels, non-raw readers
cannot see a revision unless its entire transitive evidence/namespace lineage
is authorized and permitted by current disclosure. Raw-reader access is the
owner's explicit exception, not model-provider disclosure permission.

The canonical transport types are in
[hx-schema protocol](../../crates/hx-schema/src/protocol.rs),
[core kinds](../../crates/hx-schema/src/kinds.rs), and
[server handlers](../../crates/hx-server/src/lib.rs). JSON schemas and tests
check the envelope/companion contracts. See the [complete route inventory](routes.md).

## Source administration

All following endpoints require `read_raw` and the `source:` namespace.
IDs are lowercase 26-character ULIDs. JSON input rejects unknown fields.
Source keys stay in private files, never SQLite/public response fields.

| Endpoint | Input | Result |
| --- | --- | --- |
| GET `/v1/sources` | None | Scrubbed source/configuration metadata, never keys |
| POST `/v1/sources` | `name` (nonblank, ≤200 UTF-8 bytes, no control characters), `description` (≤16384 bytes), optional `key`, `probe`, `probe_json`, `cli_home`, `cli_executable`, `history_days` (0–30, default 1) | Creates/enables source; asynchronous paid setup |
| PATCH `/v1/sources/{id}` | At least one of `enabled`, `key` | Updated switch state; key rotation validates saved config |
| POST `/v1/sources/{id}/setup` | No body required | 202 with `setup_generation` and `status` |
| GET `/v1/recipes`, `/v1/addons`, `/v1/agent-tables` | Optional `source` query | Scrubbed configured objects and status |
| PATCH `/v1/recipes/{id}`, `/v1/addons/{id}` | `enabled` boolean | Owner switch state |

Source routes have a 32 KiB body limit. Source request variants and their fields
are defined in [Request](../../crates/hx-sources/src/request.rs).
HTTP/CLI/MCP operations have bounded output, deadlines, no implicit retry, and
credential scrubbing; that is not host isolation. Turning a source off stops
broker requests/pulls, including in-flight exchanges, but retains keys and data.
There is no source-deletion endpoint.

Dashboard paths, form methods and callback routes appear in [routes.md](routes.md).
They return escaped HTML/redirects; do not automate them as a stable JSON API.

## Run-scoped MCP and Rust APIs

The private broker has its own authenticated, run-scoped endpoint, not an
unauthenticated public MCP service. Window and source-setup modes expose
different tools. Setup is bound to one server-selected source.
POST `/mcp` accepts JSON-RPC 2.0 with a run-secret bearer token and a 32 KiB body
limit. `initialize` reports protocol `2025-11-25`; `ping` returns an empty object;
`tools/list` lists the current mode; `tools/call` takes `name` and `arguments`.
Requests without `id` return 202. Unknown methods use JSON-RPC error -32601.
Call failures use text content with `isError=true`, not an HTTP error status.
[mcp-tools.json](mcp-tools.json) is exported from the actual tool registry and
contains every tool's input schema. Host bash is included in that reference;
`HX_HOST_BASH=off` removes it. Source commands remain possible.

Every public Rust crate/type/function is navigable in generated rustdoc:

```sh
cargo doc --locked --workspace --no-deps
```

On Linux append `--exclude hx-macos-sys --exclude hx-agent --exclude hx-agent-app`.
Open `target/doc/hx_server/index.html` (or the configured CARGO_TARGET_DIR).
This reference documents callable surfaces, not a promise that every public
Rust helper is stable before 1.0.
