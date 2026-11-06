# Moving your history from Postgres to SQLite

You do this once, alone, on your real history. Read the whole page first. Nothing
here is automatic: you run each step and check it before the next one.

Every command is written for the Mac's default shell (zsh) and is safe to paste.
Nothing asks for a password. Never use `sudo`.

The point of no return is [step 7](#7-cut-over). Until then, your Postgres history is
untouched and going back takes one command.

## Before you start

**What this changes.** The server stops using Postgres and a data folder, and uses
one local state folder instead (`core.sqlite`, `agent.sqlite`, `blobs/`, `locks/`).
The service gets a new binary and new arguments.

**What this doesn't change.**

- Every evidence ID, revision, citation, job and paid-work counter stays the same.
- Device tokens are copied, so the Mac app keeps working without pairing again, on
  the same address.
- The mover never writes to Postgres. Its connection is read only, and it applies no
  migrations. Old blob files aren't changed: the new folder hard-links them.
- Spending and automatic gc: see [step 9](#9-after-the-move).

**How long it takes: unknown.** Nobody has timed this on your history. The mover
reads every row and hashes every published blob, so it grows with both. The only
honest number comes from a [timing run](#optional-timing-run) on your machine.

**The Mac keeps queueing while the server is down.** Leave the Mac app running. Its
outbox holds uploads and sends them when the server returns, but it's capped: by
default 64 MiB of events and 1 GiB of media. Stills alone are limited to 64 MiB an
hour, so media can fill in about 16 hours at the worst case, and sooner if other
media is flowing. When a budget fills, the outbox drops captures and records loss
events. That loss is permanent, so finish well inside that window, and use the timing
run if you're unsure.

**Disk space.** Check what you have after step 1 sets the variables:

```sh
df -h "$SUPPORT" "$MOVE_BACKUP"
du -sh "$LEGACY_DATA_DIR/blobs"
psql_hx "select pg_size_pretty(pg_database_size(current_database()))"
```

Plan for about **4 × the database size plus 3 × the blob folder** in total:

| What | Database | Blobs |
| --- | --- | --- |
| Dump and blob copy (step 2) | up to 1 × | 1 × |
| The moved state (step 5) | about 1 × | almost nothing, hard links |
| First SQLite backup (step 6) | about 1 × | 1 × |
| Throwaway check copy (step 6, deleted afterwards) | about 1 × | 1 × |

Your backups can go on another disk. Keep `NEW_STATE` on the same internal disk as the
old data: it must be a local disk, because SQLite's WAL doesn't work on network
filesystems, and hard links only work within one volume (across volumes it copies).

**Don't rebuild `.target-main`.** That's your only Postgres binary and the rollback
needs it. Today's source can't run on Postgres. This page builds into a different
folder.

## 1. Get ready (server still running)

Make a folder for everything this move keeps, then save your settings in it. Edit the
`<...>` values before you paste this.

```sh
export MOVE_BACKUP="<a folder with plenty of space, outside the data folder>"
mkdir -p "$MOVE_BACKUP" && chmod 700 "$MOVE_BACKUP"
cat > "$MOVE_BACKUP/vars.zsh" <<'EOF'
export MOVE_BACKUP="<the same folder as above>"
export REPO="<your Hyperthymesia checkout>"
export SUPPORT="<support>"
export BUILD_DIR="$REPO/.target-sqlite"
export LEGACY_BIN="$REPO/.target-main/release/hx-server"
export LEGACY_DATA_DIR="$SUPPORT/server-data"
export NEW_STATE="$SUPPORT/state"
export HX_BIN="$SUPPORT/bin/hx-server-sqlite"
export MOVE_BIN="$SUPPORT/bin/hx-move"
export SERVER_PLIST="$HOME/Library/LaunchAgents/com.hyperthymesia.server.plist"
export SAVED_PLIST="$MOVE_BACKUP/com.hyperthymesia.server.postgres.plist"
export LIVE_URL="http://127.0.0.1:7377"
export SPARE_PORT=7399
export POSTGRES_CONTAINER="<name from docker ps>"
export HX_MOVE_POSTGRES_URL="<the DATABASE_URL value in your plist>"
export PG_ROLE="<the role in that URL>"
export PG_DB="<the database name in that URL>"
psql_hx() { docker exec -i "$POSTGRES_CONTAINER" psql -X -At -U "$PG_ROLE" -d "$PG_DB" -c "$1"; }
EOF
chmod 600 "$MOVE_BACKUP/vars.zsh"
source "$MOVE_BACKUP/vars.zsh"
```

To find the values, use `docker ps --format '{{.Names}} {{.Ports}}'` and
`plutil -p "$HOME/Library/LaunchAgents/com.hyperthymesia.server.plist"`. If the plist
has no `DATABASE_URL`, the old server is using `postgres://hx:hx@localhost:55432/hx`.
If you open a new Terminal window later, run `source <that folder>/vars.zsh` first.
That file holds a database password, so don't share it.

Check that it points at your real history. This must print a large number:

```sh
psql_hx "select count(*) from events"
```

Save the service definition and the old binary, for the rollback:

```sh
cp -p "$SERVER_PLIST" "$SAVED_PLIST"
cp -p "$LEGACY_BIN" "$MOVE_BACKUP/hx-server-postgres"
plutil -p "$SAVED_PLIST"
```

Build the new binaries. A clean build took about 4 minutes on the test machine. Your
checkout must be on current `main`, and `.target-main` stays as it is:

```sh
cd "$REPO" && git switch main && git pull --ff-only
CARGO_TARGET_DIR="$BUILD_DIR" cargo build --release -p hx-server -p hx-move
mkdir -p "$SUPPORT/bin"
cp "$BUILD_DIR/release/hx-server" "$HX_BIN"
cp "$BUILD_DIR/release/hx-move" "$MOVE_BIN"
"$HX_BIN" --version && "$MOVE_BIN" --help > /dev/null && echo built
```

Then record what the old server shows, so you can compare it with the moved copy.
This creates two check tokens: one that sees everything and one limited to read_l3.

```sh
mkdir -p "$MOVE_BACKUP/check"
prefixes=()
for p in $(psql_hx "select distinct split_part(connector_instance,':',1)||':' from events"); do prefixes+=(--prefix "$p"); done
DATABASE_URL="$HX_MOVE_POSTGRES_URL" "$LEGACY_BIN" token create --name move-check-raw "${prefixes[@]}" --scopes read_l3,read_raw > "$MOVE_BACKUP/check/token-raw"
DATABASE_URL="$HX_MOVE_POSTGRES_URL" "$LEGACY_BIN" token create --name move-check-l3 "${prefixes[@]}" --scopes read_l3 > "$MOVE_BACKUP/check/token-l3"
export CHECK_REVISION="$(psql_hx "select canonical_revision from windows where canonical_revision is not null and end_at < now() - interval '2 days' order by start_at desc limit 1")"
export CHECK_DAY="$(psql_hx "select to_char(start_at at time zone 'UTC','YYYY-MM-DD') from windows where canonical_revision=$CHECK_REVISION")"
export CHECK_OCR="$(psql_hx "select id from ocr_evidence order by id desc limit 1")"
export CHECK_RESTRICTED="$(psql_hx "select id from events where sensitivity='restricted' and exists(select 1 from normalized n where n.event_id=events.id) order by id desc limit 1")"
export CHECK_WORD="$(psql_hx "select (regexp_match(search_text,'[[:alpha:]]{5,}'))[1] from normalized where event_id=$CHECK_RESTRICTED")"
cat >> "$MOVE_BACKUP/vars.zsh" <<EOF
export CHECK_REVISION="$CHECK_REVISION"
export CHECK_DAY="$CHECK_DAY"
export CHECK_OCR="$CHECK_OCR"
export CHECK_RESTRICTED="$CHECK_RESTRICTED"
export CHECK_WORD="$CHECK_WORD"
EOF
cat > "$MOVE_BACKUP/check/pages.sh" <<'EOF'
#!/bin/sh
# pages.sh <server url> <output folder>: saves the pages this runbook compares.
base=$1; out=$2; check=$MOVE_BACKUP/check; mkdir -p "$out"
raw="Authorization: Bearer $(cat "$check/token-raw")"
l3="Authorization: Bearer $(cat "$check/token-l3")"
curl -fsS -H "$raw" "$base/day/$CHECK_DAY" -o "$out/day.html" || echo "FAILED day page"
curl -fsS -H "$raw" "$base/revision/$CHECK_REVISION/run" -o "$out/run.html" || echo "FAILED run page"
ids=$(grep -Eo 'href="/evidence/[0-9]+"' "$out/day.html" | grep -Eo '[0-9]+' | sort -un | head -5)
for id in $ids $CHECK_OCR $CHECK_RESTRICTED; do
  curl -fsS -H "$raw" "$base/evidence/$id" -o "$out/evidence-$id.html" || echo "FAILED evidence $id"
done
curl -fsS -H "$raw" "$base/v1/search?q=$CHECK_WORD" -o "$out/search-raw.json" || echo "FAILED search"
curl -fsS -H "$l3" "$base/v1/search?q=$CHECK_WORD" -o "$out/search-l3.json" || echo "FAILED search"
curl -sS -o /dev/null -w '%{http_code}\n' -H "$l3" "$base/evidence/$CHECK_RESTRICTED" > "$out/restricted-as-l3.txt"
EOF
source "$MOVE_BACKUP/vars.zsh"
sh "$MOVE_BACKUP/check/pages.sh" "$LIVE_URL" "$MOVE_BACKUP/check/before"
ls "$MOVE_BACKUP/check/before"
```

You should see `day.html`, `run.html`, some `evidence-*.html`, both search files and
no `FAILED` lines. A `FAILED` line means stop and find out why. These files hold your
private data, so keep them in this folder. If you have no restricted items, you'll see
a `psql` syntax error from the `CHECK_WORD` line. That's harmless, and later you can
skip the restricted checks.

## 2. Back up first

Do this while the server still runs. It costs no downtime and tells you how long the
backups take.

```sh
time docker exec "$POSTGRES_CONTAINER" pg_dump --no-password -U "$PG_ROLE" -d "$PG_DB" --format=custom > "$MOVE_BACKUP/before-stop.dump"
echo "dump exit: $?"
time cp -R "$LEGACY_DATA_DIR/blobs" "$MOVE_BACKUP/blobs"
echo "copy exit: $?"
```

**Check they're good.** Both exits must be 0. Then restore the dump into a throwaway
database and compare the event count, and compare the blob copy byte for byte:

```sh
docker exec "$POSTGRES_CONTAINER" createdb -U "$PG_ROLE" hx_dump_check
docker exec -i "$POSTGRES_CONTAINER" pg_restore --no-owner --exit-on-error -U "$PG_ROLE" -d hx_dump_check < "$MOVE_BACKUP/before-stop.dump"
echo "restore exit: $?"
docker exec "$POSTGRES_CONTAINER" psql -X -At -U "$PG_ROLE" -d hx_dump_check -c "select count(*) from events"
docker exec "$POSTGRES_CONTAINER" dropdb -U "$PG_ROLE" hx_dump_check
psql_hx "select count(*) from events"
diff -rq "$LEGACY_DATA_DIR/blobs" "$MOVE_BACKUP/blobs" && echo "blob copy matches"
```

The restored count can be a little lower than the live one, because the server kept
ingesting. Files uploaded after the copy show up as `Only in`; any line saying `differ`
is a real problem. You'll top both up in step 4.

## 3. Stop the server

First, make sure no paid run would be killed. This must print 0. If it doesn't, wait:

```sh
psql_hx "select count(*) from jobs where kind='interpret' and state='running'"
```

Stop it, then move the plist aside. launchd loads whatever is in `LaunchAgents` at
login, so without this a restart or logout would bring the old server back mid-move.

```sh
launchctl bootout "gui/$(id -u)/com.hyperthymesia.server"
mv "$SERVER_PLIST" "$SERVER_PLIST.stopped"
```

**Confirm it's stopped.** All three must agree:

```sh
launchctl print "gui/$(id -u)/com.hyperthymesia.server"
lsof -nP -iTCP:7377 -sTCP:LISTEN
curl -sS -m 3 "$LIVE_URL/healthz"
```

`print` says it could not find the service, `lsof` prints nothing, and `curl` says it
couldn't connect. Close every other client of this database too, including SQL
consoles and database GUIs. From here on the Mac is queueing, and the clock from
"Before you start" is running.

## Optional: timing run

Do this if you don't know whether the move fits the outbox window. Stop the server as
in step 3, then run the real mover into a throwaway folder:

```sh
time caffeinate -i "$MOVE_BIN" --source-data-dir "$LEGACY_DATA_DIR" --state-dir "$SUPPORT/state-timing" > "$MOVE_BACKUP/timing-report.json"
```

Refusals here are useful: fix them now, while you have no deadline. Then remove the
throwaway copy and start the old server again. The Mac then uploads its queue:

```sh
rm -rf "$SUPPORT/state-timing"
mv "$SERVER_PLIST.stopped" "$SERVER_PLIST"
launchctl bootstrap "gui/$(id -u)" "$SERVER_PLIST"
curl -sS -m 3 "$LIVE_URL/healthz"
```

Your downtime on the real day is that time, plus step 4 (the same dump again, and a
short blob top-up), plus about as long again for the backup and check in step 6, plus
the comparison itself. Come back to step 3 when you're ready.

## 4. Top up the backups

The server is stopped, so this dump is the one that counts:

```sh
docker exec "$POSTGRES_CONTAINER" pg_dump --no-password -U "$PG_ROLE" -d "$PG_DB" --format=custom > "$MOVE_BACKUP/final.dump"
echo "dump exit: $?"
docker exec -i "$POSTGRES_CONTAINER" pg_restore --list < "$MOVE_BACKUP/final.dump" | grep -c 'TABLE DATA'
rsync -a "$LEGACY_DATA_DIR/blobs/" "$MOVE_BACKUP/blobs/"
diff -rq "$LEGACY_DATA_DIR/blobs" "$MOVE_BACKUP/blobs" && echo "blob copy matches"
```

The dump exit must be 0, the count must be above 0, and it must end with `blob copy
matches`. The `diff` reads every blob again, so it counts against your downtime.

## 5. Run the mover

```sh
time caffeinate -i "$MOVE_BIN" --source-data-dir "$LEGACY_DATA_DIR" --state-dir "$NEW_STATE" > "$MOVE_BACKUP/move-report.json"
echo "mover exit: $?"
```

**What success looks like:**

- `mover exit: 0` and nothing printed by the mover itself.
- `$NEW_STATE` holds `core.sqlite`, `agent.sqlite`, `blobs/` and `locks/`, and no
  `core.sqlite.moving`.
- `move-report.json` has only counts and hashes. Look at it:

```sh
ls "$NEW_STATE"
python3 -c "import json,os;r=json.load(open(os.environ['MOVE_BACKUP']+'/move-report.json'));print({k:v for k,v in r.items() if k!='tables'});print({k:v['rows'] for k,v in r['tables'].items()})"
psql_hx "select (select count(*) from events), (select count(*) from revisions), (select count(*) from blobs where published)"
```

`events` and `revisions` must equal the Postgres counts on the last line, and
`blobs_checked` must equal the published blob count. The tables that are new in SQLite
(`sources`, `recipes`, `pair_codes` and so on) correctly show 0. `search_checked` can be
lower than `evidence_checked`, because rows without a plain ASCII word aren't sampled;
the full index check still covers them.

### If it refuses

Any refusal prints `Move refused: <reason>` and exits non-zero. Nothing in Postgres has
changed, and nothing is published to the target, so you can fix the cause and run the
same command again. The reason is safe to share: passwords and row contents are removed.

| It says | What it means | What to do |
| --- | --- | --- |
| `published blob verification failed (N problem(s)): <sha>: ...` | Published files are missing, or have the wrong size or hash. Every bad file is listed at once. | **There is no override.** See below. |
| `source database has N other client session(s)` | Something else is connected, even if idle: the server, a SQL console, a GUI. | Step 3 again. To see what, run `psql_hx "select application_name,state,client_addr from pg_stat_activity where datname=current_database() and pid<>pg_backend_pid()"`. Close it and retry. |
| `source has a live lease` | A job was running when the server stopped, and its lease hasn't expired. | Confirm the server is stopped, wait 2 minutes (leases last 120 s) and retry. |
| `target must be empty; refusing an existing core or other files` | `$NEW_STATE` already has something in it. | Don't delete anything you haven't looked at. If it's a failed try of yours, remove it, or pick a new `NEW_STATE`. If it's a finished move and the server has since run on it, stop: see [Roll back](#8-roll-back). |
| `target must be a directory, not a symlink`, `target unavailable` | `$NEW_STATE` is a file or symlink, or can't be reached. | Use a real path on the local disk. |
| `source has zero events; pass --allow-empty ...` | This database has no history. | You almost certainly have the wrong database. Fix `HX_MOVE_POSTGRES_URL`. Don't use `--allow-empty` for this move. |
| `source migration version mismatch`, `source migration mismatch` | The database isn't at the exact frozen migration 10. | Wrong database, or the old server is older than the mover expects. Check the URL. Don't run migrations by hand. Stay on Postgres and open an issue. |
| `unknown source table ...; refusing schema drift`, `source table ... has unexpected or missing columns`, `source schema has missing tables` | The tables aren't what the mover knows. | Same as above. An empty or unrelated database fails this way too. |
| `overlapping evidence IDs`, `watermark sequence outside u64 range` | Stored data breaks a rule the new store depends on. | Don't fix it by hand. Stay on Postgres and open an issue, with the message only. |
| `source sequences changed during move`, `source row hashes changed` | Something wrote to Postgres during the move. | Something is still running. Step 3 again, then retry. |
| `... row count or hash differs`, `independent source audit: ...`, `sampled evidence differs`, `search sample did not resolve ...`, `unresolved citation`, `foreign key check failed`, `integrity check failed` | Verification found that the copy isn't faithful. It wasn't published. | Don't retry in a loop. Save the message, stay on Postgres and open an issue. |
| `the following required arguments were not provided: --postgres-url` | Your settings aren't loaded in this window. | `source` your `vars.zsh` and retry. |
| A connection error | Postgres isn't reachable. | Check Docker is running and `psql_hx "select 1"` works. If the container stopped, use `docker start "$POSTGRES_CONTAINER"`. Never create a new container or volume: that wouldn't be your history. |

**Missing published files.** The database says the bytes exist and the file isn't
there, or doesn't match. A crash during an old gc can cause this. The mover never drops
history silently, so it refuses, and no flag changes that. In order, try:

1. Look in your blob copy, and any older backups, for the file named by the SHA:
   `ls "$MOVE_BACKUP/blobs/<first two characters>/<sha>"`. If it's there and
   `shasum -a 256` of it gives that same SHA, copy it to the same path under
   `$LEGACY_DATA_DIR/blobs/`, then retry. If your step 2 copy has it but the folder
   doesn't, something removed it recently, so find out what.
2. If the Mac might still have it, restart the old server (the commands are in the
   timing run) and let the Mac upload. A re-upload restores a missing file.
3. If the bytes are really gone, stop and stay on Postgres. Don't edit the `blobs` table
   to get past this: that rewrites your record of what existed. Open an issue with the
   count and the message, and decide then.

**A leftover `.hx-move-*` folder** next to `$NEW_STATE` means a mover was killed or lost
power. It's never read as history and doesn't block a retry. Once you've confirmed no
mover is running, you can delete it.

## 6. Check the moved copy, before changing anything

`serve` isn't a passive viewer: it starts the scheduler and OCR, and can run jobs. So
**don't serve `$NEW_STATE` for this check.** Back it up, restore that backup to a
throwaway folder, and serve the throwaway with a provider that can't spend. This also
proves your first backup restores.

```sh
"$HX_BIN" --state-dir "$NEW_STATE" backup --to "$MOVE_BACKUP/sqlite-first"
echo "backup exit: $?"
"$HX_BIN" --state-dir "$SUPPORT/state-check" restore --from "$MOVE_BACKUP/sqlite-first"
echo "restore exit: $?"
export CHECK_DISCLOSURE="$(plutil -extract EnvironmentVariables.HX_DISCLOSURE raw "$SAVED_PLIST" 2>/dev/null)"
export CHECK_EGRESS="$(plutil -extract EnvironmentVariables.HX_EGRESS_SENSITIVE raw "$SAVED_PLIST" 2>/dev/null)"
nohup env -i PATH=/usr/bin:/bin HOME="$SUPPORT/state-check" HX_PROVIDER=fx HX_FX_BINARY=/usr/bin/false ${CHECK_DISCLOSURE:+HX_DISCLOSURE=$CHECK_DISCLOSURE} ${CHECK_EGRESS:+HX_EGRESS_SENSITIVE=$CHECK_EGRESS} "$HX_BIN" --state-dir "$SUPPORT/state-check" serve --listen "127.0.0.1:$SPARE_PORT" > "$MOVE_BACKUP/check/serve.log" 2>&1 &
sleep 3; curl -sS -m 3 "http://127.0.0.1:$SPARE_PORT/healthz"
```

The last line must print `ok`. Now compare the moved copy with what the old server
showed:

```sh
sh "$MOVE_BACKUP/check/pages.sh" "http://127.0.0.1:$SPARE_PORT" "$MOVE_BACKUP/check/after"
diff -r "$MOVE_BACKUP/check/before" "$MOVE_BACKUP/check/after" && echo "PAGES IDENTICAL"
cat "$MOVE_BACKUP/check/after/restricted-as-l3.txt"
grep -c "\"id\":$CHECK_RESTRICTED," "$MOVE_BACKUP/check/after/search-raw.json"
grep -c "\"id\":$CHECK_RESTRICTED," "$MOVE_BACKUP/check/after/search-l3.json"
```

**Must be true before you go on:**

- No `FAILED` lines, and the `diff` prints `PAGES IDENTICAL` for the day page, the run
  page and every evidence page, including the OCR-derived ID `$CHECK_OCR`.
- `restricted-as-l3.txt` says `403`. The full-access search finds the restricted item
  (count 1) and the read_l3 search doesn't (count 0).
- Search can differ slightly. SQLite splits punctuated terms differently, so it may
  find **more** matches than Postgres did. If `diff` only reports `search-*.json` and
  the results are a superset, that alone isn't a failure. A missing result is.

Any difference in a page is a failure: go to [Roll back](#8-roll-back), which at
this point costs nothing.

Then look with your own eyes. Open `http://127.0.0.1:7399/login` in a **private
window**, because cookies aren't separated by port and this token would replace your
normal login. Sign in with the token in `$MOVE_BACKUP/check/token-raw`, then:

1. Open `/day/<CHECK_DAY>` and click a citation. It opens its evidence page.
2. Open `/revision/<CHECK_REVISION>/run`, then an evidence link on it. The tool calls
   are there.
3. Open `/evidence/<CHECK_OCR>`. The OCR text is there.
4. Pick a day you remember and read it. It should be your day.

Then stop the check server and remove the throwaway copy. Keep `sqlite-first`.

```sh
pkill -f "state-dir $SUPPORT/state-check serve"
sleep 2
pgrep -fl "state-dir $SUPPORT/state-check" || echo "check server stopped"
rm -rf "$SUPPORT/state-check"
```

## 7. Cut over

**This is the point of no return.** Within seconds of starting, the Mac sends its queue
and deletes what the server acknowledged. From the first ingest, rolling back loses that
data. Only go on if step 6 passed completely.

Write the new service definition. The old arguments were `serve --listen
127.0.0.1:7377 --data-dir "<support>/server-data"` with the binary at
`.target-main/release/hx-server`. The new ones keep the same address, so nothing on the
Mac changes:

```sh
cp -p "$SAVED_PLIST" "$SERVER_PLIST"
rm -f "$SERVER_PLIST.stopped"
plutil -replace ProgramArguments -json "[\"$HX_BIN\",\"--state-dir\",\"$NEW_STATE\",\"serve\",\"--listen\",\"127.0.0.1:7377\"]" "$SERVER_PLIST"
plutil -remove EnvironmentVariables.DATABASE_URL "$SERVER_PLIST"
plutil -lint "$SERVER_PLIST"
plutil -p "$SERVER_PLIST"
```

Read that output. `ProgramArguments` should show the new binary and arguments, and
`DATABASE_URL` should be gone. Keep everything else, in particular `HX_PROVIDER`,
`HX_DISCLOSURE` and `HX_EGRESS_SENSITIVE`, because a changed policy hides old run pages.

Note the current numbers, then start it:

```sh
sqlite3 -readonly "$NEW_STATE/core.sqlite" "select count(*) from events" > "$MOVE_BACKUP/events-at-cutover.txt"
cat "$MOVE_BACKUP/events-at-cutover.txt"
launchctl bootstrap "gui/$(id -u)" "$SERVER_PLIST"
launchctl kickstart "gui/$(id -u)/com.hyperthymesia.server"
sleep 3
launchctl print "gui/$(id -u)/com.hyperthymesia.server" | grep -E 'state =|program =|last exit'
curl -sS -m 3 "$LIVE_URL/healthz"
```

It should say `state = running` and `ok`. Then open a citation you know in your normal
dashboard. After a minute, confirm the Mac is delivering. This number should grow past
the one you noted:

```sh
sqlite3 -readonly "$NEW_STATE/core.sqlite" "select count(*) from events"
```

## 8. Roll back

**Before the first ingest,** rollback loses nothing. Compare the event count with
`events-at-cutover.txt`. If you haven't cut over, or the count hasn't moved:

```sh
launchctl bootout "gui/$(id -u)/com.hyperthymesia.server"
cp -p "$SAVED_PLIST" "$SERVER_PLIST"
rm -f "$SERVER_PLIST.stopped"
launchctl bootstrap "gui/$(id -u)" "$SERVER_PLIST"
launchctl kickstart "gui/$(id -u)/com.hyperthymesia.server"
sleep 3
curl -sS -m 3 "$LIVE_URL/healthz"
```

If you're still before step 7, the service is already stopped, so `bootout` just reports
that it found nothing. That's fine. Postgres and its blobs are as you left them. Keep
`$NEW_STATE` for looking into what went wrong, and to move again later, use a new empty
folder.

**After new data arrives, rolling back loses that data.** Anything SQLite accepted isn't
in Postgres, the Mac has already deleted it from its outbox, and **there's no reverse
mover**. Whatever the count shows above `events-at-cutover.txt` would be gone, along
with any revisions the new server has bought. That's why step 6 must pass before the
first ingest. Once data has arrived, prefer fixing forward on SQLite, and treat rollback
as a last resort you've decided on knowing the loss.

## 9. After the move

**Take a backup now, while it's running.** Each completed backup needs a new, separate
folder, and it copies every blob, so check the space. It's online: the server keeps
ingesting. `sqlite-first` is your pre-cutover restore point, and this is the first with
live data.

```sh
"$HX_BIN" --state-dir "$NEW_STATE" backup --to "$MOVE_BACKUP/sqlite-after-cutover"
echo "backup exit: $?"
```

To restore one, stop the service and run `"$HX_BIN" --state-dir <new empty folder>
restore --from <backup>`, then `chmod 700` that folder. Restore doesn't keep the
backup's owner-only permissions.

**Watch it for a day.** These are safe while it runs:

```sh
curl -sS -m 3 "$LIVE_URL/healthz"
sqlite3 -readonly "$NEW_STATE/core.sqlite" "select kind,state,count(*) from jobs group by 1,2"
sqlite3 -readonly "$NEW_STATE/core.sqlite" "select status,count(*) from ocr_jobs group by 1"
tail -n 50 "$(plutil -extract StandardErrorPath raw "$SERVER_PLIST")"
du -sh "$NEW_STATE"
```

- Today's day page fills in, and new evidence gets citations after the next 4-hour window.
- Stuck work: a `running` interpret job should change within a run's length, and `failed`
  jobs shouldn't climb. After a crash, a lease recovers on its own in about 2 minutes.
- The log: `WARNING: automatic blob GC is off` is expected on every start. `database is
  locked`, `scheduler failed` or repeated `processing failed` aren't.
- The Mac app's queue drains, and no new loss events appear after that.
- Disk grows steadily, because nothing deletes blobs.

**Spending doesn't stop because you moved.** Recent windows and the daily sweep still buy
what the old server would have bought, and it starts again as soon as the server does.
Queued `revision24` jobs stay ready and will run when due. To see how many:

```sh
sqlite3 -readonly "$NEW_STATE/core.sqlite" "select count(*) from jobs where kind='interpret' and state='ready' and json_extract(payload,'$.revision24')=1"
```

Settled history doesn't create extra paid work just by moving: attempts, revivals, sweep
reservations and `revision24_finished` are copied exactly.

**Automatic gc stays off.** `serve` doesn't schedule it, and moving doesn't change that.
Turning it on is a separate decision, and its first run permanently deletes every chunk
over 30 days old. See [gc-safety.md](gc-safety.md). Don't run `hx-server gc` as part of
this move.

**When you're happy,** revoke the two check tokens. Use the IDs of `move-check-raw` and
`move-check-l3`:

```sh
"$HX_BIN" --state-dir "$NEW_STATE" token list
"$HX_BIN" --state-dir "$NEW_STATE" token revoke "PASTE_THE_ID_HERE"
```

The `check/` folder holds copies of your pages, so delete it once you're done comparing.

## 10. Deleting Postgres and the old blobs

Don't hurry. This space is cheap compared with your history, and nothing here gets
safer by being quick. Keep Postgres and the old data folder untouched for **at least 30
days**, and until all of these are true:

- The server has run on SQLite for those 30 days with no sign of a problem.
- You've taken a recent `hx-server backup`, restored it to a throwaway folder and opened
  a known citation from it.
- You've decided you'll never roll back, knowing it would already lose 30 days.

Then, one at a time and with a pause between them:

1. Remove the old blobs. The new state has its own links to the same bytes, so deleting
   the old names doesn't delete them. First check that one file really is linked: `ls -li`
   on a blob in each folder should show the same number, and a link count of 2 (or 1 if
   the mover had to copy across volumes).
2. Drop only your history database, and only with a fresh dump kept elsewhere:
   `docker exec "$POSTGRES_CONTAINER" dropdb -U "$PG_ROLE" "$PG_DB"`. Don't remove the
   container's volume unless nothing else lives on it.
3. Much later, delete `$MOVE_BACKUP`, including its dumps, blob copy and old binary.
