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
codesign --force --sign - "$app/Contents/MacOS/hx-agent"
codesign --force --sign - "$app"
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
