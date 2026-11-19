#!/bin/zsh
# Builds the "Two hour planning" eval case: the gold files beside this script, and the test
# library's take of that meeting linked in as its audio. Reads the library only. Prints the
# case folder.
#
#   scripts/fixtures/two-hour-planning.sh [CASES_DIR] [LIBRARY]
#
# CASES_DIR defaults to ~/Library/Application Support/dory-dev/eval-cases, and LIBRARY to the
# test library at ~/Library/Application Support/dory-dev/live-data. Then score it with:
#   dory-cli eval CASES_DIR/two-hour-planning --label baseline --tsv runs.tsv
set -euo pipefail

here=${0:A:h}
cases=${1:-$HOME/Library/Application Support/dory-dev/eval-cases}
library=${2:-$HOME/Library/Application Support/dory-dev/live-data}
id=$(sqlite3 -readonly "$library/dory.sqlite" \
  "select id from recordings where title = 'Two hour planning' order by created_at limit 1")
take="$library/takes/$id.flac"
if [[ -z $id || ! -f $take ]]; then
  print -u2 "two-hour-planning.sh: no FLAC take titled \"Two hour planning\" in $library"
  exit 1
fi
case_dir="$cases/two-hour-planning"
mkdir -p "$case_dir"
cp "$here/two-hour-planning/"{expected.txt,terms.txt,language} "$case_dir/"
ln -sfn "$take" "$case_dir/take.flac"
print -r -- "$case_dir"
