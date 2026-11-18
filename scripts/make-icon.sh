#!/usr/bin/env bash
# Builds apps/dory-gpui/Dory.icns from the icon art in apps/dory-gpui/icon/source.png.
#
#   scripts/make-icon.sh [SOURCE_PNG]
#
# The art is a full-bleed square: Dory's rounded icon shape drawn on black. The script finds the
# shape, cuts it with a clean rounded-rectangle mask (so the black around it and its dark rim go,
# while blacks inside the art, like the eye, stay), sets it on Apple's macOS icon grid (an 824 pt
# body on a 1024 canvas, with the standard soft shadow), and writes every .icns size.
# Needs ImageMagick (`magick`) and macOS's `iconutil`.
set -euo pipefail
cd "$(dirname "$0")/.."

src="${1:-apps/dory-gpui/icon/source.png}"
icns="apps/dory-gpui/Dory.icns"
command -v magick >/dev/null || { echo "make-icon: ImageMagick is missing (brew install imagemagick)" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The shape's bounds: everything brighter than the black around it.
read -r bw bh bx by < <(magick "$src" -colorspace gray -threshold 6% -format '%@\n' info: | tr 'x+' '  ')
# Trim the dark anti-aliased rim, then cut a clean rounded rectangle (Apple's corner, 22.37% of
# the side) drawn at 4x and scaled down, so the edge is smooth on any background.
inset=6
w=$((bw - 2 * inset)) h=$((bh - 2 * inset))
side=$((w < h ? w : h))
r=$((side * 2237 / 10000))
magick -size $((w * 4))x$((h * 4)) xc:black -fill white \
  -draw "roundrectangle 0,0 $((w * 4 - 1)),$((h * 4 - 1)) $((r * 4)),$((r * 4))" \
  -resize "${w}x${h}!" "$work/mask.png"
magick "$src" -crop "${w}x${h}+$((bx + inset))+$((by + inset))" +repage \
  "$work/mask.png" -alpha off -compose CopyOpacity -composite "$work/shape.png"

# Apple's grid: an 824 body centred on 1024, shadow 30% black, 10 px blur, 10 px down.
magick "$work/shape.png" -resize '824x824!' "$work/body.png"
magick -size 1024x1024 xc:none \
  \( "$work/body.png" -background black -shadow 30x10+0+10 \) -gravity center -geometry +0+5 -composite \
  "$work/body.png" -gravity center -geometry +0+0 -composite \
  "$work/icon-1024.png"

set_dir="$work/Dory.iconset"
mkdir -p "$set_dir"
for size in 16 32 128 256 512; do
  magick "$work/icon-1024.png" -resize "${size}x${size}" "$set_dir/icon_${size}x${size}.png"
  magick "$work/icon-1024.png" -resize "$((size * 2))x$((size * 2))" "$set_dir/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$set_dir" -o "$icns"
echo "$icns"
