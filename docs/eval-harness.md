# Accuracy eval harness

`dory-cli eval` runs real audio through the real Dory pipeline and scores what comes out. It exists so a change to hotwords, beam search, the summary prompt, or diarization can be compared with a measured baseline instead of a hunch.

## What it measures

- **Word error rate.** The transcript against a hand-corrected reference, when the case has one. Substitutions, deletions, and insertions are counted separately. The corpus rate is the summed errors over the summed reference words, so a long case weighs more than a short one.
- **Term recall.** How many of the names and jargon words in `terms.txt` the transcript got right. Each occurrence counts once, from the reference or from the term's own count. The transcript earns at most as many hits as the reference has.
- **Summary recall.** How many of the action items and decisions in `expected.txt` appear in the summary, and how many bullets match nothing expected. Action items are read from the Action Items section and decisions from the Decisions section. A summary that lacks the three sections is read as one list and flagged unstructured. Each bullet pairs with at most one expected item, so a summary that repeats an item finds it once and counts the copy as extra.
- **Time.** Seconds spent in the transcribe stage and in the summarize stage, and the transcribe time per second of audio.

Words are folded with the search index's rules: lowercase, no accents, no punctuation. "Café," and "cafe" are the same word.

## Case folders

A case is a folder:

```
standup/
  take.m4a          exactly one audio file
  reference.txt     optional, the transcript corrected by hand
  terms.txt         optional, one term per line
  expected.txt      optional, one gold item per line
  language          optional, one BCP-47 tag
```

The audio can be `wav`, `flac`, `mp3`, `m4a`, `aac`, `ogg`, `opus`, `aif`, `aiff`, `caf`, `mp4`, or `webm`. Dory reads `wav` and `flac` itself and decodes the rest with ffmpeg.

`reference.txt` is plain words. Leave out speaker names and timestamps. A meeting too long to correct by hand can go without one. Word error rate is then not scored, and every term needs a count. A case needs at least one of `reference.txt`, `terms.txt`, and `expected.txt`.

`terms.txt` lists names and jargon. A term may be several words. Blank lines and lines that start with `#` are ignored. A term the reference never says is reported and not scored, because it is probably a typo. A term may end in `*N` to say the meeting says it `N` times, as in `Maria will finish the billing dashboard *4`. The count then replaces counting it in the reference.

`expected.txt` lists what a good summary must contain:

```
# one item per line
action: omar, pricing, deck|slides, friday
decision: ship, beta, monday
```

A comma separates concepts, and every concept must appear in the same bullet. A `|` inside a concept separates alternatives, and any one is enough. An alternative may be several words, which must appear next to each other in order. A bad line stops the run and names the file and the line number.

`language` pins the transcription language. Without it the recording keeps the language preference it was imported with.

`dory-cli eval` takes one case folder, or a folder of case folders. Each subfolder with a `reference.txt`, `terms.txt`, or `expected.txt` is a case, run in name order.

## Running it

Eval writes recordings into the library, so it always needs `DORY_DATA_DIR` set to a folder of its own. The folder also holds `models/`, and a new folder downloads every model again. Keep one eval folder and reuse it, or link `models` to another folder's models.

```
export DORY_DATA_DIR="$HOME/Library/Application Support/dory-dev/eval-data"
mkdir -p "$DORY_DATA_DIR"
ln -s "$HOME/Library/Application Support/dory-dev/live-data/models" "$DORY_DATA_DIR/models"
dory-cli eval ~/dory-cases --label baseline --tsv ~/dory-cases.tsv
```

Each case is imported once as a recording titled `eval: <case name>`. A rerun finds it and transcribes it again, so the folder holds one recording per case. Delete the recording after you replace a case's audio, or the old audio is reused.

The options:

- `--stt MODEL_ID` transcribes with that model for this run. Without it the Settings choice runs.
- `--llm MODEL_ID` summarizes with that model. The transcribe run still writes a summary with the Settings model first, so the Settings model must be able to run. The picked model then writes a second summary over it, and that one is scored. `llm_secs` is the second run.
- `--summary-only` summarizes the stored transcript again and skips transcription. The case must already have run once. Use it to compare summary prompts or models without paying for transcription.
- `--label TEXT` names the run in the TSV.
- `--tsv FILE` appends one row per case. The header is written when the file is new or empty.
- `--fake-engines` runs the fake engines, for checking the harness itself. `scripts/eval-smoke.sh` does this with one case made by `say`.

A case that blocks or fails prints its error and the run goes on. The command exits with an error at the end if any case failed.

## The planning meeting case

`scripts/fixtures/two-hour-planning.sh [CASES_DIR] [LIBRARY]` builds a case from the test library's "Two hour planning", a 132-minute meeting with 40 action items, each said four times. It copies the gold files from `scripts/fixtures/two-hour-planning/` and links the library's take as the audio, reading the library only. The case has no reference. Its terms are the 160 exact item sentences, and its expected items need the owner and the item in one bullet.

## Building a case from a real recording

1. Run `dory-cli show ID` on a recording to get a draft transcript.
2. Copy the spoken text into `reference.txt`. Fix every word by hand while you listen. Drop speaker names and timestamps.
3. List the names and jargon the speech model tends to miss in `terms.txt`.
4. Read the summary and write the action items and decisions a good summary must hold in `expected.txt`. Pick the few words that make each item unmistakable, not its full wording.
5. Copy the audio into the folder. An imported recording's audio is `takes/<ID>.flac` in its data folder. A recording made with Dory can hold its mic and system audio as separate channels, and eval imports one mixed track.

Keep cases outside the repo. They hold private audio.

## Comparing runs

Give each run a `--label` and append every run to the same TSV. Each row has the label, the case, the models, the counts, and the times, so two runs of one case sit side by side in a spreadsheet. Change one thing between runs. Rerun the baseline when the audio or the reference changes, since the numbers only compare across runs of the same case files.

## Limits

- Imported audio is one mixed channel. The mic and system split, and bleed removal, are not exercised.
- Number words and digits count as errors. "Twenty" against "20" is a substitution.
- Summary matching is keyword based. A good bullet that uses none of your alternatives counts as a miss, and a bullet that contains the words by accident counts as a hit. Write alternatives for the wordings you expect.
- Eval always runs at the Fast pace, so times compare across runs. Heat can still slow a long run, so compare times from runs made in similar conditions.
- The first summary of a run probably includes loading the model into Ollama. On a 27-second case it took 15.0 s, and a second summary right after took 1.4 s. Drop the first case's `llm_secs`, or warm the model, before comparing summary times.
- A job that finishes between two polls of 25 ms reports no stage time. The fake engines do. Real engines do not.
