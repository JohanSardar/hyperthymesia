#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

if [[ "$(uname -s)" == Darwin ]]; then
  cargo clippy -p hx-agent -p hx-agent-app --all-targets --features e2e-harness -- -D warnings
  cargo test -p hx-agent -p hx-agent-app --features e2e-harness
  scripts/check-e2e-harness.sh
fi
