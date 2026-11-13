#!/usr/bin/env bash
# Build before hx-server: its binary embeds app/dist for installation/rollback.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -f app/package.json ]]; then
    (cd app && bun install --frozen-lockfile && bun run build)
    [[ -s app/dist/index.html ]] || { echo 'Web build did not produce app/dist/index.html' >&2; exit 1; }
else
    echo 'No web app source yet; hx-server will embed its placeholder.'
fi
