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
