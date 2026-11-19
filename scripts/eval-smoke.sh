#!/usr/bin/env bash
# Runs `dory-cli eval` on one throwaway case spoken by macOS `say`, with the fake engines.
# Usage: scripts/eval-smoke.sh [extra eval flags]
#
# It checks the harness end to end: the case loads, the audio imports, the pipeline runs, and the
# scores print. The fake engines write fixed text, so the numbers mean nothing. $DORY_DATA_DIR
# defaults to a temp folder, and the case folder is deleted when the script ends.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/dory-eval-smoke.XXXXXX")
trap 'rm -rf "$work"' EXIT

spoken="Omar will send the pricing deck by Friday. We decided to ship the beta on Monday."
mkdir "$work/smoke"
say -o "$work/take.aiff" "$spoken"
afconvert -f WAVE -d LEI16@16000 -c 1 "$work/take.aiff" "$work/smoke/take.wav"
echo "$spoken" > "$work/smoke/reference.txt"
printf '# names\nOmar\nbeta\n' > "$work/smoke/terms.txt"
printf 'action: omar, pricing, deck|slides, friday\ndecision: ship, beta, monday\n' > "$work/smoke/expected.txt"

export DORY_DATA_DIR="${DORY_DATA_DIR:-$work/data}"
cd "$root"
cargo run -q -p dory-cli -- eval "$work/smoke" --fake-engines "$@"
