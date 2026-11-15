# App model, Settings and Runs API

These JSON routes serve the upcoming React app. They accept the signed-in
`hx_token` cookie or an `Authorization: Bearer TOKEN` header. Settings require
an owner token with `read_raw` and all four namespace prefixes: `source:`,
`macos-agent:`, `macos-media:`, `companion:`. Restricted readers and device ingest
tokens cannot administer settings or keys. All responses use `Cache-Control:
no-store`. JSON bodies have a 16 KiB limit and errors do not echo request values.
Cookie-authenticated mutations require an `Origin` matching the request host;
programmatic clients can use a bearer token without an Origin header.

## Model setup

1. `GET /v1/models` returns `providers`, with IDs `vercel`, `codex`, `grok`, names
   and authentication methods. `selected` is null; nothing is preselected.
2. `GET /v1/models?provider=vercel` returns fx's `models` array, including each
   model's `id`, `name`, and `efforts`. It runs the pinned fx catalog command in
   an isolated profile. Subscription catalogs need an existing fx login. If the
   catalog cannot be read, `models` is empty and `error` is `catalog_unavailable`;
   the client can retry after sign-in or accept an explicit model ID. No model
   list is invented. `vercel` maps to fx's internal `gateway` request provider.
3. `PUT /v1/settings/gateway-key` with `{"key":"YOUR_KEY"}` saves the key in a
   0600 file under the state's 0700 `secrets/` folder. It returns 204, never the
   key. `DELETE` removes this stored key; an environment-supplied key remains
   configured until the owner removes it from the environment. Keys are omitted
   from backups, logs, model prompts and responses.
4. `PUT /v1/settings` replaces preferences. For example:

   ```json
   {
     "model": {"provider":"vercel","model":"anthropic/claude-sonnet-4.5","effort":null},
     "mac_recording":true,
     "chunk_retention_days":30,
     "appearance":"dark",
     "spend_limit_usd":null,
     "setup":{"step":"done","finished":true,"tour_pending":false}
   }
   ```

   Use the catalog's efforts; null leaves effort to the model. `model:null`
   clears the choice. A new installation returns `model:null`. Explicit
   environment provider/model values are used only until preferences are first
   saved. Changes apply to the next agent run; in-flight work keeps its choice.
   The server does not change fx's own provider, model, or settings.
5. `POST /v1/model/check` makes one small request with the saved model/effort.
   This can cost money. It returns `ok`, a plain `message`, `problem` (null or
   a code such as `signin`, `key`, `model`, `usage_limit`, `timeout`, `other`),
   `seconds`, and the `run_id` of the check, which is recorded as a `check` run.
   Missing selection is 400. No captured data, source tools, or personal project context is sent.
   Provider output and credentials are never returned. Checks use a disposable
   profile. Subscription checks share the agent runner's credential lock and
   save token refreshes to the existing login so it remains usable.

## Provider accounts and sign-in

`GET /v1/providers` returns one entry per provider (`codex`, `grok`, `vercel`)
with `method` (`subscription` or `key`), `signed_in` (an fx login file or a
saved key is present), `in_use`, the latest `last_check` and `last_run`
(state, problem, time), and the current `signin` state.

`POST /v1/providers/ID/signin` starts the provider's own login through
`fx login ID` on the server's computer (subscriptions only; a key provider is
400). The provider's page opens once in that computer's browser. `GET` returns
`{provider, state, url, signed_in}`, where `state` is `idle`, `waiting`, `done`
or `failed`; `done` means fx exited successfully and its login file exists. An
unfinished sign-in gives up after ten minutes. `DELETE` cancels it. If fx cannot
start, the response is 503 with a plain message. Passwords go only to the
provider; the server never reads or returns the login.

## Settings

`GET /v1/settings` returns `settings` with the shape above,
`gateway_key_configured`, `storage` (`state_dir`, `published_blob_bytes`,
`automatic_gc:false`), and `api_keys` containing safe token metadata.

Mac recording is a server-wide control for paired Macs running this version.
The Mac reads `GET /v1/recording` with its ingest token every five seconds,
pauses content capture and drops new content submissions while disabled.
Health reporting and upload of previously queued records continue. Network
failures retain the last received setting. On app launch capture waits for the
first successful settings response. It does not revoke macOS permissions.

## Pause and never-record

`GET /v1/recording` accepts the Mac's ingest token or an owner session/token
and returns:

```json
{
  "enabled": true,
  "paused_until": "2026-11-14T15:00:00Z",
  "never_record": {
    "apps": [{"bundle_id": "com.apple.TextEdit", "name": "TextEdit"}],
    "sites": ["bank.example"]
  },
  "changed_at": "2026-11-14T14:00:00Z",
  "notices": {"account_ready": {"start": "…", "end": "…"}, "failed_pass": 12, "failed_pull": {"id": 3, "recipe_id": "weather"}}
}
```

`paused_until` is `null` unless a pause is still running. `PUT
/v1/recording/pause` takes `{"until": time}` (at most eight days ahead) or
`{"until": null}` to resume. `PUT /v1/recording/never-record` replaces the
whole list with `{"apps": [...], "sites": [...]}`; sites may be pasted URLs and
are stored as lowercase hosts without `www.`, each also covering its
subdomains. Cookie-session writes need a same-origin request. Both answer with
the `GET` shape.

