# Changelog

All notable changes are documented here, following
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Versions before this first public binary release were source-only checkpoints.

## [Unreleased]

### Changed

- The project moved from GitHub to GitLab (https://gitlab.com/johansardar/hyperthymesia).
  The public installer is now `curl -fsSL https://hyperthymesia.eilmer.si/install | sh`
  and release downloads come from `https://hyperthymesia.eilmer.si/releases/`.
  The 0.1.3 assets are byte-identical, so the launcher's manifest pin is unchanged.

- Mac visual capture now saves JPEG screenshots only. Video encoding, MP4
  staging and new video persistence are removed; existing evidence is retained.
- Capture health/menu text now describes screenshots. Temporary native objects
  are drained after JPEG conversion on the background capture thread.

## [0.1.3] - 2026-11-08

### Added

- First public GitHub binary release for Linux x86_64/arm64 and Apple Silicon,
  including license/source/dependency notices and SHA256SUMS.
- A checksum-pinned GitHub Pages installer using the existing owned user-service
  installation, stop-before-swap, backup and rollback lifecycle.
- Atomic, idempotent Bash/zsh PATH setup and tamper/interrupted-upgrade tests.
- Contributor, conduct and private security-reporting guidance; issue/PR templates.
- Current architecture, task guides, troubleshooting, generated CLI/options,
  environment/route inventories, MCP tool schemas and Rust API reference checks.
- One locked contributor command for lint, typecheck, build, tests, synthetic
  CLI/API/backup/restore, installers, companions and privacy/documentation checks.
- Disposable-host release smoke tests and separate public-curl/fresh-clone
  acceptance without accounts or paid requests.

### Changed

- Main now requires green, current-main CI, including admin merges.
- CI uses read-only tokens, timeouts, fixed Bun and reproducible companion installs.
- Extracted citation support and MCP schema admission from large server modules,
  preserving their public paths and policy.
- Moved obsolete internal planning/review/build notes out of the current tree;
  published history is unchanged.

### Fixed

- Fixture anonymization tests no longer depend on a developer's Cargo setup,
  target directory or real HOME key.
- New Linux contributors run the same platform-aware checks as CI.

### Security

- This remains experimental, with cloud egress, unsandboxed host execution,
  unencrypted state/keys, retained history and disabled GC.
- The macOS app is ad-hoc signed unless a maintainer separately provisions
  Developer ID signing/notarization. No signing credentials are bundled.
- Synthetic checks do not prove capture permissions, provider login/model
  availability, paid execution or a real reboot.
