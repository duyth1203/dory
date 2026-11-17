#!/usr/bin/env bash
# The visual pass: opens every view through DORY_SHOW on a fresh copy of the test library, in
# light, dark, and Increase Contrast, and captures Dory's own windows with `screencapture -l`.
# It never captures the screen or another app, and never sends the app input.
#
# Usage: scripts/visual-pass.sh <out-dir> [view ...]
#   With no views it runs the whole set, then the main and Settings windows at 800 x 560 (Settings
#   at its minimum height) and 200% text.
# Environment:
#   CARGO_TARGET_DIR     where cargo builds (default: ./target)
#   VISUAL_CARGO_ARGS    extra cargo arguments, split on spaces
#   VISUAL_LIBRARY       the library to copy (default: ~/Library/Application Support/dory-dev/live-data)
#   VISUAL_THEMES        themes to run (default: "light dark contrast")
#   VISUAL_SETTLE        seconds to wait after the view is ready (default: 1.5)
#   VISUAL_SMALL         1 adds the 800 x 560, 200% pass (default: 1 for the whole set, else 0)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:?usage: scripts/visual-pass.sh <out-dir> [view ...]}
shift
mkdir -p "$out"
out=$(cd "$out" && pwd)
library=${VISUAL_LIBRARY:-"$HOME/Library/Application Support/dory-dev/live-data"}
target=${CARGO_TARGET_DIR:-"$root/target"}
themes=${VISUAL_THEMES:-"light dark contrast"}
[ -d "$library" ] || { echo "visual-pass: no library at $library" >&2; exit 1; }

views=("$@")
small=${VISUAL_SMALL:-0}
if [ ${#views[@]} -eq 0 ]; then
  small=${VISUAL_SMALL:-1}
  views=(
    settings:general settings:recording settings:transcription settings:summaries settings:storage
    settings:transcription:languages-open explainer about queue
    queue:running queue:light queue:blocked queue:finished queue:empty queue:busy
    summaries:U1 summaries:U2 summaries:U3
    "recording:Weekly sync" "recording:Weekly sync:transcript"
    "recording:Pulong sa Tagalog" "recording:Pulong sa Tagalog:transcript"
    "recording:Two hour planning" "recording:Two hour planning:transcript"
    "phase:P1:Weekly sync" "phase:P2:Weekly sync" "phase:P3a:Weekly sync" "phase:P3b:Weekly sync"
    "phase:P4:Weekly sync" "phase:P5:Weekly sync" "phase:P6:Weekly sync" "phase:P7:Weekly sync"
    "phase:P8:Weekly sync" phase:P4 phase:P8
  )
fi

# shellcheck disable=SC2086
(cd "$root" && cargo build -p dory-gpui ${VISUAL_CARGO_ARGS:-})
app="$out/.build/DoryVisual.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$target/debug/dory-gpui" "$app/Contents/MacOS/dory-gpui"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.dory.visual-pass</string>
  <key>CFBundleName</key><string>Dory</string>
  <key>CFBundleExecutable</key><string>dory-gpui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Dory records your side of the call.</string>
  <key>NSAudioCaptureUsageDescription</key><string>Dory records the other side of the call.</string>
</dict></plist>
PLIST
identity=$(git -C "$root" config dory.signingIdentity || true)
codesign --force --sign "${identity:--}" "$app" >/dev/null 2>&1

index="$out/index.md"
{
  echo "# Visual pass"
  echo
  echo "Built from $(git -C "$root" rev-parse --short HEAD) on $(date '+%Y-%m-%d %H:%M')."
  echo
  echo "| File | View | Theme | Window |"
  echo "| --- | --- | --- | --- |"
} > "$index"

slug() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9\n' '-' | tr -s '-'; }

# capture <view> <theme> <width> <height> <text-scale> <suffix>
capture() {
  local view=$1 theme=$2 width=$3 height=$4 scale=$5 suffix=$6
  local lib log name appearance prefs_env=() pid=""
  lib=$(mktemp -d /tmp/dory-visual.XXXXXX)
  log="$lib.log"
  cp -cR "$library/." "$lib/"
  rm -f "$lib/.lock"
  printf '{"version":1,"onboarding":"done","window_width":%s,"window_height":%s,"text_scale":%s}\n' \
    "$width" "$height" "$scale" > "$lib/ui-prefs.json"
  case $theme in
    dark) appearance=dark ;;
    contrast) appearance=light; prefs_env=(--env DORY_SYSTEM_PREFS=increase-contrast) ;;
    *) appearance=light ;;
  esac
  name="$(slug "$view")-$theme$suffix"
  : > "$log"
  open -n --stderr "$log" --env "DORY_DATA_DIR=$lib" --env "DORY_SHOW=$view" \
    --env "DORY_APPEARANCE=$appearance" ${prefs_env[@]+"${prefs_env[@]}"} "$app"
  local ok=0
  for _ in $(seq 1 300); do
    if grep -q '^show: ready' "$log"; then ok=1; break; fi
    if grep -q '^dory: ' "$log" && ! grep -q '^dory: data folder' "$log"; then break; fi
    if grep -q 'panicked' "$log"; then break; fi
    sleep 0.1
  done
  # Lets the frame after the view lands paint, and the explainer and popovers settle.
  sleep "${VISUAL_SETTLE:-1.5}"
  pid=$(sed -n 's/^first-frame: window opened, pid \([0-9]*\)$/\1/p' "$log" | head -1)
  local ids
  ids=$(sed -n 's/^window-id: \([0-9]*\)$/\1/p' "$log")
  if [ $ok -eq 0 ] || [ -z "$ids" ]; then
    echo "| (none) | \`$view\` | $theme$suffix | not captured: $(grep -m1 -E '^(dory|show):' "$log" | tr '|' '/') |" >> "$index"
    echo "visual-pass: $name not captured, log at $log" >&2
  else
    local count n=0 file role
    count=$(printf '%s\n' "$ids" | wc -l | tr -d ' ')
    for id in $ids; do
      n=$((n + 1))
      if [ "$count" -gt 1 ] && [ $n -lt "$count" ]; then
        file="$name-main.png"; role=main
      else
        file="$name.png"; role=$([ "$count" -gt 1 ] && echo view || echo main)
      fi
      if screencapture -x -l "$id" "$out/$file"; then
        echo "| [$file]($file) | \`$view\` | $theme$suffix | $role ($id) |" >> "$index"
      else
        echo "| (none) | \`$view\` | $theme$suffix | $role ($id) not captured: screencapture failed |" >> "$index"
      fi
    done
    echo "visual-pass: $name"
  fi
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$pid" 2>/dev/null || true
    while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
  fi
  [ $ok -eq 1 ] && rm -rf "$lib" "$log"
  return 0
}

for theme in $themes; do
  for view in "${views[@]}"; do
    capture "$view" "$theme" 1100 720 1.0 ""
  done
done

if [ $small -eq 1 ]; then
  for view in "${views[@]}"; do
    case $view in
      about) continue ;;
      settings:*) view="$view:min-height" ;;
    esac
    capture "$view" light 800 560 2.0 "-800x560-200"
  done
fi

echo "visual-pass: index at $index"
