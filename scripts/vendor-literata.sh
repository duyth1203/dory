#!/usr/bin/env bash
# Vendors the Literata faces the Book theme reads in into apps/dory-gpui/assets/fonts/literata/,
# from one pinned release of the official repository (googlefonts/literata, SIL OFL 1.1).
# The app embeds them with include_bytes!, so every Mac renders the same type.
#
#   scripts/vendor-literata.sh     fetch the pinned release and check its checksum
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
dest="$repo/apps/dory-gpui/assets/fonts/literata"
version=3.103
sha256=f7fb973cafb26cf785cbebaeaf51c18f87c15a3bcf4d82a7d4857564db5b056d
url="https://github.com/googlefonts/literata/releases/download/$version/$version.zip"

# Keep in step with `FACES` in apps/dory-gpui/src/fonts.rs; its tests fail on a mismatch.
faces=(Literata-Regular Literata-Italic Literata-Medium Literata-SemiBold)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -sfL --retry 2 -o "$work/release.zip" "$url"
echo "$sha256  $work/release.zip" | shasum -a 256 -c - >/dev/null || {
  echo "Literata $version: checksum mismatch" >&2
  exit 1
}
names=(OFL.txt)
for face in "${faces[@]}"; do names+=("fonts/ttf/$face.ttf"); done
unzip -q -o "$work/release.zip" "${names[@]}" -d "$work/out"

for face in "${faces[@]}"; do
  file="$work/out/fonts/ttf/$face.ttf"
  # A TrueType font starts with version 1.0 (00 01 00 00).
  [[ $(head -c 4 "$file" | xxd -p) == 00010000 ]] || { echo "$face: not TrueType" >&2; exit 1; }
done

rm -rf "$dest"
mkdir -p "$dest"
for face in "${faces[@]}"; do cp "$work/out/fonts/ttf/$face.ttf" "$dest/"; done
cp "$work/out/OFL.txt" "$dest/OFL.txt"
echo "$version" >"$dest/VERSION"
bytes=$(cat "$dest"/*.ttf | wc -c | tr -d ' ')
echo "Literata $version: ${#faces[@]} faces, $bytes bytes"