The Mac keeps its own copy, so a pause holds across restarts and while the
server is unreachable; a change made on the Mac is sent within about a second
and wins over the server's copy until the server confirms it. While paused,
the Mac records only content-free gaps. A never-record app or site gets an
excluded focus span with no app name, title, text, clicks or page address.
`notices` are content-free markers the Mac turns into desktop notifications.

`chunk_retention_days` accepts 1–3650 and sets the policy for chunk collection.
Automatic GC and the destructive GC CLI remain disabled; changing this policy
alone does not delete data. `appearance` accepts `system`, `light`, or `dark`
(default `dark`); the UI client reads this preference.

`GET /v1/settings/tokens` returns `tokens` with safe IDs (hashes), names,
namespace prefixes, scopes and revocation status. `POST` accepts:

```json
{"name":"My client","prefixes":["source:"],"scopes":["read_l3"]}
```

Creation returns 201 with `token` **once** and its `id`. Save it privately.
`DELETE /v1/settings/tokens/ID` revokes it, returning 204 (404 if absent).
`spend_limit_usd` is an optional monthly limit (0–100000, null for none, the
default). Once the month's recorded run cost reaches it, new passes wait until
the next month; a run already going finishes. Raising or removing the limit, or
changing the model, releases held passes. Subscription runs report $0.

`setup` records first-run progress for the app: `step` (lowercase letters and
`-`, at most 32), `finished`, and `tour_pending` (show the guided tour on the
next Today visit). It is null on installations set up before this release.

Revoking the current owner token signs that client out immediately.

## First-run setup

`GET /v1/setup` returns `app` (Mac app `installed`, `paired`, `paired_here`,
`server`, `running`), `permissions` (Accessibility, Screen Recording and input
monitoring as the Mac app last reported them, or null if older than 90 seconds),
`guide` (the live state of an in-app permission request), `pairing` (the
result of pairing this Mac with another computer) and `last_record_at`.

`POST /v1/setup/permissions` asks the Mac app to show its permission guide.
`POST /v1/setup/pair-elsewhere` with `{"link":"hyperthymesia://pair?..."}` asks
the Mac app to redeem a one-time link from another computer.
`GET /v1/setup/proof` returns up to three screen-text snapshots from the last
two minutes (app, window title, a few readable lines), so setup can show that
capture works. Nothing else is returned and no image is ever stored.

## Runs

`GET /v1/runs?limit=50` returns `passes` (four-hour interpretation jobs) and
`pulls` (persisted pull executions). Times are UTC microseconds. Arrays are in
newest ID order, bounded to 1–100 rows each. `next_pass_before` and
`next_pull_before` are independent cursors: send `pass_before=ID` and/or
`pull_before=ID` on the next request. `before=ID` sets both cursors for clients
reading only one array. Each array can be paged independently.

Passes expose job ID, kind, state, window start, generation, attempts, next
availability, lease, error presence, and the window's current revision ID.
Pulls expose ID, source and recipe IDs, start/finish, status, accepted,
duplicate and rejected counts. Pull history starts with this release; old
recipe state contains only its latest result and cannot reconstruct past runs.
Interrupted pulls remain `running` without a finish time; a recovered job adds
its next execution. Raw errors and provider output are withheld.

### AI runs

`GET /v1/agent-runs?limit=50&before=ID` returns `runs`, newest first: each
four-hour pass, connection setup and (with `kind=check`) model check, with
`state` (`running`, `done`, `failed`, `waiting`, `stopped`, `skipped`), start and
finish, provider, model, effort, `cost_usd` and token counts as fx reports them,
a plain `problem` code, and the `revision_id` it wrote. `upcoming` lists queued
passes with `available_at`, `catch_up` (the next-day re-read) and `held` (by the
spending limit). `month_spend_usd` and `spend_limit_usd` are included.

`GET /v1/agent-runs/ID` adds the step `log` (plain step names and record kinds,
never screen text or model output) and the underlying job. `POST
/v1/agent-runs/ID/rerun` queues the same window again with the current model;
an unchanged window and model is recorded as `skipped`.

## Day, timeline and search

All four need a token with `read_raw` and are never cached. Records and
accounts outside the token's namespace prefixes are left out.

`GET /v1/days` returns `buckets` (record, screen-text and focus counts per
15-minute UTC bucket, `at` in Unix seconds; clients group them into local days
with their own time zone rules) and `windows` (four-hour interpretation
windows with their canonical revision ID, or null).

`GET /v1/day?from=ISO&to=ISO` (at most 50 hours) returns per-collector
`coverage` (observed, unknown or suppressed), `lanes` (focus, presence and
attention spans), `points` (text points carry a 160-character preview),
per-minute input `activity` as counts only, `aggregates`, the canonical
`revisions` in range without their input manifest, and `windows`.

`GET /v1/cite/ID?revision=R` returns the stored `record`, `seen` (the exact
digest lines and snapshot the AI read in that revision's run, or
`unavailable`), and for spans up to 60 `screen_text` records captured during
it with `screen_text_total`.

`GET /v1/find?q=TEXT` returns `entries` (tasks, episodes and summaries from
canonical accounts where every term matches) and `records` (full-text matches
in recorded screen text and other records, with a snippet), each newest first
and capped at 100.
