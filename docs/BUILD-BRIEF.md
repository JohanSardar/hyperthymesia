# Shared rules for build workers

Read `docs/DESIGN.md` (the consensus plan) first. The contract is `crates/hx-schema` (envelope v1, core kinds, connector protocol, `CompanionEvent`). Frozen interfaces you code against are the stubs in `crates/hx-compile/src/lib.rs` and `crates/hx-interpret/src/lib.rs`.

## Ownership
- You own ONLY the directories named in your task. Do not edit any other path, including `Cargo.toml` at the root, `crates/hx-schema`, `docs/`, or another worker's crate.
- Need a change to `hx-schema` or a frozen interface? Do not make it. Work around it locally and list the requested change in your final report.
- Add dependencies only in your own crates' `Cargo.toml`. Prefer `foo.workspace = true` for deps already in the root `[workspace.dependencies]`.
- Do not run `git commit`, `git add`, or any git write. The coordinator commits.

## Building
- Set `CARGO_TARGET_DIR` to the value in your task for every cargo command (disk is tight; never create a second target dir).
- Build and test only your packages: `cargo test -p <pkg>` and `cargo clippy -p <pkg> --all-targets -- -D warnings`, then `cargo fmt -p <pkg>`.
- Edition 2024, Rust 1.99. `unsafe_code` is forbidden by workspace lints except in `hx-macos-sys`.
- Postgres for tests: `postgres://hx:hx@localhost:55432/hx` (already running via docker compose). Use a fresh schema or database per test run so tests don't collide.
- No sudo, nothing that triggers a password or keychain prompt, no network services exposed beyond localhost.

## Quality bar
- Test behavior, not implementation. Every unit you claim done has a test that would fail if the behavior broke.
- Keep comments to non-obvious *why*. No phase-narrating comments.
- Parse external data at boundaries; trust internal types. Make illegal states unrepresentable.
- Smallest design that meets the task. No speculative abstraction.

## Final report (your last message)
1. What you built, per crate, with file paths.
2. Exact commands you ran to verify, and their results (pass counts).
3. What is NOT verified and why (e.g. needs macOS permission).
4. Requested changes to `hx-schema` or frozen interfaces, if any.
