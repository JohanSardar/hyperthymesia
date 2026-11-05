#!/usr/bin/env bash
# A successful release compilation here is a safety regression.
set -euo pipefail
cd "$(dirname "$0")/.."
log="$(mktemp "${TMPDIR:-/tmp}/hx-n5-release-guard.XXXXXX")"
trap 'rm -f "$log"' EXIT
# Even enabling debug assertions must not bypass the release guard.
if CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true cargo check --release -p hx-agent-app --features e2e-harness >"$log" 2>&1; then
  echo 'ERROR: e2e-harness compiled in release mode' >&2
  exit 1
fi
if ! rg -q 'e2e-harness must never be enabled in release builds' "$log"; then
  cat "$log" >&2
  exit 1
fi
echo 'Verified: e2e-harness cannot compile in release mode.'
