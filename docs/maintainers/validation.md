# Open-source validation

## Baseline (2026-11-08)

- Compared fx shallow clone `3d90fe9254eac209ddf55fccc16984651e6f20a8`
  with Hyperthymesia `33cbc2e1847c164648cb738a68ddeb81d636ef76`.
- Privacy scanner over current files and all branch/remote/tag history:
  exit 0; 0 unapproved, 232 approved pattern matches. Private build-process
  notes were found manually and must be removed from the tree. Historical
  author identities/project URLs remain public; history is not rewritten.
- Companion frozen-lockfile installs, TypeScript checks and builds passed;
  browser tests 8/8, editor tests 1/1.
- Existing latest-main GitHub CI: success, run `42521252237`.
- Local Docker check failed because its daemon is not running. No local
  clean-machine claim is made.

## Evidence rules

Only completed checks count as passing. Fixture/service stubs do not establish
real service behavior. A temporary HOME does not establish a clean OS image.
Commands needing credentials, permissions, destructive legacy data operations
or a paid model request are documented separately and are not silently run
against the owner's machine.

## CI hardening

- Local `scripts/check.sh` completed successfully, including macOS synthetic
  capture/pairing harness checks. Installer contracts and actionlint passed.
- First PR CI exposed two shell-hook failures because Ubuntu lacked zsh. The
  checks were not skipped or weakened; the runner prerequisite was added.
- Private vulnerability reporting and strict/admin-enforced branch protection
  with `Required checks` were enabled through the GitHub API.

## Combined local candidate checkpoint

Candidate `b11b362` (server/app 0.1.3) completed the default
`./scripts/check.sh` on macOS in **249.35 seconds** with an existing dependency
cache. This is a local validation measurement, not clean-machine evidence or an
installation-time promise.

- Rust fmt, clippy with warnings denied, explicit locked typecheck/build,
  unit/integration tests and rustdoc passed.
- **531 passed Rust test executions**, including re-executed subprocess helpers
  and the second Mac harness pass; this is not a unique-test count.
- Six ignored declarations: five disposable-Postgres tests (exercised by the
  green hosted mover job) and one subprocess-only interpretation-lock helper
  exercised through its parent test, not an untested user feature.
- Real synthetic CLI/loopback/auth/online-backup/restore smoke and the Mac
  permission-free pairing/capture harness passed.
- Browser/editor companion frozen installs, typechecks, builds and 8/1 tests;
  shell hook 3 tests and zsh syntax passed. Installed-Chrome coverage is
  conditional and is not a proof of real extension permissions.
- Reference generator 5 tests, release integrity 5 tests, README selector
  3 tests, generated help/environment/routes/MCP drift and local file links passed.
- Offline installer contracts and piped-launcher tamper/duplicate-manifest/
  forwarding/cleanup tests passed.
- Privacy scan: 0 unapproved, 258 approved pattern matches at that checkpoint.
  Approved counts vary as public URL/author blob versions accumulate.

Subsequently, a fail-first injected partial PATH write demonstrated corruption
of an existing Bash profile. Installer commit `2caad79` stages beside the file,
preserves its bytes/mode, renames only a complete block and removes the stage on
rollback. The final full offline installer suite passes with unchanged original
profile, CLI link, evidence and environment. Final public release acceptance
must include this later fix, not only the earlier combined checkpoint.

## Independent secret and privacy audit

Gitleaks 8.30.1, with output redacted and no new suppressions:

| Scope | Scanner exit | Findings | Classification verified against original source lines |
| --- | --- | --- | --- |
| Tracked-source archive of `b11b362`, excluding Git metadata/ignored builds/private untracked work | 1 | 25 | 24 generated HMAC-anonymized fixture identifiers; 1 intentional shell-hook redaction assertion |
| All local branch/remote/tag history | 1 | 73 | 71 generated anonymized fixture identifiers across versions; 2 intentional redaction assertions |

Every finding was classified; none remains an unexplained credential finding.
Exit 1 is reported honestly, not relabeled as a clean scanner exit. The original
whole-directory attempt included ignored compiled dependencies, so it was
replaced by the tracked-source archive scan above.

The custom privacy scan is separate from Gitleaks. Manual inspection found
internal machine/process notes in the old build/planning/review documents.
PR #87 removes them from the current tree, **not published history**. Public
author identities and repository URLs also remain in history. No rewrite,
registry publication, visibility change or deletion of pre-existing untracked
work was performed. These scans do not prove the absence of arbitrary private
content or every possible secret.

## Public release acceptance

