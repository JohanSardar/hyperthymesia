# Unknowns for connectors and providers (#31)

Checked 2026-11-05 on macOS with fx 0.0.16 (build `481104fd`, the `v0.0.16` tag) and gws 0.22.5.
Nothing used a real login, real data or a paid model. Probes ran under empty temp HOMEs, with made-up
logins, a local stub model and a throwaway Postgres. The stub relies on fx's own test hooks
(`FX_E2E_*`), which are for testing only, never product code. Linux was not run.

**Ran** means run here. **Source** means read in code. **Docs** means stated by the vendor.

| Question | Answer |
| --- | --- |
| S1 Make the fx provider configurable | **PASS**: a config change, not a broker rewrite. Real logins other than Codex: **BLOCKED** |
| S2 Fake email is citable today | **FAIL**: the model never sees it |
| S2 Mid-run record makes publish refuse the run | **PASS**: confirmed |
| S3 gws with no desktop session | **BLOCKED** on a real refresh; everything else below is established |
| Broker tool taking 180 seconds | **FAIL** at today's 15 s setting; **PASS** once the setting is raised |

## S1. fx logins per provider

| Provider | `FX_PROVIDER` | Where the login lives | Fake login with headless `fx ask --json` under a clean HOME |
| --- | --- | --- | --- |
| Codex | `codex` | `~/.fx/chatgpt-auth.json` | **Ran**: works. An expired one refreshes and the new token is saved to the file |
| Grok | `grok` | `~/.fx/grok-auth.json` | **Ran**: works, including refresh |
| Vercel AI Gateway | `gateway` | Checked in order: `VERCEL_OIDC_TOKEN`, `AI_GATEWAY_API_KEY`, `~/.fx/auth.json`, stored key | **Ran**: `AI_GATEWAY_API_KEY` works. `FX_AI_GATEWAY_API_KEY` is ignored. The rest is **Source** only |
| Your own endpoint | its name in `settings.json` | `providers.<name>.auth`: none, or a bearer token from a named env var | **Ran** with no auth. Bearer is **Source** only |

- **Codex and Grok have no env var for the login.** They only read the file. It must be a regular
  file with mode 600 and one hard link. **Ran**: mode 644 and a symlink both fail with
  `CredentialStorageUnavailable`.
- **The wrong file fails loudly.** **Ran**: a Grok file with `FX_PROVIDER=codex` gives
  `MissingCredentials` and never falls back to another provider. Codex and Grok also need a model set.
- **A rejected refresh deletes the temp copy.** **Ran**: fx removed the login file from the temp HOME.
  **Source**: `CredentialLease` then fails to save and leaves the source file alone (`fx.rs:102-106`).
- **macOS Vercel logins are in the Keychain.** **Source**: `fx login` stores there unless
  `FX_DISABLE_KEYCHAIN=1`. Hyperthymesia always sets that, so its runs can't see a Keychain login or a
  key from `fx setup`.
- **Only documented or inferred, not confirmed:** real logins for Grok and Gateway; whether
  `fx login` can finish on a headless VPS; whether fx's usage-limit text (`fx.rs:211,340`) matches
  other providers; which `--effort` values each model takes. `--fast` applies "when the model supports it".

**Decision for #35.** Make it a config change:

- Replace the hardcoded values at `fx.rs:22,236,250-251,272-273,299-302,310` and the two manifests
  (`fx.rs:375,595-596`) with provider, model and effort settings.
- Keep `CredentialLease`. Its `.lock` naming already matches fx's for all three login files.
- Add one small mapping: provider to login filename, or to an env var passed through `env_clear`.
- Installer: fetch release `v0.0.16` (Mac and Linux builds with checksums exist), because
  `fx.rs:288-291` refuses any other version.
- Vercel login: use an API key, or log in with `FX_DISABLE_KEYCHAIN=1` so the login lands in a file.

## S2. Fake email through the pipeline

**Ran** with envelope kind `email.message`, with the real code and a verbatim copy of the private
`citation_support`.

| Stage | Point record | Record with start and end |
| --- | --- | --- |
| Ingest | Accepted, but stored as `restricted` though labelled normal (`hx-store/src/lib.rs:132-136`) | Same |
| Normalize | Not quarantined. `search_text` is empty, so `store.search` finds nothing, even with raw access | Same |
| Compile | No lane. One point labelled only `email.message`, without the subject | Same point. The end time isn't in the timeline |
| Digest | Citable internally, but **not printed**. The model only finds it by spending one of 6 lookups | Same |
| Citation support | The exact instant only | The instant and the declared start to end |
| Checker | Passes only with a zero-length interval matching to the millisecond. Any real duration or rounded second is rejected | Passes on exactly start to end. A part of the span is rejected |

Other results:

- A `completed` claim on an email is downgraded to `attempted`.
- The broker's own `search` does find these records, because it scans the JSON and ignores the
  `restricted` column. So the model can read what the HTTP API hides without raw access.
- Records with no device add no coverage spans.
- Core kinds such as calendar and shell are also missing from the digest text, so mapping email to a
  core kind would not fix this.

**Smallest change.** Change `build_digest` only and leave the checker alone: for records of a non-core
kind, print `[id] start-end kind | short payload` and register that line as support. A point must print
as its **whole second plus one** (`13:10:00-13:10:01`), because `fx.rs:473` drops zero-length digest
support. **Ran** with a temporary 33-line patch, since reverted: all emails appeared in the digest, and
point, millisecond and span records all passed the checker citing their printed lines. Filling
`search_text` is a separate fix for API search and isn't needed for citations.

