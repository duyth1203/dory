#!/usr/bin/env bash
# Wraps the dory-gpui binary in an ad-hoc signed .app, so macOS and the AX tools see a bundle id.
# Usage: apps/dory-gpui/bundle.sh [debug|release]. Prints the .app path.
set -euo pipefail
cd "$(dirname "$0")/../.."
profile="${1:-debug}"
target="${CARGO_TARGET_DIR:-target}"
if [ "$profile" = release ]; then cargo build --release -p dory-gpui; else cargo build -p dory-gpui; fi
app="$target/$profile/DoryDev.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$target/$profile/dory-gpui" "$app/Contents/MacOS/DoryDev"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.dory.gpui-dev</string>
  <key>CFBundleName</key><string>DoryDev</string>
  <key>CFBundleExecutable</key><string>DoryDev</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Dory records your side of the call.</string>
  <key>NSAudioCaptureUsageDescription</key><string>Dory records the other side of the call.</string>
</dict></plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"
