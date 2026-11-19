#!/usr/bin/env bash
# Compiles apps/dory-gpui/icon/Dory.icon into OUT_DIR/Assets.car and OUT_DIR/Dory.icns.
#
#   scripts/compile-icon.sh OUT_DIR
#
# macOS 26 draws the app icon from Assets.car through CFBundleIconName (apps/dory-gpui/Info.plist)
# and masks it to the system squircle itself. Needs Xcode 26's `actool`.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:?usage: scripts/compile-icon.sh OUT_DIR}"
mkdir -p "$out"
log="$out/actool.log"
if ! xcrun actool apps/dory-gpui/icon/Dory.icon --compile "$out" --app-icon Dory \
  --enable-on-demand-resources NO --development-region en --target-device mac \
  --platform macosx --minimum-deployment-target 26.0 \
  --output-partial-info-plist "$out/partial.plist" >"$log" 2>&1; then
  echo "compile-icon: actool failed; see $log" >&2
  exit 1
fi
for f in Assets.car Dory.icns; do
  [ -s "$out/$f" ] || { echo "compile-icon: actool wrote no $f; see $log" >&2; exit 1; }
done
