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
`HX_EGRESS_SENSITIVE=redact` sends only its kind and time. Both egress settings
and disclosure rules are part of the input digest. To apply a policy change to already interpreted windows, run
`interpret` again for the desired range; changing the file alone does not enqueue
old windows.

The serve process normalizes and compiles queued windows, schedules interpretation
at four-hour UTC boundaries, and enqueues a second interpretation at window end
plus 24 hours. Late data dirties affected windows, including the next window when
its thirty-minute input overlap is affected. The prior canonical revision remains
readable while new work runs. Changed entity memory invalidates existing windows;
changed carried tasks invalidate later windows. Run gc regularly with the same
data directory as serve; chunk files expire after thirty days, stills remain,
and pins prevent deletion. Raw envelopes and expired blob manifests remain.

## Server-owner disclosure policy

`HX_DISCLOSURE` selects a server-owned JSON rules file. When unset, the built-in
`hx-disclosure/v1` laptop policy applies to token-authorized installations:

- `macos-agent:`: title-free `focus.span`, `presence.span`, `input.bucket`,
  `clock.calibration`, and `collector.health` are normal. A `focus.span` with a
  non-null window title is sensitive as a whole record; its source interval still
  supports the timeline. `ui.*` and `text.*` are sensitive.
- `macos-media:`: derived `text.*` is sensitive; `media.still` is sensitive only as
  the lineage source for server `ocr.text` (image blobs never leave, and the still
  record itself is not packet evidence); `collector.health` is normal.
- `companion:browser:`, `companion:editor:`, and `companion:shell:`: events are
  sensitive, including open-ended companion kinds.

These prefixes identify the owner's laptop agent family; issue ingest tokens
only for trusted devices, preferably exact device prefixes. Every event must
first pass token scope and connector namespace checks at ingest. The disclosure
policy is applied only to stored, validated observations; raw producer labels
and stored envelopes are never rewritten. Other namespaces/kinds retain their
producer labels and existing unknown-kind restrictions. `policy.gap` retains its
label and never contributes content to the provider. `media.still` and
`media.chunk` are always excluded from provider input, even with a normal rule;
blob bytes/references are never added to the packet. Only derived text is eligible.

Rules use literal connector prefixes and exact kinds or a single trailing `*`.
The **first matching rule wins**: put specific exceptions before broad rules.
An optional `field_present` matches a non-null top-level payload field. A file
replaces all built-in rules; missing matches keep the producer class. For example:

```json
{
  "rules": [
    {"connector_prefix": "macos-agent:work-laptop", "kind": "text.*", "class": "restricted"},
    {"connector_prefix": "macos-agent:work-laptop", "kind": "focus.span", "field_present": "window_title", "class": "sensitive"},
    {"connector_prefix": "macos-agent:work-laptop", "kind": "focus.span", "class": "normal"}
  ]
}
```

Use `HX_DISCLOSURE=off` or a file containing `{"rules":[]}` to disable all
reclassification. Invalid or unreadable configuration fails preparation before a
provider call. This setting cannot be supplied by an upload. Sensitive grants
still obey `HX_EGRESS_SENSITIVE=redact`. Evidence records carry `effective_class`
separately from the existing kind prefix, which controls full-text vs redacted
transport. Each input manifest stores the complete rules, policy version and
SHA-256 digest; these participate in the interpretation input digest, including
when two policies happen to produce identical packets. Rules operate at record
level, so an owner granting normal for title-bearing focus explicitly discloses
the title too.

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

Interpretation input excludes effectively restricted observations and non-normal
entities. Sensitive observation text follows `HX_EGRESS_SENSITIVE`.
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

Operational backup/restore drills, production provider integration, and
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

## Server OCR

On macOS, `serve` starts an independent OCR worker using `hx-visual`'s Apple Vision
helper (`swiftc` must be installed). It discovers published `media.still` blobs on
startup and while serving, including old stills and either event/blob upload
ordering. Migration 0005 stores results per exact blob hash and engine identity
(helper source/configuration plus macOS build). One database advisory lock limits
OCR concurrency to one across server processes. Decoding and OCR run on a blocking
thread, independently of the interpretation queue. PNG, JPEG and TIFF stills are
supported by the sampling decoder; decode/engine failures retry five times with a
30-second delay, then remain visibly `failed` in `ocr_jobs`. An operator can reset
attempts/status after fixing the cause. Non-macOS servers retain stills for later
backfill; no empty result is recorded as successful OCR.

An 8×8 luminance hash skips frames within three bits of a recognized frame on the
same connector/device/display/application within thirty seconds. Skips are recorded
as `sampled` with a reference hash and no OCR result: similarity never borrows text
or deletes source evidence. All recognized text has partial OCR coverage.

`ocr_evidence` links each recognized result to every referencing still observation.
`ocr_derived` resolves time, device, connector and effective sensitivity through the
source at read time. Late source associations reuse exact results without another
OCR call. New derived evidence dirties affected interpretation windows, including
overlap. Derived evidence has a distinct, retrievable evidence ID and kind
`ocr.text`; the source observation ID and blob hash remain explicit in its payload.
The dashboard shows escaped OCR text beside the original thumbnail.

