#!/usr/bin/env bash
# Builds apps/dory-gpui/Dory.icns from the icon art in apps/dory-gpui/icon/source.png.
#
#   scripts/make-icon.sh [SOURCE_PNG]
#
# The art is a full-bleed square: Dory's rounded icon shape drawn on pure black. The script
# turns the black outside the shape transparent by flood fill from the corners (so blacks inside
# the art, like the eye, stay), crops to the shape, sets it on Apple's macOS icon grid (an
# 824 pt body on a 1024 canvas, with the standard soft shadow), and writes every .icns size.
# Needs ImageMagick (`magick`) and macOS's `iconutil`.
set -euo pipefail
cd "$(dirname "$0")/.."

src="${1:-apps/dory-gpui/icon/source.png}"
icns="apps/dory-gpui/Dory.icns"
command -v magick >/dev/null || { echo "make-icon: ImageMagick is missing (brew install imagemagick)" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

read -r w h < <(magick identify -format '%w %h\n' "$src")
max_x=$((w - 1)) max_y=$((h - 1))

# Outer black to transparent, then a half-pixel feather on the cut edge only.
magick "$src" -alpha set -fuzz 1.2% -fill none \
  -draw "color 0,0 floodfill" -draw "color $max_x,0 floodfill" \
  -draw "color 0,$max_y floodfill" -draw "color $max_x,$max_y floodfill" \
  \( +clone -alpha extract -blur 0x0.6 \) -compose CopyOpacity -composite \
  "$work/cut.png"

# Square crop around the shape, so it scales without distortion.
read -r bw bh bx by < <(magick "$work/cut.png" -trim -format '%w %h %X %Y\n' info: | tr -d '+')
side=$((bw > bh ? bw : bh))
cx=$((bx + bw / 2)) cy=$((by + bh / 2))
magick "$work/cut.png" -crop "${side}x${side}+$((cx - side / 2))+$((cy - side / 2))" +repage \
  -background none -gravity center -extent "${side}x${side}" "$work/shape.png"

# Apple's grid: an 824 body centred on 1024, shadow 30% black, 10 px blur, 10 px down.
magick "$work/shape.png" -resize 824x824 "$work/body.png"
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
