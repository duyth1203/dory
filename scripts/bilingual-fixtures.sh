#!/usr/bin/env bash
# Builds the bilingual bench's public audio and adds it to a Dory library.
# Usage: scripts/bilingual-fixtures.sh [--dry-run] <data dir>
#
# <data dir> is a Dory data folder that holds the "Pulong sa Tagalog" and "Weekly sync" takes.
# Use a throwaway copy, never the real library. The script
#   1. downloads the 24 CS-FLEURS tgl_eng clips listed in docs/plans/bilingual-spike/clips.tsv
#      into fixtures/public/cs-fleurs/raw/, and checks each SHA-256,
#   2. converts them to 16 kHz mono 16-bit WAV and writes fixtures/public/cs-fleurs/refs.tsv,
#   3. builds the 11 turn-switch clips in fixtures/public/turn-switch/ with
#      scripts/bilingual_turns.py, and
#   4. adds every clip as a recording with `dory-cli add`, skipping titles already in the library.
# A second run downloads nothing and adds nothing. A clip whose SHA-256 is wrong is deleted, named,
# and the script exits 1 before it adds anything. CS-FLEURS is CC BY-NC 4.0, so fixtures/ stays
# out of git. $DORY_CLI overrides the dory-cli command (default: cargo run -q -p dory-cli --).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
clips_tsv="$root/docs/plans/bilingual-spike/clips.tsv"
cs_dir="$root/fixtures/public/cs-fleurs"
turns_dir="$root/fixtures/public/turn-switch"
metadata_url="https://huggingface.co/datasets/byan/cs-fleurs/resolve/main/mms/test/metadata.jsonl"
metadata_sha="48c065a79260058951ba2f25e58c865bea313874beb15fc07db7e72b6bc63f37"

dry_run=0
data_dir=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    -*) echo "unknown flag $arg" >&2; exit 2 ;;
    *) [ -z "$data_dir" ] || { echo "one data dir only" >&2; exit 2; }; data_dir="$arg" ;;
  esac
done
[ -n "$data_dir" ] || { echo "usage: scripts/bilingual-fixtures.sh [--dry-run] <data dir>" >&2; exit 2; }

read -r -a cli <<< "${DORY_CLI:-cargo run --quiet -p dory-cli --}"
sha_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
note() { printf '%s\n' "$*"; }

# Prints the file, sha256, and url of each clip, without the header row.
clip_rows() { tail -n +2 "$clips_tsv" | cut -f1,2,4; }

library_titles() {
  [ -d "$data_dir" ] || return 0
  # A dry run never builds: it reads the library only when $DORY_CLI names a built dory-cli.
  [ "$dry_run" = 0 ] || [ -n "${DORY_CLI:-}" ] || return 0
  DORY_DATA_DIR="$data_dir" "${cli[@]}" list 2>/dev/null || true
}

if [ "$dry_run" = 1 ]; then
  titles=$(library_titles)
  rows=$(clip_rows | wc -l | tr -d ' ')
  note "dry run: $rows clips in $clips_tsv, nothing is changed"
  while IFS=$'\t' read -r file sha url; do
    if [ -f "$cs_dir/raw/$file" ] && [ "$(sha_of "$cs_dir/raw/$file")" = "$sha" ]; then
      note "have     $file"
    else
      note "download $file $url"
    fi
  done < <(clip_rows)
  [ -f "$cs_dir/raw/metadata.jsonl" ] || note "download metadata.jsonl $metadata_url"
  note "convert  $rows clips to 16 kHz mono WAV in $cs_dir"
  note "build    11 turn-switch clips in $turns_dir:"
  python3 -I "$root/scripts/bilingual_turns.py" --list | sed 's/^/           /'
  while IFS=$'\t' read -r file _ _; do
    title="cs-fleurs ${file%.wav}"
    grep -qF -- "  $title  " <<< "$titles" || note "add      $title"
  done < <(clip_rows)
  for name in $(python3 -I "$root/scripts/bilingual_turns.py" --list); do
    title="turn-switch $name"
    grep -qF -- "  $title  " <<< "$titles" || note "add      $title"
  done
  exit 0
fi

[ -d "$data_dir" ] || { echo "$data_dir is not a folder" >&2; exit 2; }
for tool in curl shasum afconvert python3; do
  command -v "$tool" > /dev/null || { echo "$tool is required" >&2; exit 2; }
done
mkdir -p "$cs_dir/raw" "$turns_dir"

failed=0
fetch() { # file, sha256, url
  local file=$1 sha=$2 url=$3 path="$cs_dir/raw/$1"
  if [ ! -f "$path" ]; then
    note "downloading $file"
    curl --fail --location --silent --show-error --output "$path.part" "$url"
    mv "$path.part" "$path"
  fi
  if [ "$(sha_of "$path")" != "$sha" ]; then
    echo "sha256 mismatch for $file, deleting it" >&2
    rm -f "$path" "$cs_dir/${file}"
    failed=1
  fi
}

fetch metadata.jsonl "$metadata_sha" "$metadata_url"
while IFS=$'\t' read -r file sha url; do
  fetch "$file" "$sha" "$url"
done < <(clip_rows)
[ "$failed" = 0 ] || { echo "bad checksums, nothing was added" >&2; exit 1; }
note "all checksums match"

while IFS=$'\t' read -r file _ _; do
  if [ ! -f "$cs_dir/$file" ] || [ "$cs_dir/raw/$file" -nt "$cs_dir/$file" ]; then
    afconvert -f WAVE -d LEI16@16000 -c 1 "$cs_dir/raw/$file" "$cs_dir/$file.part"
    mv "$cs_dir/$file.part" "$cs_dir/$file"
  fi
done < <(clip_rows)

python3 -I - "$cs_dir/raw/metadata.jsonl" "$clips_tsv" "$cs_dir/refs.tsv" << 'PY'
import json, sys
metadata, clips, out = sys.argv[1:]
text = {}
for line in open(metadata):
    row = json.loads(line)
    text[row['id']] = ' '.join(row['text'].split())
with open(out, 'w') as f:
    for line in list(open(clips))[1:]:
        file = line.split('\t')[0]
        f.write(f"{file}\t{text['tgl_eng_' + file[:-4]]}\n")
PY

python3 -I "$root/scripts/bilingual_turns.py" "$data_dir" "$turns_dir"

titles=$(library_titles)
added=0
add() { # wav, title
  if grep -qF -- "  $2  " <<< "$titles"; then
    return 0
  fi
  DORY_DATA_DIR="$data_dir" "${cli[@]}" add "$1" --title "$2" > /dev/null
  added=$((added + 1))
}
while IFS=$'\t' read -r file _ _; do
  add "$cs_dir/$file" "cs-fleurs ${file%.wav}"
done < <(clip_rows)
for name in $(python3 -I "$root/scripts/bilingual_turns.py" --list); do
  add "$turns_dir/$name.wav" "turn-switch $name"
done
note "added $added recordings to $data_dir"
