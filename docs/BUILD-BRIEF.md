# Shared rules for build workers

Read `docs/DESIGN.md` (the consensus plan) first. The contract is `crates/hx-schema` (envelope v1, core kinds, connector protocol, `CompanionEvent`). Frozen interfaces you code against are the stubs in `crates/hx-compile/src/lib.rs` and `crates/hx-interpret/src/lib.rs`.

## Ownership
- You own ONLY the directories named in your task. Do not edit any other path, including `Cargo.toml` at the root, `crates/hx-schema`, `docs/`, or another worker's crate.
- Need a change to `hx-schema` or a frozen interface? Do not make it. Work around it locally and list the requested change in your final report.
- Add dependencies only in your own crates' `Cargo.toml`. Prefer `foo.workspace = true` for deps already in the root `[workspace.dependencies]`.
- Git: the repo is PUBLIC at https://github.com/JohanSardar/hyperthymesia. Commit your own paths only (`git add <your dirs>`, never `git add -A`), in small commits with clear messages ending with the line `Co-Authored-By: GPT-6.1 Sol <noreply@openai.com>`. If you run in your own worktree/branch, push that branch (`git push -u origin HEAD`) and open a PR to `main` with `gh pr create`. If you run in the shared main checkout, commit to `main` and `git pull --rebase` before `git push`. Never force-push, never rewrite history, never commit build output.
- PUBLIC REPO PRIVACY: never commit real personal data. Fixtures from Johan's machine must be anonymized (no real message text, thread titles, file paths under /Users, emails, names, URLs, tokens). When unsure, generate synthetic data instead.

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
