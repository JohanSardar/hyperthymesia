#!/bin/zsh
set -euo pipefail
source "$HOME/.cargo/env"
cd "${0:A:h:h}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/.target-native}"
cargo build --release -p hx-agent -p hx-agent-app
app="$PWD/dist/Hyperthymesia.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$CARGO_TARGET_DIR/release/hx-agent-app" "$app/Contents/MacOS/Hyperthymesia"
cp "$CARGO_TARGET_DIR/release/hx-agent" "$app/Contents/MacOS/hx-agent"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.hyperthymesia.agent</string>
<key>CFBundleName</key><string>Hyperthymesia</string>
<key>CFBundleExecutable</key><string>Hyperthymesia</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSAccessibilityUsageDescription</key><string>Record focused window context for your private activity timeline.</string>
<key>NSScreenCaptureUsageDescription</key><string>Record screen evidence for your private activity timeline.</string>
<key>NSInputMonitoringUsageDescription</key><string>Measure input activity without recording key identities.</string>
</dict></plist>
PLIST
# A certificate signature keeps macOS privacy grants across rebuilds; ad-hoc grants
# are pinned to one binary hash. Override with HX_SIGN_IDENTITY (use - for ad-hoc).
identity="${HX_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | awk '/"Apple Development:/ {print $2; exit}')}"
if [[ -z "$identity" ]]; then
  identity=-
  print -u2 -- "warning: no Apple Development identity; ad-hoc signing resets permissions on every rebuild"
fi
flags=(--force --sign "$identity")
[[ "$identity" != - ]] && flags+=(--options runtime)
codesign "${flags[@]}" "$app/Contents/MacOS/hx-agent"
codesign "${flags[@]}" "$app"
codesign --verify --deep --strict "$app"
print -r -- "$app"
python3 - "$app" "$PWD/dist/com.hyperthymesia.agent.plist" <<'PY'
import plistlib, pathlib, sys
app, output = sys.argv[1:]
with open(output, 'wb') as stream:
    plistlib.dump({
        'Label': 'com.hyperthymesia.agent',
        'ProgramArguments': [str(pathlib.Path(app) / 'Contents/MacOS/Hyperthymesia')],
        'RunAtLoad': True,
        'KeepAlive': {'SuccessfulExit': False},
        'LimitLoadToSessionType': 'Aqua',
        'ThrottleInterval': 30,
    }, stream)
PY
