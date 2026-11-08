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

**Pending at this checkpoint.** Do not infer live installation from fixture
tests, artifact-only builds, a scratch HOME or the presence of a workflow file.
Record the final tag, merged VERSION/SHA256-only pin PR, Pages deployment and
completed new-hosted-VM run here before declaring the public curl path proven.

Real provider authentication/model availability, paid execution, actual Mac
permissions/capture, real reboot/power-loss recovery and Developer ID
notarization remain separate owner-controlled checks. No such claim follows
from the login-free service check.
