#!/usr/bin/env bash
# Vendors the Lucide icons Dory draws into apps/dory-gpui/assets/icons/, from one pinned
# lucide-static release. Safe to rerun: it rewrites the folder from the list below.
#
#   scripts/vendor-lucide.sh            fetch VERSION's release
#   scripts/vendor-lucide.sh 1.66.0     move to another release
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
dest="$repo/apps/dory-gpui/assets/icons"
version=${1:-$(cat "$dest/VERSION")}

# Keep in step with `Icon` in apps/dory-gpui/src/icons.rs; its tests fail on a mismatch.
icons=(
  battery-medium bot captions check chevron-down chevron-right chevrons-up-down circle
  circle-check circle-dashed circle-stop circle-x clock copy ellipsis file-text grip-vertical
  hard-drive info list list-filter mic pause play plus search settings-2 square triangle-alert x
)
# The 20 to 22 pt transport controls draw at 1.5 stroke, which Lucide sets inside each file.
transport=(pause play rotate-ccw rotate-cw)

base="https://cdn.jsdelivr.net/npm/lucide-static@$version"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() {
  curl -sfL --retry 2 -o "$work/$2" "$base/$1"
}

# A plain icon: one <svg> of paths and shapes, with no script, styles, links, or handlers.
check() {
  local file=$1
  head -c 5 "$file" | grep -q '^<' || { echo "$file: not markup" >&2; exit 1; }
  grep -q '<svg' "$file" || { echo "$file: no <svg>" >&2; exit 1; }
  if grep -Eiq '<script|<style|<foreignObject|<image|<use|href|javascript:|[[:space:]]on[a-z]+=' "$file"; then
    echo "$file: has script, styles, links, or handlers" >&2
    exit 1
  fi
}

fetch LICENSE LICENSE
for name in "${icons[@]}"; do
  fetch "icons/$name.svg" "$name.svg"
  check "$work/$name.svg"
done
for name in "${transport[@]}"; do
  [[ -f "$work/$name.svg" ]] || fetch "icons/$name.svg" "$name.svg"
  check "$work/$name.svg"
  grep -q 'stroke-width="2"' "$work/$name.svg" || { echo "$name: no stroke-width=2" >&2; exit 1; }
  sed 's/stroke-width="2"/stroke-width="1.5"/' "$work/$name.svg" >"$work/transport-$name.svg"
done
for name in "${transport[@]}"; do
  [[ " ${icons[*]} " == *" $name "* ]] || rm "$work/$name.svg"
done

rm -f "$dest"/*.svg
mkdir -p "$dest"
cp "$work"/*.svg "$dest/"
cp "$work/LICENSE" "$dest/LICENSE"
echo "$version" >"$dest/VERSION"
count=$(find "$dest" -name '*.svg' | wc -l | tr -d ' ')
bytes=$(cat "$dest"/*.svg | wc -c | tr -d ' ')
echo "lucide-static $version: $count icons, $bytes bytes"
