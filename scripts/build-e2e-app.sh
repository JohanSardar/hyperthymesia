#!/bin/zsh
# Explicit debug-only synthetic app; never writes to dist or installs a service.
set -euo pipefail
cd "${0:A:h:h}"
if (( $# > 1 )); then print -u2 -- "usage: $0 [output-directory]"; exit 2; fi
output="${1:-$(mktemp -d "${TMPDIR:-/tmp}/hx-n5-harness.XXXXXX")}"
output="${output:A}"
if [[ "$output" == "$PWD/dist" || "$output" == "$PWD/dist/"* ]]; then
  print -u2 -- "the harness must be built outside dist"; exit 2
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/.target-native}"
cargo build -p hx-agent -p hx-agent-app --features e2e-harness
app="$output/Hyperthymesia-e2e.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$CARGO_TARGET_DIR/debug/hx-agent-app" "$app/Contents/MacOS/Hyperthymesia-e2e"
cp "$CARGO_TARGET_DIR/debug/hx-agent" "$app/Contents/MacOS/hx-agent"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.hyperthymesia.agent.e2e</string>
<key>CFBundleName</key><string>Hyperthymesia-e2e</string>
<key>CFBundleExecutable</key><string>Hyperthymesia-e2e</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PLIST
codesign --force --sign - "$app/Contents/MacOS/hx-agent"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
print -r -- "$app"