The disclosure integration point is `ocr::derived_evidence_for_sources`: call it
with evidence items after the server's source disclosure policy. It inherits each
source item's inclusion, redaction and self-generated decision, then uses the same
`PacketBudget` per-item and total limits as other evidence. OCR never creates a
producer-controlled disclosure grant. Synthetic fake-engine tests exercise jobs,
exact caching, sampling, lineage, retries, packet inclusion, authorization and HTML
escaping on every platform. `hx-visual` also tests real Vision OCR on a generated
text image only on macOS; capture permissions and actual screenshots are unused.

## fx agent interpreter

Run migrations, then select `HX_PROVIDER=fx` for `serve` or `interpret`. The stub
(default) and `HX_PROVIDER=claude` remain available. fx **0.0.16** is required;
version drift fails closed. `HX_FX_BINARY` overrides `$HOME/.local/bin/fx` and
`HX_FX_CREDENTIAL` overrides `$HOME/.fx/chatgpt-auth.json`. Sign in separately
with `fx login codex`; jobs never initiate sign-in or show approval dialogs.

Each window gets an authenticated, ephemeral loopback Streamable HTTP MCP server.
The bearer credential is random, per invocation, and never stored with the run.
Only the eight tools in the agent spec are exposed. Results are at most 48 KiB,
records use explicit excerpts above 12 KiB, and scans cover at most 2,000 rows.
Narrow time ranges to explore dense data. Timeline levels preserve exact seconds;
`minutes`/`lanes` select the same lossless compiled projection. Source completeness
is explicitly unknown rather than inferred from silence. Search defaults to the
job window; explicit ranges may cover any day in the store. `screen_at` returns
OCR/text and focus within 30 seconds, with no image bytes.

Jobs use a private temporary HOME/profile and empty working directory: `context`
is false, all skill discovery roots are empty, inherited MCP servers and settings
are absent, and all tools are denied except broker discovery and the broker's
namespaced tools. The subscription credential alone is copied with mode 0600 and
removed with the temporary directory. fx's credential advisory lock serializes
refreshes; refreshed credentials are atomically persisted back to the source file
with mode 0600, including on cancellation. Busy credentials defer the job without
spending attempts. Environment variables (including API keys,
tracing, additional workspaces and user shell configuration) are not inherited.
No full-access flag is used. The network boundary is fx's denied tools, not an OS
network sandbox; the harness itself still contacts ChatGPT and the loopback MCP
server. Automatic upgrades, session saves, prompt history, titles and sounds are
disabled. `--system` accepts text, so the compiled-in versioned prompt file's
contents are passed verbatim, with its version and SHA-256 recorded.

A run has 120 total model steps across up to three scoped repairs (`HX_FX_MAX_STEPS`),
240 total broker calls and a 1,200-second total timeout (`HX_FX_TIMEOUT_S`). A dense
real four-hour window exhausted the original 24-step budget before producing output. Output streams are capped at 2 MiB each.
Provider failures and timeouts keep the previous canonical revision. A valid
repaired response is retained if a later repair fails. Subscription rate limits
defer jobs for five minutes without consuming attempts. Publication checks the
input generation atomically and rejects stale agent runs without storing them.

`revisions.agent_run` stores exact broker arguments/results, evidence projections,
fx JSON completion logs, and pinned model/prompt settings in the same transaction
as the revision. Dashboard evidence links under `/revision/{revision}/evidence/{id}`
open that immutable projection, with a link to the original stored record and its
source links. `/revision/{revision}/run` shows the complete audit. These routes
require raw-read authorization for every input namespace and fail closed when the
current disclosure policy or sensitive-egress setting differs from the run.
Legacy prior replicas without a matching policy manifest are withheld. Entity
assertions resolve their evidence against the current policy, regardless of cached
producer labels.

`live_lookup` currently returns `no live connectors configured`. A future connector
must snapshot the exact returned content into an immutable server-owned event
before returning it: use an authenticated connector namespace, source URL and
retrieval timestamp, preserve the complete response as text/JSON evidence, assign
an event ID, and project it through the same disclosure broker. A changing remote
document creates another event/revision; it must never mutate the cited snapshot.
The generation manifest must include the new snapshot before publication. No live
connector or remote network operation is reachable through the current stub.

Synthetic tests:

```sh
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/synthetic-test-runner.py" \
  HX_TEST_DATABASE_URL=postgres://hx:hx@localhost:55432/YOUR_SCRATCH_DB cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# Optional subscription smoke, synthetic fixtures only:
CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/synthetic-test-runner.py" \
  HX_TEST_DATABASE_URL=postgres://hx:hx@localhost:55432/YOUR_SCRATCH_DB HX_LIVE_FX=1 \
  HX_FX_BINARY=/absolute/path/to/fx HX_FX_CREDENTIAL=/absolute/path/to/chatgpt-auth.json \
  cargo test -p hx-server --test server fx_live_synthetic_only -- --nocapture
```

Use the runner variable for your Rust host triple on other platforms. The runner
keeps connector fixtures from consulting real home directories. The live smoke
requires explicit binary and credential paths because its test home is empty.
