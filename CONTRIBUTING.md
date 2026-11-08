# Contributing

Small, test-backed pull requests are welcome. Original contributions are
AGPL-3.0-only, like the project. No contributor license agreement is required.
Follow the [code of conduct](CODE_OF_CONDUCT.md).

## Set up

Use Git, Python 3, curl, a C compiler/linker and
[rustup](https://rustup.rs/). Cargo selects `rust-toolchain.toml` automatically.
Linux needs build-essential and pkg-config (or your distribution's equivalents).
macOS development needs Apple Silicon, macOS 14+ and Xcode command line tools.
Companion development needs Bun 1.3.12; shell-hook tests also need zsh.
No database service is needed for normal development.

Clone the repository, enter its root, then build:

```sh
cargo build --locked --workspace
```

On Linux, exclude the three macOS-only packages:

```sh
cargo build --locked --workspace --exclude hx-macos-sys --exclude hx-agent --exclude hx-agent-app
```

Never point development commands at your real capture/server state. Tests use
temporary databases and synthetic inputs. Never commit real captures, tokens,
login files, screenshots, window titles, private URLs or logs.

## Check your change

```sh
./scripts/check.sh
```

This runs locked Rust formatting, clippy, explicit typecheck/build,
unit/integration tests, real synthetic CLI/API/backup/restore smoke, installer
contracts, companion typechecks/builds/tests, shell syntax, privacy and generated
documentation/rustdoc checks. On macOS it also runs the synthetic capture harness.
Linux automatically excludes macOS-only crates. No paid account, captured history
or persistent service is used. See [reference generation](docs/guides.md#change-a-documented-interface)
when changing CLI, configuration, MCP or HTTP interfaces.
The installed-Chrome test is optional when Chrome is unavailable; it is not
proof of a browser's real extension permissions.

The explicitly ignored legacy-mover tests need a **disposable** PostgreSQL
database via `HX_MOVE_TEST_POSTGRES`; normal tests never require it.
Hosted CI provisions its own database. Real macOS capture, an eight-hour soak,
real provider logins and paid model calls are not prerequisites for a small
code/documentation change, and synthetic tests do not prove them.

## Pull request flow

1. Fork or branch from current main. Use a short branch name.
2. Reproduce a bug and add a failing test before changing behavior. Keep public
   contracts, migrations and data-recovery guarantees backwards compatible.
3. Make small commits with Conventional Commit titles, such as
   `fix: preserve queued evidence after retry` or `docs: explain pairing`.
4. Run checks on the final tree. Include commands/results, privacy impact,
   compatibility and anything not verified in the PR description.
5. Open a PR to main. Required CI must pass against current main. Resolve review
   conversations. A maintainer squash-merges; do not bypass checks or force-push
   published main history.

Do not run real installer lifecycle steps on your normal HOME just to test a PR.
Use the offline contracts or disposable hosted installation tests.
Do not publish packages, push release tags, alter repository visibility, or
rewrite public history as part of an ordinary contribution.

## Reports

Use the issue forms for reproducible bugs and focused feature proposals.
Report vulnerabilities through [SECURITY.md](SECURITY.md), not public issues.
Redact reports; do not attach server.env, state databases, credentials or raw
evidence. State your OS/architecture, release version and exact safe reproduction.
