# Server

The server stores raw evidence before acknowledging producers and runs derived
processing through a leased Postgres queue. Fixtures in this crate are synthetic.

## Run

Set DATABASE_URL to a local Postgres URL. From the workspace:

    cargo run -p hx-server -- migrate
    cargo run -p hx-server -- token create --name laptop --prefix macos-agent: --scopes ingest,read_raw,read_l3
    cargo run --release -p hx-server -- serve --listen 127.0.0.1:7377 --data-dir ./data

Token creation prints the credential once; only its SHA-256 is stored. Issue
separate ingest-only and reader credentials for actual deployments. read_raw
includes reader operations. Prefixes are literal connector-instance prefixes;
choose namespace delimiters deliberately. The listener rejects non-loopback
addresses. Put a private Tailscale proxy in front when accessing another machine;
transport/proxy setup is external.

Visit /login and enter a reader token for the dashboard. The cookie is HttpOnly
and SameSite Strict, expires in twelve hours, and contains a revocable token.
Use HTTPS at the private proxy. REST callers use Authorization: Bearer credentials.
Revocation is currently administrative: set device_tokens.revoked=true for the
credential hash.

    cargo run -p hx-server -- compile --from 2026-01-01T00:00:00Z --to 2026-01-02T00:00:00Z
    cargo run -p hx-server -- interpret --from 2026-01-01T00:00:00Z --to 2026-01-02T00:00:00Z
    cargo run -p hx-server -- gc --data-dir ./data

Interpretation uses a stub provider unless `HX_PROVIDER=claude` selects the
`claude -p` CLI (model from `HX_MODEL`, default `sonnet`). Restricted evidence
never reaches a provider. Sensitive evidence is sent in full text by default;
`HX_EGRESS_SENSITIVE=redact` sends only its kind and time. The policy is part of
the input digest, so changing it re-interprets windows.

The serve process normalizes and compiles queued windows, schedules interpretation
at four-hour UTC boundaries, and enqueues a second interpretation at window end
plus 24 hours. Late data dirties affected windows, including the next window when
its thirty-minute input overlap is affected. The prior canonical revision remains
readable while new work runs. Changed entity memory invalidates existing windows;
changed carried tasks invalidate later windows. Run gc regularly with the same
data directory as serve; chunk files expire after thirty days, stills remain,
and pins prevent deletion. Raw envelopes and expired blob manifests remain.

## API and disclosure

- POST /v1/events accepts hx_schema::UploadBatch JSON with optional zstd encoding.
  Acknowledgments echo batch_id. Parseable observation identities get explicit
  rejection reasons for invalid or forbidden envelopes. Invalid JSON, missing
  batch identity, or an unparseable observation identity returns HTTP 400 before
  anything commits. Core payload decoding happens asynchronously; failures
  quarantine L1 while retaining L0.
- PUT /v1/blobs/{sha256} verifies raw bytes, returning 201 on publish and 200 on
  an already-published hash. Replay recovers interrupted publication. Files use
  same-directory temporary files, fsync and atomic rename.
- GET /v1/blobs/{sha256} requires read_raw and a matching evidence namespace.
  Downloads use safe attachment headers; expired content returns 410.
- GET /v1/timeline?from=…&to=…&level=raw|lanes|episodes|tasks accepts RFC3339 bounds,
  with positive ranges up to 31 days. Raw and lanes require read_raw. Responses
  include coverage, observation/interpretation watermarks and immutable revision
  IDs/generations. Each revision includes its input manifest. A canonical
  interpretation is withheld unless the token covers every namespace in its
  transitive evidence, entity and carried-task lineage.
- GET /v1/search?q=…, /v1/evidence/{id}, /v1/entities and /v1/now enforce read scopes
  and namespaces. Sensitive/restricted text requires read_raw. Unknown kinds stay
  restricted even when producer-labeled normal.
- / and /day/YYYY-MM-DD show UTC day lanes, points, coverage, exposure totals,
  episode/task sections, anchor zoom levels, search, and linked evidence pages.
  Evidence pages escape text and embed only local PNG/JPEG stills. Captured HTML,
  SVG, scripts, remote images and remote fonts are never rendered.

Interpretation input excludes sensitive/restricted observations and entities.
Self-generated evidence is tagged; captured source content remains data. An
immutable manifest records the bounded packet, versions, namespace lineage,
provider identity and input generation; its digest is supplied to the interpreter.
Publishing checks current generation and monotonically advancing revision
identity. Retry output keys preserve one immutable result per job.

The coordinator's provider wiring point is processing::coordinator_provider.
The default stub returns empty synthetic interpretations. `HX_PROVIDER=claude`
selects the local Claude CLI, with `HX_MODEL` choosing its model.

Normalize and compile jobs use one durable slot per four-hour window. Ingest
bumps each affected generation once per committed upload, including overlap
windows, and updates the latest request without replacing a running lease.
Normalization writes chunks of 512 events. Completion requeues a slot when input
changed during its lease; generation checks still fence compiled and published
outputs. The worker drains available work immediately and sleeps when idle.
Missing normalization or changed compile inputs defer for one second without
spending an attempt. Stored job errors use fixed categories or SQLSTATE codes,
never arbitrary provider/database/payload text; detailed errors stay in local logs.

Stop existing workers before running the migration and restart on the new binary.
Migration 0004 retires the legacy normalize/compile backlog, fences its leases,
and seeds window slots for remaining work. Existing failed interpret jobs are
preserved; an operator can reset their attempts and requeue them after inspecting
the local failure. The scheduler also requests interpretation for new generations.

## Verification

    cargo test -p hx-store -p hx-jobs -p hx-server
    cargo clippy -p hx-store -p hx-jobs -p hx-server --all-targets -- -D warnings
    cargo fmt -p hx-store -p hx-jobs -p hx-server -- --check

Set the task's CARGO_TARGET_DIR for every cargo command.
Database tests require Postgres, default to the local development database,
and accept HX_TEST_DATABASE_URL. Each uses a unique schema and temporary blob
directory, dropping its schema after success. Tests never skip silently if
Postgres is unavailable. Failed tests may leave synthetic schemas for diagnosis.

Operational backup/restore drills, production provider integration, OCR, and
long-duration deployment monitoring are outside this crate's verification.

The `synthetic_day_burst` test ingests 20,000 generated agent messages over one
UTC day in forty 500-event uploads, drains with the stub provider, checks all
seven windows (including trailing overlap), and enforces a 60-second total bound.
For an optimized measurement, run:

    cargo test --release -p hx-server synthetic_day_burst -- --nocapture

An additional `synthetic_browser_point_stress` test uses open-ended browser
transitions. It is explicitly ignored by default because profiling exposes a
quadratic whole-window scan with repeated payload decoding inside hx-compile,
which is outside this queue change. Its 60-second target is not met; this remains
a compiler follow-up, not evidence of general workload performance:

    cargo test --release -p hx-server synthetic_browser_point_stress -- --ignored --nocapture
