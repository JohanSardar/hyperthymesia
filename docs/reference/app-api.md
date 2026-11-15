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
     "appearance":"dark"
   }
   ```

   Use the catalog's efforts; null leaves effort to the model. `model:null`
   clears the choice. A new installation returns `model:null`. Explicit
   environment provider/model values are used only until preferences are first
   saved. Changes apply to the next agent run; in-flight work keeps its choice.
   The server does not change fx's own provider, model, or settings.
5. `POST /v1/model/check` makes one small request with the saved model/effort.
   This can cost money. It returns `{"ok":true,"message":"The selected model
   responded."}` or `ok:false` with a generic failure message. Missing selection
   is 400. No captured data, source tools, or personal project context is sent.
   Provider output and credentials are never returned. Checks use a disposable
   profile. Subscription checks share the agent runner's credential lock and
   save token refreshes to the existing login so it remains usable.

Subscription authentication uses the existing guided setup, which names the
chosen provider. This API never launches login or opens a system prompt.

## Settings

`GET /v1/settings` returns `settings` with the shape above,
`gateway_key_configured`, `storage` (`state_dir`, `published_blob_bytes`,
`automatic_gc:false`), and `api_keys` containing safe token metadata.

Mac recording is a server-wide control for paired Macs running this version.
The Mac polls `GET /v1/recording` with its ingest token every five seconds,
pauses content capture and drops new content submissions while disabled.
Health reporting and upload of previously queued records continue. Network
failures retain the last received setting. On app launch capture waits for the
first successful settings response. It does not revoke macOS permissions.
The local menu-bar pause remains independent.

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
Revoking the current owner token signs that client out immediately.

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
