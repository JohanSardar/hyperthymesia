# Install-anywhere end-to-end evidence (#35)

Manual verification on 2026-11-06, using a newly created Ubuntu 24.04 aarch64
container, systemd PID 1, and fresh unprivileged users with their own user managers
and linger. No host state or existing container was mounted or modified. Port
7377 existed only inside that container's network namespace, with no published
host port. The product installation used neither Docker nor a database server.
Docker was only the disposable verification environment.

No tests were added or changed. The following are live CLI/HTTP checks, not a new
test suite. All observations, device identity, image, credentials and model
responses were synthetic. The real pinned fx archive was downloaded and its
production SHA-256 verified; real fx was invoked only with `--version`. Before
admitting any interpretation, `HX_FX_BINARY` was replaced with a network-free
stand-in. The container's external network was then disconnected as an additional
barrier. No login, paid model call, release tag or Mac service operation occurred.

## Install and device flow

- `HX_RELEASE_BASE=file:///work/artifacts/schema3` selected a locally built
  `hx-server-0.1.0-linux-aarch64.tar.gz` and its SHA256SUMS. With
  `HX_FX_RELEASE_BASE` pointing at an absent archive, the installer exited nonzero,
  printed an explicit download failure and created neither installation nor state.
- One `sh scripts/install.sh --server-only --provider gateway --model
  synthetic-install-model --effort low` invocation produced an active systemd user
  service, `/healthz`, and exactly one initial `core.sqlite`. The state root was
  0700; databases, environment and log were 0600. Configuration retained the
  selected provider, model and effort without printing the synthetic secret.
- A second clean user installed the final full-schema artifact with Grok, a
  different synthetic model and `high` effort. Missing login produced the explicit
  setup warning without preventing service health. `ss -ltn` showed only
  `127.0.0.1:7377`, not a wildcard listener. All active state files/directories had
  0600/0700 modes. Uninstall removed service entry points but retained history.
  A real reinstall after uninstall plus `systemctl --user daemon-reload` exposed
  systemd exit 5 for the already-absent unit. The installer now treats only that
  specific stop result as success; the repeated reinstall backed up retained state
  and started the service. Other service-manager failures remain fatal.
- The pairing CLI produced a link; `POST /v1/pair` redeemed it once (200), and
  replay returned 410. The resulting device token ingested two observations and
  one synthetic JPEG. Its prefix restrictions were retained through upgrade.
- On Linux, `/v1/device` advertised `{"ocr":"device"}`. A paired-token
  `PUT /v1/ocr/{sha}` accepted a synthetic Vision result. The real lineage worker
  published it; both `/v1/evidence/{id}` and the human-readable `/evidence/{id}`
  citation returned the OCR text.

## Schema-changing upgrade and interruption

The two local release binaries really embedded different migration sets. The
baseline was built with migrations 1–3; the upgrade included the unmodified real
`0004_ocr_result_scope.sql`. This exercises its table rebuild through the installed
CLI/service lifecycle, not by editing `_sqlx_migrations` or applying DDL manually.
It is not a claim of compatibility with an independently published older release.

- After installing the baseline, public APIs seeded citable history, a blob and
  device tokens. A synthetic fault artifact delegated to the new real binary but
  deliberately failed its post-start state doctor. The candidate had reached
  migration 4. The installer returned nonzero, restored the old binary and schema
  1–3, restarted the service, and served byte-identical original evidence. The
  failed schema-4 generation remained available beside state.
- The successful upgrade applied the real migration 4, added OCR scope columns,
  passed foreign-key checks and preserved exact event IDs/envelopes, the citation,
  blob, tokens and byte-identical `server.env`. Previous versions and verified
  backups remained available. Device OCR was then accepted and cited.
- A fault artifact paused immediately after `migrate`. SIGKILL of the lifecycle
  shell made the outer guard kill the remaining migration process group, restore
  the previous generation and restart a healthy service with identical history.
- Separate SIGKILLs of the top-level installer and of the guard allowed the
  independently running lifecycle to finish successfully. Its inherited lock
  remained effective until completion; history and service health survived.
- Killing the entire installer process tree left the durable recovery record.
  On the next `systemctl --user start hyperthymesia.service`, the service wrapper
  itself resumed rollback before loading configuration/opening state. Health and
  original history returned without rerunning the installer. This simulates the
  next user-manager start, not a real machine reboot.
- The final installer also repeated failed, SIGKILL-interrupted and successful
  schema-changing upgrades with state on a different, `noexec` filesystem
  (`/dev/shm` in this disposable container). Migration 4, history preservation,
  rollback/restart and foreign keys passed. The executable recovery payload stays
  in the program prefix; only its private non-executable lock anchor sits beside
  state. This avoids both cross-filesystem hard-link failures and attempts to
  execute binaries on a data-only mount. Normal completion removed the temporary
  anchor. The volatile mount was a verification fixture, not a recommendation for
  storing real history.

## Restore and subprocess lifetime

- Online backup included both databases and the published JPEG. The pre-change
  restore reproduced 0755 state-root/0644 file permissions under umask 022.
  Repeating backup and restore with the fixed release binary produced a 0700
  root, 0700 nested directories and 0600 files throughout, with identical evidence
  and blob bytes. Privacy is established before copying history, not repaired
  after publication.
- The pre-change fx lifecycle was rebuilt for a direct comparison. SIGKILL of a
  directly launched server left both the stand-in fx process and its sleeping
  descendant alive; both synthetic orphans were explicitly cleaned up.
- The Linux run caught a shell portability issue in the first lifetime watcher:
  dash replaces background stdin with `/dev/null`. Saving the lifetime pipe on an
  explicit descriptor fixes that and allows normal `fx --version` completion.
  The watcher starts before fx, also closing the cancellation/spawn race.
- Repeating the direct server SIGKILL with the fixed release binary killed and
  reaped both fx and its descendant, without any service manager. The stand-in
  observed the selected gateway, synthetic model and `low` effort on the actual
  subprocess invocation. The installed systemd service separately proved crash
  restart with a new MainPID and no remaining fx/descendant, using selected Grok,
  another synthetic model and `high` effort. Device-token revocation took effect
  immediately (401 on the next request).
- The final installed artifact repeated paired-device OCR, human-readable and
  JSON citation retrieval, and backup/restore of the OCR evidence ID and JPEG.
  Every backup/restore directory/file had 0700/0600 modes under umask 022. Pairing
  secrets and bearer tokens were absent from the server log. A read token with
  exact device prefixes intentionally reserves that identity too; the repeat
  used a fresh synthetic device identity rather than weakening that protection.

## Existing checks and limits

All commands below passed with exit code 0 (existing ignored tests remain
ignored). Cargo commands use the synthetic runner on the Mac:

```sh
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/synthetic-test-runner.py"
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
sh scripts/install.test.sh
python3 scripts/check-store-transactions.py
```

The existing installer script covers scratch Mac paths and stubbed LaunchAgents;
no installer was run against the Mac's real user. No screen automation was needed:
citations were opened over their real authenticated HTTP routes. Agent-desktop MCP
was not configured in this run, and no real app, capture permissions or account
login was used. Real reboot/headless provisioning, actual Mac Vision output and
real provider authentication remain outside this synthetic check. CI's existing
install smoke still uses a same-schema layout upgrade; the schema-changing proof
above is the separate manual installed-service run.
