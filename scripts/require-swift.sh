#!/usr/bin/env bash
# Fails with one sentence unless Xcode's swiftc and the macOS 26.4 SDK or later are installed.
# dory-macos's build.rs compiles swift/DoryAI.swift with them, and Foundation Models'
# `contextSize` first ships in that SDK.
set -euo pipefail

xcrun --sdk macosx --find swiftc >/dev/null 2>&1 || {
  echo "Dory needs Xcode's swiftc to build Apple Intelligence support; install Xcode and run: xcode-select -s /Applications/Xcode.app" >&2
  exit 1
}
sdk="$(xcrun --sdk macosx --show-sdk-version 2>/dev/null || echo 0)"
printf '%s\n' 26.4 "$sdk" | sort -V -C || {
  echo "Dory needs the macOS 26.4 SDK or later to build Apple Intelligence support, and Xcode has $sdk; update Xcode." >&2
  exit 1
}
