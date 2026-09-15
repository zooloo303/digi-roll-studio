#!/usr/bin/env bash
# Independent preview bundle; does not install or replace the stable application.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="$ROOT/target/plugin-preview"
cargo build --release -p digi_roll_studio --features plugin-host
APP="$ROOT/dist/Digi-Roll Studio - Plugin Preview.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$CARGO_TARGET_DIR/release/digi_roll_studio" "$APP/Contents/MacOS/drs-plugin-preview"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>drs-plugin-preview</string>
<key>CFBundleIdentifier</key><string>io.github.zooloo303.digi-roll-studio.plugin-preview</string>
<key>CFBundleName</key><string>Digi-Roll Studio — Plugin Preview</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
git rev-parse HEAD > "$APP/Contents/Resources/source-commit.txt"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
printf '%s\n' "$APP"
