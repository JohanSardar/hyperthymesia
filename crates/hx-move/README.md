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
  cargo test -p hx-move -- --ignored
# The test creates and drops its own scratch database, even on failure.
docker rm -f hx-move-synthetic
```

If a test service already exists at `postgres://hx:hx@localhost:55432/hx`, use that
URL instead; do not restart or reconfigure it. The test's admin database must be
`hx` and its role must have CREATEDB. It generates a unique `hx_move_*` database,
uses synthetic rows only, and drops that database on success, returned errors, or
test panics.
Default `cargo test` ignores this test and needs no database server.

## Owner's cutover steps

These are instructions for the owner, not an automated migration script. Set the
variables below yourself to the intended database, files and folders; no real
paths or defaults are provided. Keep a copy of the original server plist and the
legacy server executable for rollback. Do not put credentials in shared logs.

1. Finish the independent review and synthetic rehearsal. Identify the actual
   legacy database and data folder containing `blobs/`. Use the existing database
   and volume, not the synthetic instance above.
2. Confirm no interpretation is running. Stop the Mac server LaunchAgent in the
   **user domain** so launchd cannot restart it (never use sudo):

   ```sh
   launchctl bootout "gui/$(id -u)/com.hyperthymesia.server"
   launchctl print "gui/$(id -u)/com.hyperthymesia.server"
   ```

   Confirm `print` says the service could not be found. Stop any manually started
   server and close every other client of this database, including SQL consoles.
   The mover refuses other client sessions at startup and at its final fence,
   even idle sessions that allocate no IDs. It also refuses live job leases.
   The Mac outbox retains uploads while offline, but it is capped; budget the
   downtime and watch its capacity.
3. Before moving, save a Postgres dump and copy the legacy blobs while all writers
   remain stopped. Set `HX_MOVE_POSTGRES_URL`, `PG_DUMP_FILE`, `LEGACY_DATA_DIR`,
   and an absent `LEGACY_BLOB_BACKUP` privately in your shell. With PostgreSQL
   client tools installed:

   ```sh
   pg_dump --no-password --dbname "${HX_MOVE_POSTGRES_URL:?set the legacy database URL}" \
     --format=custom --file "${PG_DUMP_FILE:?set the dump filename}"
   pg_restore --list "${PG_DUMP_FILE:?}" > /dev/null
   cp -R "${LEGACY_DATA_DIR:?set the legacy data folder}/blobs" \
     "${LEGACY_BLOB_BACKUP:?set an absent backup folder}"
   ```

   If the client tools are only inside the existing Docker container, use this
   dump command instead, with that container's database role/name:

   ```sh
   docker exec "${POSTGRES_CONTAINER:?set the existing container name}" \
     pg_dump --no-password -U "${PGUSER:?set the database role}" \
     -d "${PGDATABASE:?set the legacy database name}" --format=custom \
     > "${PG_DUMP_FILE:?set the dump filename}"
   ```

   Check the dump command exited zero; retain the dump and blob copy separately.
4. Build and run the mover. Set `NEW_STATE` to an absent or **entirely empty**
   folder on a local filesystem, separate from the legacy data. Set `MOVE_REPORT`
   to the report filename. Run:

   ```sh
   cargo build --release -p hx-move
   export HX_MOVE_POSTGRES_URL
   cargo run --release -p hx-move -- \
     --source-data-dir "${LEGACY_DATA_DIR:?}" \
     --state-dir "${NEW_STATE:?set an absent or empty state folder}" \
     --verify-all-blobs > "${MOVE_REPORT:?set the report filename}"
   ```

   `--postgres-url` is also accepted; credentials are safer in the environment
   than in process arguments. `HX_STATE_DIR` can replace `--state-dir`.
   Zero-event sources are refused unless you explicitly pass `--allow-empty`;
   investigate an unexpected empty history before using that flag.
5. Require exit status zero and review the report's table counts and hashes.
   Errors print the reason, with connection passwords and database row details
   removed. Every missing, wrong-size or wrong-hash published blob is listed by
   SHA together; the move still refuses, with no override. Decide what to do
   with the legacy install separately, then retry. A GC crash may have left
   published metadata without a file; the mover never silently drops it.
   Back up the successfully moved SQLite store **before** starting capture:

   ```sh
   cargo run --release -p hx-server -- --state-dir "${NEW_STATE:?}" \
     backup --to "${SQLITE_BACKUP_DIR:?set a separate backup folder}"
   ```

6. Repoint the saved server configuration to the SQLite binary and
   `--state-dir` set to `NEW_STATE`. Set `SERVER_PLIST` to the intended plist
   filename, then load it in the user domain and confirm its state:

   ```sh
   launchctl bootstrap "gui/$(id -u)" "${SERVER_PLIST:?set the server plist filename}"
   launchctl kickstart "gui/$(id -u)/com.hyperthymesia.server"
   launchctl print "gui/$(id -u)/com.hyperthymesia.server"
   ```

   Open a known existing citation in your normal client and confirm its evidence
   still resolves. Check that restricted evidence stays hidden and namespace
   limits still hold. Normal scheduling continues: recent windows and the daily sweep may buy the work the old server
   would have bought; queued `revision24` jobs remain ready. Settled history must
   not create extra paid work merely because it moved.
7. Keep Postgres and legacy blobs untouched for at least 30 days. To roll back
   **before SQLite has acknowledged new uploads**, boot out the SQLite service
   with the stop command above, restore your saved legacy plist/configuration
   and legacy server executable, and start the old server again:

   ```sh
   launchctl bootstrap "gui/$(id -u)" "${SERVER_PLIST:?set the restored legacy plist filename}"
   launchctl kickstart "gui/$(id -u)/com.hyperthymesia.server"
   launchctl print "gui/$(id -u)/com.hyperthymesia.server"
   ```

   Ensure the existing Postgres container is running as described above. There
   is no reverse mover: reverting after new SQLite uploads would lose those
   uploads. The mover does not edit launch configuration or stop/start services.

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
SQLite readback. An independent SQL audit also computes source row counts and
keyed per-column aggregates on Postgres itself, without using the loader projection or decoder.
SQLite readback must match these aggregates. JSONB uses the server's own text/key
order deliberately, rather than re-serializing JSON in Rust. Source rows are
rehashed in the same snapshot; source sequences and other client sessions are
checked again after verification. Unknown source tables or columns are refused.
Schema metadata is validated by `Store::create/open`;
the generated FTS index is checked against its external content, rather than
hashed as an independent copy of the normalized rows.

Verification also runs SQLite foreign-key/integrity/FTS checks, resolves all
revision/entity citations through `Store::evidence`, compares a fixed-seed sample
of up to 1,000 evidence IDs plus first/last and **every OCR ID**, and checks up to
1,000 sampled search rows on both backends, using whole ASCII words from the
original text. URL-only and Unicode-only rows without such a word are skipped;
the full FTS integrity check still covers their index entries. Blob ordering in
evidence JSON is normalized for comparison; the rows and their stored text are never normalized.
Every published blob is checked for existence, size and SHA-256. All problems
are collected before refusal. `--verify-all-blobs` remains accepted for
compatibility; full hashing now happens even without it, so allow time to read
all published bytes.
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

`pair_codes` is SQLite-only and starts empty; existing `device_tokens`, including
revocation, prefixes and scopes, are copied and audited. No device secrets are
printed. The synthetic rehearsal verifies an existing token still authenticates.
