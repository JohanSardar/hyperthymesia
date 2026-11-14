# Connector SDK

The Apple Bridge connector (`connectors/apple-bridge`) supports:

```sh
hx-connector-apple-bridge describe
hx-connector-apple-bridge sync --dry-run --limit 20
hx-connector-apple-bridge sync --since 2000-01-01T00:00:00Z
```

Set `HX_SERVER` and `HX_TOKEN` for delivery (or `--server` and `--token`).
`HX_INSTANCE` / `--instance` selects the installation namespace. The default
hashes the hostname and source location; set an explicit stable instance when
moving a source or changing hostnames. `HX_SOURCE` / `--source` overrides the
Apple Bridge base URL. Apple Bridge defaults to its loopback API on port 7780, and uses
`HX_APPLE_TOKEN` / `--apple-token` if authentication is needed. Only its
`records.query` read operation is invoked.

State lives in `~/Library/Application Support/Hyperthymesia/connectors/`;
`--state-dir` overrides this for testing. A per-instance lock prevents concurrent
writers. State files have mode 0600 and are written with fsync and atomic rename.
Pending batches, including observation and batch IDs, are saved before upload.
Retries and process restarts reuse that batch. Only a matching acknowledgement
covering every envelope commits its next cursor. A partial source scan can still
checkpoint its successfully collected prefix; it never implies deletions.

Dry runs always backfill from the start, print envelope NDJSON to stdout, report
coverage to stderr, and neither read nor write connector state. A normal sync
uploads and checkpoints up to 100 bounded batches per invocation, stopping when
`complete` is true, the cursor stops progressing, or 30 seconds have elapsed
between batches. A slow source read or upload can exceed that soft time budget.
Repeat sync if the final report still has `complete:false`. Every batch has its
own durable pending record and matching acknowledgement before the next scan.
`--limit` bounds each batch. Sources may
emit fewer envelopes for metadata-only or excluded records. `--since` filters
source timestamps inclusively; records with unknown timestamps remain eligible.
Use a new instance for an independent historical backfill.



Apple snapshots include tombstones and resume pagination; a completed snapshot
starts another revision reconciliation pass on the next sync. No absence is
interpreted as a delete. Upstream coverage stays partial, and offline or
unsupported responses report unknown coverage. App-day rollups retain the source
civil day/zone and caveats and have no source point or activity span. Floating
calendar dates are retained in `planned_local` with `timing_unknown`, without an
absolute `planned` interval; the frozen core CalendarEvent decoder cannot yet
normalize those records and must abstain until civil-date support is added.

`scripts/extract-fixtures.sh` streams small real dry runs directly through a
deny-by-default anonymizer into fixtures. No raw private output is written to a
file. Every string outside an exact structural enum allowlist is hashed or
replaced with a length-class placeholder; times and observation IDs are normalized
for deterministic output. `input.jsonl` fixtures are entirely synthetic parser
inputs, while `envelopes.ndjson` samples are anonymized connector outputs. The
privacy test rejects paths, emails, URLs, and prose in the whole fixture tree.

A complete acknowledgement may refuse individual records. Sync commits those
original envelopes and reasons to `<instance>.dead-letter.sqlite` (`dead_letters`
table) before advancing the cursor, and writes the cumulative refusal count to
`<instance>.health.json` as `upload.dropped`. Later batches continue. An interrupted
checkpoint write can replay the batch; dead-letter inserts are idempotent by
observation ID. HTTP auth/overload errors and malformed or incomplete acknowledgements
keep the pending batch and cursor intact. Low-level `upload()` returns the validated
`UploadAck`; callers outside `run_cli()` must retain its rejected records before
advancing their own checkpoint.
