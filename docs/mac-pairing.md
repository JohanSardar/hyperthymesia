# Mac first run and pairing

An unconfigured app shows **Not paired** and keeps observations in its local
outbox. Choose **Grant permissions** to request macOS capture permissions when
ready. Permission requests happen only through that action or `hx-agent onboard`.

Copy a server-issued link and choose **Pair from clipboard**, or run:

```sh
hx-agent pair 'hyperthymesia://pair?server=<percent-encoded-url>&code=<64-hex-characters>'
```

Pairing requires HTTPS, except for literal loopback addresses and `localhost`.
Redirects are refused. Expired, used or revoked links must be minted again. A
503 means the server is temporarily unavailable: try the same link again. Failures
leave `agent.json` unchanged. Pairing merges `server_url`, `token` and `device_id`, preserving
capture settings and unknown fields. The file is atomically replaced with mode
600; no Keychain access is used. The running uploader notices new credentials
within its next one-second config poll (or after an in-flight request completes).

An existing token is retained unless the CLI receives `--replace`. Replacement
also sends any queued observations to the new server, so use it deliberately.
Before re-pairing the same device, revoke its previous server token using
`hx-server token list` and `hx-server token revoke <id>`. A live token with that
exact device's prefixes prevents another pairing; this refusal does not consume
the code. `--replace` replaces local credentials and does not revoke server tokens.
The menu action never replaces an existing token.

New pairing saves the exact device ID sent to the server in the same atomic write
as the token. Moving networks or changing the Mac's hostname then keeps uploads
on the original device prefixes. IDs allow 1–255 ASCII letters, digits, `.`, `_`
and `-`, excluding `.` and `..`. Before pairing, the client replaces other
hostname characters with `-`, limits the result to 255 characters, and uses
`mac-unknown` for an empty result or the reserved IDs `.` and `..`. Unpaired capture uses the
same normalized ID. Existing configs with a token but no saved `device_id` keep
their current hostname fallback; they are not migrated or rewritten.

Server owners mint links with `hx-server pair --name NAME [--url URL]`. Use
`hx-server pair list` to find a code's hash ID and `hx-server pair revoke <id>`
to disable an unused link without revealing its secret. Revoking a code does
not revoke a token already issued from it; revoke that token separately.

## Synthetic app

The `e2e-harness` Cargo feature builds a separate, debug-only app with bundle ID
`com.hyperthymesia.agent.e2e`. It uses `Hyperthymesia-e2e` under Application Support
(or an explicit scratch directory). It has no capture, companion listener, or
login-startup registration. **Grant permissions** is disabled in this app.

Build outside `dist`, without installing or launching anything:

```sh
scripts/build-app.sh --e2e /path/to/scratch-build
```

Run the resulting `Hyperthymesia-e2e.app/Contents/MacOS/Hyperthymesia-e2e` with
`--support-dir /path/to/scratch-state --seed-jpeg /path/to/synthetic.jpg`.
`--headless --duration-s 30` exercises delivery without AppKit. Use only synthetic
JPEGs. The bundled `hx-agent pair` uses its separate support folder; for an
explicit scratch directory, set `HX_E2E_SUPPORT_DIR` to the same directory.

The harness integration test starts unpaired, queues an 8×8 synthetic JPEG,
redeems against a fake Axum server, and verifies authenticated blob/event uploads
without restarting. All state lives in temporary folders. Both `scripts/check.sh`
and macOS CI run this test and `scripts/check-e2e-harness.sh`; the latter requires
a release build to fail even when release debug assertions are enabled.
