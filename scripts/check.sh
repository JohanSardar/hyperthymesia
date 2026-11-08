#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
for command in cargo python3 bun zsh; do
  command -v "$command" >/dev/null || { echo "Missing test prerequisite: $command" >&2; exit 1; }
done
packages=(--locked --workspace)
if [[ "$(uname -s)" != Darwin ]]; then
  packages+=(--exclude hx-macos-sys --exclude hx-agent --exclude hx-agent-app)
fi
# SQLite writes must take the writer lock before reading any state.
python3 scripts/check-store-transactions.py
python3 scripts/check-tooling.py
cargo fmt --all -- --check
cargo clippy "${packages[@]}" --all-targets -- -D warnings
cargo check "${packages[@]}" --all-targets
cargo build "${packages[@]}"
cargo test "${packages[@]}"
python3 scripts/quickstart.test.py

if [[ "$(uname -s)" == Darwin ]]; then
  cargo clippy --locked -p hx-agent -p hx-agent-app --all-targets --features e2e-harness -- -D warnings
  cargo test --locked -p hx-agent -p hx-agent-app --features e2e-harness
  scripts/check-e2e-harness.sh
fi

sh scripts/install.test.sh
python3 scripts/installer-launcher.test.py
for directory in companions/*/; do
  if [[ -f "$directory/package.json" ]]; then
    (cd "$directory" && bun install --frozen-lockfile && bun run typecheck && bun run build && bun test)
  fi
done
bun test companions/shell-hook
zsh -n companions/shell-hook/hyperthymesia.zsh
python3 scripts/scan-personal-data.py
if [[ -n "${HX_MOVE_TEST_POSTGRES:-}" ]]; then
  cargo test --locked -p hx-move -- --ignored
fi
