# Mac first run and pairing

An unconfigured app shows **Not paired** and keeps observations in its local
outbox. Choose **Grant permissions** to request macOS capture permissions when
ready. Permission requests happen only through that action or `hx-agent onboard`.

Copy a server-issued link and choose **Pair from clipboard**, or run:

```sh
hx-agent pair 'hyperthymesia://pair?server=<percent-encoded-url>&code=<64-hex-characters>'
```

Pairing requires HTTPS, except for literal loopback addresses and `localhost`.
Redirects are refused. Expired or used links must be minted again; failures leave
`agent.json` unchanged. Pairing only merges `server_url` and `token`, preserving
capture settings and unknown fields. The file is atomically replaced with mode
600; no Keychain access is used. The running uploader notices new credentials
within its next one-second config poll (or after an in-flight request completes).

An existing token is retained unless the CLI receives `--replace`. Replacement
also sends any queued observations to the new server, so use it deliberately.
The menu action never replaces an existing token.

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