**Completed for v0.1.3.** The public pin and disposable-host results below
establish the login-free installation path. The earlier combined checkpoint
alone did not establish live installation.

Real provider authentication/model availability, paid execution, actual Mac
permissions/capture, real reboot/power-loss recovery and Developer ID
notarization remain separate owner-controlled checks. No such claim follows
from the login-free service check.

## Takeover and published release

- Reference PR #93 and README PR #94 merged after all checks passed. Their
  generated reference/link checks and README-selector tests also passed locally.
- The complete local scripts/check.sh passed for the final 0.1.3 candidate,
  including the atomic PATH fix, real CLI/API/backup/restore smoke, capture
  harness, installer contracts, companions and privacy scan (0 unapproved,
  261 approved matches). Merging current main into the candidate changed no
  file contents, verified with git diff HEAD^ HEAD.
- Release metadata PR #95 merged with all 16 checks successful (publication is
  intentionally skipped on PRs). Tag v0.1.3 resolves to
  545d2224c325c02da9de4400c30d77cacd4b9e30.
- [Publication workflow](https://github.com/JohanSardar/hyperthymesia/actions/runs/42553332228)
  passed native builds, pristine Linux x86_64/arm64 binary execution, real user
  service smoke on Linux/macOS, assembly and GitHub publication.
- Downloaded all six assets from the public GitHub release and verified the
  exact asset set and each digest with scripts/release-notes.py. Published
  notes match the independently generated notes (apart from a trailing newline).
  SHA256SUMS hashes to
  0943a5d8c6f96e51c435f95738ec52cd93046234805aec93c88de11faa3de599.
- Pages was enabled successfully from main /docs and its public install URL
  returned the expected bootstrap launcher. Public activation and clean-host
  results are recorded below only after completion.

- Pin-only PR #96 passed all seven checks and merged at
  3db3eec4c023711b4b114a483620710d29b024e0. Its diff changes only VERSION and
  SHA256 in docs/install. Piped-launcher checksum/tamper/duplicate-manifest/
  forwarding/cleanup contracts passed locally before submission.

## Completed public curl and stranger acceptance

- [Pages deployment](https://github.com/JohanSardar/hyperthymesia/actions/runs/42556289189)
  completed for 3db3eec4c023711b4b114a483620710d29b024e0. Fetching the public
  install URL confirmed VERSION=0.1.3 and the independently verified manifest
  hash before acceptance was dispatched.
- [Fresh hosted-VM acceptance](https://github.com/JohanSardar/hyperthymesia/actions/runs/42556331182)
  completed successfully on 2026-11-09 UTC, using a new GitHub-hosted Ubuntu VM.
  It ran the exact public curl command against real Pages/release URLs, verified
  hx-server 0.1.3, executed the exact README login-free block, checked doctor,
  status and health, created synthetic token metadata, reran installation,
  verified retained metadata and upgrade backup, then uninstalled the service.
- Measured first curl-to-healthy-service: **3.47 seconds**; README check:
  **0.03 seconds**; second install: **3.92 seconds**. These are observations on
  that runner/network, not latency guarantees.
- A fresh full public clone, isolated Git configuration and empty build target
  ran the documented ./scripts/check.sh successfully in **358.55 seconds**.
  This covered locked lint/typecheck/build/tests, CLI/API/backup/restore,
  generated references/rustdoc, installer contracts, companion checks and
  privacy scan (0 unapproved, 265 approved matches). No developer build output
  was copied into the VM or fresh clone.
- The real public install command is:

  ```sh
  curl -fsSL https://johansardar.github.io/hyperthymesia/install | sh
  ```

- [Published release](https://github.com/JohanSardar/hyperthymesia/releases/tag/v0.1.3).
  No registry publication, repository visibility change or history rewrite was
  performed. No owner action remains to publish or share this release.

### Remaining limits and optional owner checks

Real provider authentication, paid model execution, actual Mac capture/TCC
permissions, a real reboot/power loss and Developer ID notarization were not
proved. The public clean-VM acceptance covers Linux x86_64; arm64 and macOS
have native build and artifact/user-service smoke evidence, rather than a
second public-curl clean-machine claim. Installed-Chrome checks remain
conditional. Large-module debt remains documented in quality.md.

To use personal capture/cloud features, follow the explicit provider login in
[installation first run](../install.md#first-run), then the deliberate
[Mac pairing and permission steps](../mac-pairing.md). Those are optional
account/device checks, not release blockers. Removed internal notes and public
author metadata remain in Git history; rewriting history still requires a
separate owner decision. No arbitrary-secret-absence guarantee is made.