**Mid-run fence: confirmed (Ran).** A record ingested mid-run moved the generation from 1 to 2. Publishing
at the old generation returned `published=false` and wrote no revision. Publishing at the new one worked.

| Record's date, relative to the running window | Run |
| --- | --- |
| Inside the window | **Invalidated** |
| Previous window, within 30 minutes of the start | **Invalidated** |
| Previous window, over 30 minutes before the start | Still valid |
| A week older, or the next window | Still valid |

**Source:** every ingest also bumps the record's own window and queues its compile, and compile queues
new agent runs for that window (`hx-server/src/processing.rs:378-393`). So an old live answer stored
through `ingest` can trigger paid reruns of old windows.

**Decisions for #34.**

- Don't save live answers through `ingest`. Use a separate store path tied to the run, which skips the
  generation bump.
- Add a checker rule for records from another time. **Ran**: an old record alone is rejected, but
  citing an old record plus an in-window focus record, with the claim resting only on the old one, is
  **kept as Observed**. That breaks "context, not proof".
- Apply the digest change above.

**Decision for #35.** None.

## S3. gws without a desktop session

Established:

- **Login:** browser OAuth. It prints a URL and waits for a callback on a random local port. It needs a
  Desktop-type OAuth client (`client_secret.json` or env vars), or `gcloud` for `gws auth setup`. **Docs,
  plus strings in the binary.** No no-browser flag appears in `gws auth login --help`.
- **Headless VPS:** the documented route is to log in on a machine with a browser, run
  `gws auth export --unmasked`, copy the file over and set `GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE`.
  **Docs.** Logging in over an SSH tunnel to the printed port is inferred and untested. Service accounts
  are documented, but none was tested.
- **Tokens:** in `~/.config/gws` or `GOOGLE_WORKSPACE_CLI_CONFIG_DIR`: `credentials.enc`,
  `token_cache.json`, and the plaintext `credentials.json`. The encryption key is in the OS keyring. On
  Linux it falls back to a `.encryption_key` file. On macOS the keychain is strict unless
  `GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND=file`. **Docs and changelog.**
- **Refresh path:** a made-up credentials file, empty HOME, file backend and no desktop reached Google's
  token endpoint with no prompt, and got `invalid_client`, exit 2. **Ran.** So the path needs no keychain.
- **Machine-readable reads:** JSON by default. **Ran**: errors are JSON on stdout, with a plain line on
  stderr and exit codes 1-5. `--page-all` gives NDJSON, per help. `gmail +read` defaults to text, so
  it needs `--format json`. `calendar +agenda` and `gmail +triage` take `--format json`.
- **In a service (Ran):**
  - `/usr/local/bin/gws` is a Node wrapper. With `PATH=/usr/bin:/bin` it exits 127 (`env: node: No such file`).
  - The native binary inside the package runs on that PATH, so either call it or add Node to PATH.
  - **Always set `HOME` or the config dir.** With `HOME` unset, gws finds the account's real home from
    the OS and uses its real config, which defeats per-run isolation.
  - It writes its API schema cache to the config dir, so that must be writable.

**Likely trap.** Google expires refresh tokens after **7 days** for External apps in "Testing" status
(**Docs**), and that is the setup the gws README walks through. Without a fix, connected Gmail would
stop working after a week.

**Needs a throwaway Google account:** a real login and export; a successful refresh on a machine with
no keychain, ideally Linux; a Gmail and Calendar read to see real output; whether a token survives past
7 days in Testing, and whether "In production" status avoids that.

**Decisions for #34.**

- Run the native binary, or add Node's folder to the extended PATH.
- Set `HOME`, a per-source config dir, `GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND=file` and
  `GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE` pointing at the private key file.
- Pass `--format json` and treat exit code 2 as "reconnect this source".

**Decision for #35.** On a VPS, connect Google by exporting from a machine with a browser. Don't promise
long-lived Gmail on a Testing app until the 7-day check is done.

## Broker tool timeouts

**Ran**, against the installed fx with a local stub model and MCP server, using Hyperthymesia's profile shape:

| Tool length | `operation_timeout_ms` | Result |
| --- | --- | --- |
| 30 s | 15000 (today) | Cancelled at 15 s |
| 180 s | 60000 (fx's default) | Cancelled at 60 s |
| 180 s | 200000 | **Succeeded at 180 s** |

**Real limit (Source):** `operation_timeout_ms` is how long a request may go without an answer. fx
accepts up to the u32 maximum. There is also a hard cap of the larger of 10 minutes and that value
(`src/core/mcp_host/config.zig:151-152`). The setting is per server, not per tool. Progress messages
would reset the wait, but the broker sends none. The whole run is still bound by `HX_FX_TIMEOUT_S`,
1200 s by default.

**Decisions for #34's bash tool:**

- Raise the value above the longest allowed command.
- Enforce the command deadline and kill the process inside the broker. fx only sends a cancel
  notification, and the broker ignores those (`mcp.rs:675-677`).
- A timeout is quiet: `fx ask` still exits 0, and the tool call is marked `error`. `fx.rs:323-330`
  only checks successful calls, so count these failures explicitly.
- The broker holds its audit lock while a tool runs (`mcp.rs:392,404`), so a long command blocks every
  other tool call. Release the lock during the command.
