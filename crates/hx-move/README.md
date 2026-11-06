# hx-move: owner-only Postgres → SQLite cutover

This crate copies the frozen migration-10 Postgres schema to a **new local state
folder**. It never applies migrations to the source. Connection startup sets
`default_transaction_read_only=on`; every source query uses one REPEATABLE READ,
READ ONLY transaction. Keep the server and all other writers stopped throughout.
The frozen migrations are byte-for-byte copies of `ffa59cb:crates/hx-store/migrations`.
Only the opt-in synthetic test applies them, in a newly created scratch database.

## Start Postgres if needed

Use the **existing** Postgres instance and volume for the actual move. Do not
initialize a fresh volume and mistake it for your history. If its container is
stopped, start that existing container with `docker start <existing-container>`.
Do not remove or reconfigure a running Postgres instance.

For an independent synthetic rehearsal, this replaces the removed root compose
file. Choose a free port; these commands create a new, disposable instance:

```sh
docker run -d --name hx-move-synthetic \
  -e POSTGRES_USER=hx -e POSTGRES_PASSWORD=hx -e POSTGRES_DB=hx \
  -p 127.0.0.1:55433:5432 postgres:17
# Wait until pg_isready reports accepting connections:
docker exec hx-move-synthetic pg_isready -U hx -d hx
HX_MOVE_TEST_POSTGRES=postgres://hx:hx@localhost:55433/hx \
  cargo test -p hx-move --test postgres -- --ignored
# The test creates and drops its own scratch database, even on failure.
docker rm -f hx-move-synthetic
```

If a test service already exists at `postgres://hx:hx@localhost:55432/hx`, use that
URL instead; do not restart or reconfigure it. The test's admin database must be
`hx` and its role must have CREATEDB. It generates a unique `hx_move_*` database,
uses synthetic rows only, and drops that database on success or a returned error.
Default `cargo test` ignores this test and needs no database server.

## Owner's cutover steps

1. Finish the independent review and synthetic rehearsal first. Identify the
   actual legacy database URL and legacy data folder (the one containing `blobs/`).
   Back up Postgres and those files using your existing backup procedure.
2. Confirm no interpretation is running, then stop the server and every other
   writer. The Mac outbox retains uploads while offline, but it is capped; budget
   the downtime and watch its capacity. The mover refuses an unexpired running
   lease, but this check cannot prove that an idle server has been stopped.
3. Build and run the mover. Choose an absent or **entirely empty** state folder on
   a local filesystem, separate from the legacy folder. No default is assumed:

   ```sh
   cargo build --release -p hx-move
   # Set HX_MOVE_POSTGRES_URL privately in your shell; do not paste it into logs.
   export HX_MOVE_POSTGRES_URL
   cargo run --release -p hx-move -- \
     --source-data-dir /absolute/path/to/legacy-data \
     --state-dir /absolute/path/to/new-state \
     --verify-all-blobs > /absolute/path/to/move-report.json
   ```

   `--postgres-url` can also be passed explicitly, and `HX_STATE_DIR` can replace
   `--state-dir`. Credentials are safer in the environment than process arguments.
   The report contains only counts and SHA-256 hashes, never source content.
   Error output is intentionally generic because database errors may contain data.
4. Require exit status zero and review the report. Run `hx-server backup` using
   the CLI available after P5 lands, before starting capture against SQLite.
   This P6 branch does not implement backup or change any launch configuration.
5. Repoint your server plist/config to `--state-dir /absolute/path/to/new-state`,
   then start the server. Verify citations and disclosure in the P7 owner runbook.
6. Keep the stopped Postgres database and legacy blobs untouched for at least
   30 days. There is no reverse mover: after SQLite acknowledges new uploads,
   reverting to Postgres would lose those uploads.

## What is verified and published

The mover stages `core.sqlite.moving`, an empty `agent.sqlite`, locks and blobs
under a private temporary sibling directory. It creates the SQLite schema before
bringing in blobs, preserving `Store::create`'s protection of blob-only folders.
Rows use explicit IDs, exact `jsonb::text`, integer epoch microseconds, JSON text
arrays and 20-digit watermark sequences. New job columns and revision dependency
records use the shared runtime helpers. Source sequence high-water values and
row maxima fence all new SQLite IDs. Overlapping evidence IDs are refused.

Every application table (including empty and derived tables and
`sqlite_sequence`) has an ordered, typed, length-prefixed SHA-256 and a row count.
Legacy/derived expected hashes are computed from the source, then compared with
SQLite readback. Source rows are rehashed in the same snapshot, and source
sequences are checked again after loading and verification. Schema metadata is validated by `Store::create/open`;
the generated FTS index is checked against its external content, rather than
hashed as an independent copy of the normalized rows.

Verification also runs SQLite foreign-key/integrity/FTS checks, resolves all
revision/entity citations through `Store::evidence`, compares a fixed-seed sample
of up to 1,000 evidence IDs plus first/last and **every OCR ID**, and checks up to
1,000 sampled search lexemes on both backends. Blob ordering in evidence JSON is
normalized for comparison; the rows and their stored text are never normalized.
Published blob sizes are all checked. By default the first blob and a fixed
approximately 1/64 sample are rehashed; `--verify-all-blobs` rehashes every one.
Unpublished/expired blob metadata stays intact without requiring deleted files.

Blobs are hard-linked, with copy fallback only for cross-volume links. The new
store's GC unlinks its own paths, preserving the old files. Neither store should
mutate blob bytes in place. All files/directories are synced before a final
single directory rename publishes the verified state. The published state directory
has owner-only permissions on Unix. A pre-existing core, any
other nonempty target, a symlink target, a changed source sequence, or failed
verification is refused. Ordinary errors remove staging automatically and leave
the target absent/empty for retry. A process kill or power loss can leave a hidden
`.hx-move-*` sibling; it is never opened as history and does not block retry. The
owner may remove that abandoned staging folder after confirming no mover runs.

OCR progress is seeded for every engine version already present in `ocr_jobs`.
All still/blob associations without existing derived evidence are retained;
failed/sampled associations remain dormant until reset. This includes unpublished
blobs, so a late upload is discovered after cutover. A new engine version starts
from zero, intentionally matching the runtime's versioned OCR behavior.

Migration 10 already applied the legacy admission repairs now represented by
`hx-store` test fixtures. The mover **does not replay** those data repairs: job
states, attempts, revival budgets, daily sweep reservations, generation clocks
and `revision24_finished` are copied exactly. Replaying repairs at move time
would change the paid-work budget. The synthetic CI contract checks that
`schedule()` creates or revives zero paid jobs on settled moved history.
