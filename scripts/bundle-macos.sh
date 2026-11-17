#!/usr/bin/env bash
# Builds a release Dory.app (bundle id com.dory.app) with cargo-packager and prints its path.
#
#   scripts/bundle-macos.sh [OUT_DIR]
#
# OUT_DIR defaults to $CARGO_TARGET_DIR/bundle, or target/bundle. sherpa-onnx and the Apple
# Intelligence bridge link statically, so the bundle holds two executables and no dylibs: the app,
# and `dory-mcp`, the helper an assistant launches from `Contents/MacOS/dory-mcp` to read the
# library. The build needs Xcode's swiftc and the macOS 26.4 SDK or later.
#
# Signing: DORY_SIGNING_IDENTITY (for example "Developer ID Application: Name (TEAMID)") signs
# with that identity and the hardened runtime. Unset, the repo's local git config
# `dory.signingIdentity` names the identity, which keeps a personal certificate out of the repo:
#
#   git config dory.signingIdentity "My Code Signing Certificate"
#
# Any stable identity, self-signed included, keeps macOS's microphone and audio grants across
# rebuilds. With neither set, the app is signed ad hoc and macOS drops those grants on every
# rebuild.
#
# Notarizing: only when a real identity is set and cargo-packager's credentials are in the
# environment, either APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID, or APPLE_API_KEY,
# APPLE_API_ISSUER and APPLE_API_KEY_PATH.
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/require-swift.sh
command -v cargo-packager >/dev/null || {
  echo "cargo-packager is missing. Run: cargo install cargo-packager --locked" >&2
  exit 1
}

target="${CARGO_TARGET_DIR:-target}"
out="${1:-$target/bundle}"
identity="${DORY_SIGNING_IDENTITY:-$(git config --get dory.signingIdentity || echo -)}"
version="$(cargo metadata --no-deps --format-version 1 |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="dory-gpui"))')"

if [ "$identity" = "-" ]; then
  echo "bundle: no signing identity set, signing ad hoc and skipping notarization" >&2
  unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH
else
  echo "bundle: signing with \"$identity\"" >&2
fi
if [ "$identity" != "-" ] && [ -z "${APPLE_ID:-}${APPLE_API_KEY:-}" ]; then
  echo "bundle: no notarization credentials in the environment, skipping notarization" >&2
fi

# Two invocations, so cargo does not unify the app's features into dory-mcp, which links no Swift.
cargo build --release -p dory-gpui
cargo build --release -p dory-mcp

mkdir -p "$out"
rm -rf "$out/Dory.app"
config="$(python3 - "$target/release" "$out" "$version" "$identity" <<'PY'
import json, os, sys
binaries_dir, out, version, identity = sys.argv[1:]
print(json.dumps({
    "productName": "Dory",
    "version": version,
    "identifier": "com.dory.app",
    "outDir": os.path.abspath(out),
    "binariesDir": os.path.abspath(binaries_dir),
    "binaries": [
        {"path": "dory-gpui", "main": True},
        {"path": "dory-mcp", "main": False},
    ],
    "formats": ["app"],
    "icons": ["apps/dory-gpui/Dory.icns"],
    "macos": {
        "minimumSystemVersion": "26.0",
        "infoPlistPath": "apps/dory-gpui/Info.plist",
        "entitlements": "apps/dory-gpui/Dory.entitlements",
        "signingIdentity": identity,
    },
}))
PY
)"
cargo packager --config "$config" --formats app --release

app="$out/Dory.app"
[ -d "$app" ] || { echo "bundle: cargo-packager did not produce $app" >&2; exit 1; }
codesign --verify --deep --strict "$app"
echo "$app"
