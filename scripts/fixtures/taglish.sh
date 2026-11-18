#!/bin/zsh
# Joins the test library's "Weekly sync" (English) and "Pulong sa Tagalog" (Tagalog) takes into one
# mono 48 kHz FLAC, a Taglish meeting for lanes, and prints its path. Reads the library only. Uses
# macOS's own afconvert, so it needs nothing from Homebrew.
#
#   DORY_DATA_DIR=/tmp/dory-ai-lane-10 scripts/fixtures/taglish.sh [OUT.flac]
#
# Then add it with: dory-cli add OUT.flac --title "Taglish sync"
set -euo pipefail

data=${DORY_DATA_DIR:?set DORY_DATA_DIR to a copy of the test library}
out=${1:-${TMPDIR:-/tmp}/taglish.flac}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

take() {
  local id
  id=$(sqlite3 -readonly "$data/dory.sqlite" \
    "select id from recordings where title = '$1' order by created_at limit 1")
  if [[ -z $id || ! -f "$data/takes/$id.flac" ]]; then
    print -u2 "taglish.sh: no FLAC take titled \"$1\" in $data"
    exit 1
  fi
  print -r -- "$data/takes/$id.flac"
}

afconvert -f WAVE -d LEI16@48000 -c 1 "$(take "Weekly sync")" "$work/english.wav"
afconvert -f WAVE -d LEI16@48000 -c 1 "$(take "Pulong sa Tagalog")" "$work/tagalog.wav"
python3 -I - "$work/english.wav" "$work/tagalog.wav" "$work/taglish.wav" <<'PY'
import sys, wave
*parts, joined = sys.argv[1:]
with wave.open(joined, "wb") as out:
    for i, part in enumerate(parts):
        with wave.open(part, "rb") as w:
            if i == 0:
                out.setparams(w.getparams())
            out.writeframes(w.readframes(w.getnframes()))
PY
afconvert -f flac -d flac "$work/taglish.wav" "$out"
print -r -- "$out"
