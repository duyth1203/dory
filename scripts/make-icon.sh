#!/usr/bin/env bash
# Builds Dory's app icon from the art in apps/dory-gpui/icon/source.png.
#
#   scripts/make-icon.sh [SOURCE_PNG]
#
# The art is Dory's rounded icon shape drawn on black. macOS 26 masks an app icon to its own
# squircle and frames any icon whose shape it does not recognise in a grey tile, so Dory ships an
# Icon Composer document instead of a pre-cut shape:
#
# - apps/dory-gpui/icon/Dory.icon holds one full-bleed square layer, cropped from inside the drawn
#   shape so no black corner or rim is left for the system mask to show.
# - apps/dory-gpui/Dory.icns is actool's rendering of that document, for anything that still
#   reads CFBundleIconFile.
#
# scripts/bundle-macos.sh compiles the same document into Contents/Resources/Assets.car, which
# macOS 26 reads through CFBundleIconName. Needs ImageMagick (`magick`) and Xcode 26's `actool`.
set -euo pipefail
cd "$(dirname "$0")/.."

src="${1:-apps/dory-gpui/icon/source.png}"
doc="apps/dory-gpui/icon/Dory.icon"
icns="apps/dory-gpui/Dory.icns"
command -v magick >/dev/null || { echo "make-icon: ImageMagick is missing (brew install imagemagick)" >&2; exit 1; }

# The shape's bounds: everything brighter than the black around it.
read -r bw bh bx by < <(magick "$src" -colorspace gray -threshold 6% -format '%@\n' info: | tr 'x+' '  ')
# The largest centred square inside the shape, less an inset that clears its rounded black
# corners and its drawn rim, scaled to the 1024 pt layer Icon Composer expects.
inset=70
side=$(((bw < bh ? bw : bh) - 2 * inset))
x=$((bx + (bw - side) / 2)) y=$((by + (bh - side) / 2))
mkdir -p "$doc/Assets"
magick "$src" -crop "${side}x${side}+${x}+${y}" +repage -resize '1024x1024!' "$doc/Assets/art.png"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
scripts/compile-icon.sh "$work"
cp "$work/Dory.icns" "$icns"
# The About window shows the 128 pt icon at 2x.
iconutil -c iconset "$icns" -o "$work/Dory.iconset"
cp "$work/Dory.iconset/icon_128x128@2x.png" apps/dory-gpui/assets/about/dory-icon.png
echo "$doc"
echo "$icns"
