# Connector SDK

All five binaries support:

```sh
hx-connector-t3code describe
hx-connector-t3code sync --dry-run --limit 20
hx-connector-t3code sync --since 2000-01-01T00:00:00Z
```

Set `HX_SERVER` and `HX_TOKEN` for delivery (or `--server` and `--token`).
`HX_INSTANCE` / `--instance` selects the installation namespace. The default
hashes the hostname and source location; set an explicit stable instance when
moving a source or changing hostnames. `HX_SOURCE` / `--source` overrides the
source database, directory, file, or Apple Bridge base URL. Git also accepts
repeated `--root` and `--depth` (default: home, depth three); discovery skips
Library, .Trash, .cache, node_modules, target, and .git directories and symlinks.
Apple Bridge defaults to its loopback API on port 7780, and uses
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
`--limit` bounds each batch. T3 and Codex consume older records without spending
the envelope limit on them; source scans also cap raw records at 100,000 per T3
scan or JSONL file to bound work when everything is excluded. Other sources may
emit fewer envelopes for metadata-only or excluded records. `--since` filters
source timestamps inclusively; records with unknown timestamps remain eligible.
Use a new instance for an independent historical backfill.

Codex cursor maps use versioned, lossless zstd compression (hex encoded in the
state JSON); legacy maps are read and upgraded on the next acknowledged batch.
Offsets, rotation anchors, and parsing context remain intact. File names and
mtime are not used as timestamp filters: rollouts may contain later appended
records or timestamps out of order.

Codex tails both plain and zstd rollouts, with offsets in decompressed bytes for
zstd. Claude tails project/subagent logs and the history index. Native identities
survive replay and rotation. History index entries and T3-linked provider records
carry `claim_scope` provenance labels; normalizers must use their `session`,
`same_as`, and `parent` links when reconciling evidence. Neither source proves
human attention. Agent strings, including nested native tool provenance, are
capped at 32 KiB per string and marked `text_truncated` if any string was capped.

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
