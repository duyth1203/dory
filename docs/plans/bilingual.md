# Bilingual meetings plan

Dory's operator holds meetings in Tagalog and English, with whole turns in each and Taglish sentences that switch inside the sentence. Whisper large-v3 turbo transcribes them, but no engine reports a language, so Dory stores none, the byline names none, and the summary prompt falls back to "the language the meeting was held in". With no Tagalog in the meeting languages, Auto picks Parakeet, which hears nothing in a Tagalog meeting and saves an empty transcript without a word of warning.
A spike on spliced turn-switch audio and on Taglish clips found a worse failure. When a 30-second Whisper window holds a Tagalog turn and then an English turn, Whisper keeps one language and silently drops the other. Asking Whisper for sentence times shows where its text stops, and decoding the uncovered speech again brings most of the dropped turn back without hurting Taglish.
After this program, Dory records the language it hears in every window, recovers dropped turns in two-language meetings when that passes on public test audio, labels the turns of the second language, and lets VoiceOver say so. The summary is written in the language heard most, and a Translate Summary action turns any summary into another language in seconds, beside the original.
The rule the program enforces is that `dory-core` decides every language rule as pure functions over the meeting languages the user set, and a speech engine only reports what it heard. No rule branches on a model or a language name.
PRs B1 to B5 land in that order as one linear stack. B1 to B3 do not depend on the in-flight output-language or search branches. B4 and B5 need output-language on trunk. The operator keeps meeting recordings private, so every measurement uses public audio.

## How to read this

One box is one unit of work. Every box names the evidence that checks it. A nested box is a sub-step of the box above it. Check a box only when its evidence exists, a file, a log line, a screenshot, a test run, or a SHA. The body is a how-to. The appendices explain and record.

The program runs `pstack/skills/poteto-mode/playbooks/autopilot-stack.md`. The root agent builds one linear stack from B1 to B5, and the operator reviews and lands it. B3, B4, and B5 change an interaction and stop at merge-ready for the operator's review. B3 turns recovery on for two-language meetings only if it passes the public-audio rule in Appendix F, adds a Settings switch, and stops at merge-ready for the operator's review. B5 adds a Translate Summary action and stops there too.

Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

Requirement ids such as `UX-26` and `A11Y-10` point into `docs/plans/ux-redesign.md`. Appendix A holds the spike numbers, Appendix B the designs that lost, and Appendix G the open questions with the default each PR builds if the operator does not answer.

A lane marked **Operator lane** needs a person at the Mac, because it runs with VoiceOver on or plays audio through the speakers. The root prepares the exact steps and the evidence to capture, and the operator runs it. Every other lane is agent-runnable.

pstack is a Claude Code plugin, not part of this repo. Paths that start with `pstack/` mean `~/.claude/plugins/cache/cursor-plugins/pstack/<version>/`. Validate this file with `node <pstack>/skills/poteto-mode/scripts/check-plan.mjs docs/plans/bilingual.md`.

## Program checklist

### Arm the program

- [ ] State the protocol and this plan to the operator, then stop. Start execution only on the operator's explicit go.
- [ ] Get the operator's answers to the open questions in Appendix G, or take the defaults written there. Log each answer as a row in `docs/plans/decisions.tsv`.
- [ ] Read these from trunk at program start. Re-read them at every tick.
  - [ ] `git show origin/main:docs/plans/bilingual.md`
  - [ ] `git show origin/main:docs/plans/speech-model-evaluation.md`
  - [ ] `git show origin/main:docs/plans/ux-redesign.md`
  - [ ] `git show origin/main:docs/architecture/hexagonal.md`
  - [ ] `git show origin/main:AGENTS.md`
  - [ ] `docs/plans/output-language.md` from `feat/output-language`, and `docs/plans/search.md` from `docs/search-plan`, until each lands on trunk.
  - [ ] The pstack copies of `poteto-mode/playbooks/autopilot-stack.md`, `poteto-mode/playbooks/opening-a-pr.md`, `swarm/SKILL.md`, and `tdd/SKILL.md`. pstack lives outside the repo, so `git show origin/main:` applies only to the repo files above.
- [ ] Tag B1's first commit as `baseline-bilingual` once it exists. That commit adds only the bench and the `language` log lines (see B1 **Build**), so it transcribes like `main`. Every "trunk" measurement in this plan runs on that tag.
- [ ] On the operator's go, arm the audit tick as `/loop 1h` with the tick prompt below. Never leave the cadence to memory.
- [ ] Use this tick prompt, verbatim. "Re-read the execution playbook from trunk. Audit the operation against it and fix drift in this tick. Probe every active lane and judge progress by side effects only. Stand down a stuck lane and dispatch its replacement now. Then post a short status message to the operator in chat only when the audit found a tracked change that no earlier status message reported, such as a PR opened, a code-ready head, a round launched or closed, a verdict, a merge, a stuck agent and the action taken, a blocker added or cleared, or a decision only the operator can make. Name every such change and nothing else. Do not repeat a table, the merged list, or an unchanged blocker. If the audit found none, end the turn with no reply text. Either way, log this tick's row in your decision trail. The row names the items reported, or none."
- [ ] On the operator's hold or stand-down, send every owner a zero-writes order at once.

### Spawn owners

- [ ] Spawn one owner per PR with the full lifecycle the execution playbook names. Owners use `pstack:poteto-agent` on the `feature, refactoring` model from `~/.claude/rules/pstack-models.md`. B3 uses the `hardest tasks` model, because it changes the speech port and the transcribe stage together.
- [ ] Follow this dependency graph. Each owner branches from its parent's branch, because the stack is linear.
  - [ ] B1 is first. It branches from `main`.
  - [ ] B2 after B1. B2 uses B1's bench to check that the language it reports matches the reference.
  - [ ] B3 after B2. B3 re-decodes what B2's language and Whisper's sentence times show was dropped. B3 ships recovery on for two-language meetings only if it passes the rule in Appendix F on the public sets.
  - [ ] B4 after B3 and after `feat/output-language` lands on trunk. B4 fills and shows output-language's `Segment.language`. If output-language has not landed when B3 merges, B4 waits. It never adds its own copy of the field.
  - [ ] B5 after B4. It also needs output-language's translator and its Original / Translated switch on trunk.
- [ ] Hold the file boundaries. Each PR's **Files** list is its boundary. A PR that needs a file outside its list stops and asks the root, which either adds a small PR below it in the stack or amends this plan with a logged decision.
- [ ] Hold the review gate. B3 adds a Settings switch, B4 labels turns, and B5 adds Translate Summary. Each waits for the operator's review in chat with screenshots and a video before merge.

### PR mechanics, for every PR

- [ ] Resolve the forge once. Default to `gh`; if `command -v origin` succeeds and Origin can resolve the repository, use `origin pr` for every PR operation. Record any fallback to `gh`. Never require `gt`.
- [ ] Open the PR ready, never draft, per **Opening a PR**. Use the run's built-in PR tool when it has one, else `origin pr create --status open --base <base-branch>` or `gh pr create --base <base-branch>` according to the resolved forge. A stack child targets its parent branch.
- [ ] Run `cargo fmt --check`, `cargo clippy --workspace --all-features --all-targets -- -D warnings`, and `cargo test --workspace --all-features` once before the PR-facing push. Push with hooks on.
- [ ] Run `/deslop` before each commit and `/no-comments` before review. Both come from `cursor-team-kit`. If the plugin is missing, write `Skipped in Claude Code` with the reason in the PR body.
- [ ] Triage every Bugbot and security-reviewer comment per `pstack/skills/poteto-mode/references/bugbot-triage.md`.
- [ ] Rebase onto current trunk before the code-ready report and babysit. Keep that merge base in fix rounds. Rebase again only at merge prep, on a `git merge-tree` conflict with trunk, or on a CI failure that comes from a change on trunk.
- [ ] Name branches `<type>/<slug>` per `AGENTS.md`. B1 uses `test/bilingual-bench`, B2 uses `fix/whisper-reports-language`, B3 uses `feat/recover-dropped-turns`, B4 uses `feat/turn-languages`, and B5 uses `feat/translate-summary`. No AI trailer in commit messages.
- [ ] Append a row to `docs/plans/decisions.tsv` for every real decision with the pstack `show-me-your-work/scripts/log.sh` helper. Mark any external fact you did not re-check this session as unverified in the row.
- [ ] Never use the operator's own meetings or the real data folder in any lane, bench, or log. The operator declined to share recordings, for privacy. Every speech measurement runs on the public sets in Appendix F.

### Verdict and merge, for every PR

- [ ] A full swarm runs once, at the code-ready head. Another full swarm runs only for new behavior, a redesign, or a large new diff. A rebase, a merge-conflict resolution, or a minor fix gets a single verifier at the new head. Run each full swarm per `pstack/skills/swarm/SKILL.md`. One gates lane. The ten live lanes from the PR's **Verify, live** block. The perf lane from its **Verify, perf** block. Two or more audit lanes, each with its own focus, that read the diff and the receipts and distrust the PR body. The root audits the receipts in the merge-ready report before the verdict.
- [ ] Clean only when every lane is `PASS`. An Operator lane that has not run is `NOT VERIFIED`, never `PASS`. Findings go back to the owner, including a defect that a lane filed as a note. A new head gets a fresh verdict, except for results that stay valid under the patch-id rule in `playbooks/shipping.md`.
- [ ] The root appends each clean PR to the stack with its base set to its parent's branch. The operator lands the stack bottom-up. A rebase that leaves `git patch-id` unchanged keeps the verdict. A changed patch-id needs a single verifier at the new head.

### Boot recipe, for every live lane

Live lanes run on the operator's Mac, an Apple Silicon machine on macOS 26 or later. Speech lanes are CPU-heavy, so at most two transcribe at once, and speed lanes run alone. No lane uses the real data folder.

- [ ] `git fetch origin <head-branch> && git checkout <head SHA>`. Build into a lane-owned target folder with `CARGO_TARGET_DIR=/tmp/dory-target-<pr-id>`, so lanes do not race other agents in the shared `target/`. Check `df -h /` first and keep at least 10 GB free.
- [ ] Make a throwaway library. Copy `~/Library/Application Support/dory-dev/live-data` with `cp -cR` to `/tmp/dory-bilingual-<n>`, remove `.lock`, and set `DORY_DATA_DIR=/tmp/dory-bilingual-<n>`. It holds Whisper turbo and Parakeet under `models/sherpa/`, the English "Weekly sync", and "Pulong sa Tagalog". Never write to the source folder.
- [ ] Import the bilingual clips. Run `scripts/bilingual-fixtures.sh /tmp/dory-bilingual-<n>`, which B1 adds. It builds the turn-switch recordings from the library's own Tagalog and English takes, downloads the 24 CS-FLEURS `tgl_eng` clips listed in `docs/plans/bilingual-spike/clips.tsv` into `fixtures/public/cs-fleurs/`, checks each SHA-256, and adds each as a recording with `dory-cli add <wav> --title <name>`. CS-FLEURS is CC BY-NC 4.0, so the clips stay out of git and out of the app bundle.
- [ ] Set the meeting languages with `dory-cli languages tl en`, then run `dory-cli process <id>`. Read the result with `dory-cli show <id> --languages`, which B2 adds.
- [ ] Start the app with `DORY_DATA_DIR=/tmp/dory-bilingual-<n> cargo run --release -p dory-gpui` and wait for the `first-frame` log line. Deliver input only through the Claude Code computer-use tools (`app_type`, `app_key`, `app_click`, `app_ax_find`, `app_screenshot`), because no `control-ui` skill drives a native macOS window. Capture Dory's own window with `screencapture -l <window id>`, never the full screen.
- [ ] Read-only diagnostics are `log stream --predicate 'subsystem == "com.dory.app"'`, `sqlite3 -readonly /tmp/dory-bilingual-<n>/dory.sqlite`, and the `window ms=`, `language heard=`, and `language chose=` log lines.
- [ ] Save every screenshot to `/tmp/swarm-<pr-id>/worker-<n>/<slug>.png` and return the paths with the report.

## Build the bilingual bench and its fixtures (B1)

**Depends on.** None.

**Files.**

- [ ] Create `crates/dory-engines/examples/bilingual-bench.rs`.
- [ ] Create `scripts/bilingual-fixtures.sh` and `scripts/bilingual_turns.py`, the spike's `make_turns.py` moved out of `docs/plans/bilingual-spike/`.
- [ ] Edit `crates/dory-core/src/app/stages/mod.rs` only to add the `language heard=` and `window ms=` log lines.
- [ ] Edit `docs/plans/speech-model-evaluation.md` to record that the operator's own recordings will not be used, for privacy, and that the public sets in Appendix F replace its code-switching step.

**Build.**

- [ ] Make the first commit instrumentation only. Log `window ms=<n> lang=<pref>` per window and `language heard=<tag|none> window=<start>-<end>` per checkpoint in `transcribe_window`. The root tags this commit `baseline-bilingual`.
- [ ] Name the data shapes first, inside the example. A `Clip` is one WAV with its reference text from `refs.tsv` and, for turn-switch clips, its parts from `langs.tsv`, each `{ language: LangTag, range: TimeRange }`. A `Mode` is one recognition rule under test. The modes are `Auto`, `Pinned(LangTag)`, and `Chunks(Ms)`, the spike's fixed cuts at the quietest point. B3 adds `Covered`. A `Score` is `{ ref_words, errors, wrong_language, english_only, dropped_turns, extra_words_on_silence, audio_ms, busy_ms }`.
- [ ] `bilingual-bench score <folders...> [--modes auto,tl,en,chunks]` runs each mode on every clip and prints one table row per mode, as Appendix A's table does.
  - [ ] Word errors after lowercasing and stripping punctuation and hyphens on both sides, as `wer-bench` does.
  - [ ] The wrong-language rate is the share of windows or chunks whose heard language is not the language holding most of their time, from `langs.tsv`. Clips without parts count only languages outside the meeting languages as wrong.
  - [ ] English-only is the share of clips whose reference has at least 20% Tagalog words but whose output has under half that share. A word is Tagalog when it is in the spike's function-word list, or not in `/usr/share/dict/words`. A dropped turn is a turn-switch part whose output keeps under half of its reference words. Extra words on silence counts output words with no reference words in the same part, for the single-language clips.
  - [ ] Speed is audio seconds per busy second, per mode.
- [ ] `scripts/bilingual-fixtures.sh <data dir>` downloads the 24 CS-FLEURS clips in `docs/plans/bilingual-spike/clips.tsv` into `fixtures/public/cs-fleurs/`, checks their SHA-256, converts them with `afconvert -f WAVE -d LEI16@16000 -c 1`, writes `refs.tsv`, converts the library's "Pulong sa Tagalog" and "Weekly sync" takes with `afconvert`, builds the eleven turn-switch clips and their reference text with `scripts/bilingual_turns.py`, and adds each clip as a recording with `dory-cli add`.

**You see.**

- [ ] `bilingual-bench score fixtures/public/cs-fleurs --modes auto,tl,en` prints the same word-error rates as Appendix A, within 2 points.
- [ ] `dory-cli process` on "Pulong sa Tagalog" logs one `language heard=none` line per window at `baseline-bilingual`, which is the bug B2 fixes.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] The example's own tests cover parsing `refs.tsv` and `langs.tsv`, word normalization with hyphens, and each rate on a hand-built three-clip case. Run `cargo test -p dory-engines --example bilingual-bench`.
- [ ] `scripts/bilingual-fixtures.sh` has a dry-run flag that prints what it would download and add. Run `scripts/bilingual-fixtures.sh --dry-run /tmp/dory-bilingual-0` and compare with `docs/plans/bilingual-spike/clips.tsv`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Run `dory-cli process` on "Pulong sa Tagalog" at `main` and at head. Save `regress-transcript.png` of both `dory-cli show` outputs. Pass when both transcripts are byte-identical, because B1 changes only logs and dev tools.
- [ ] Lane 2. Run `scripts/bilingual-fixtures.sh` on a fresh library copy. Save `fixtures.png` of `dory-cli list`. Pass when 24 CS-FLEURS recordings and 11 turn-switch recordings are listed, and every SHA-256 matched.
- [ ] Lane 3. Run the script a second time on the same folder. Save `fixtures-again.png`. Pass when it downloads nothing, adds no duplicate recordings, and exits 0.
- [ ] Lane 4. Corrupt one downloaded clip with `printf x >> <file>` and run the script. Save `sha-fail.png`. Pass when the script names that clip, deletes it, and exits non-zero before adding anything.
- [ ] Lane 5. Run `bilingual-bench score fixtures/public/cs-fleurs --modes auto,tl,en`. Save `score-csfleurs.png`. Pass when each mode's word-error rate is within 2 points of Appendix A.
- [ ] Lane 6. Run `bilingual-bench score` on the turn-switch folder. Save `score-turns.png`. Pass when pinned `en` shows Tagalog-to-English loss on the pure Tagalog clips, as Appendix A recorded.
- [ ] Lane 7. Run `bilingual-bench score` on the turn-switch folder with `--modes auto`. Save `dropped-turns.png`. Pass when it reports dropped turns in `mix_tl3_en0` and `mix_tl1_en3`, as Appendix A recorded.
- [ ] Lane 8. Run `bilingual-bench score` on the six single-language turn clips with `--modes chunks`. Save `silence-words.png`. Pass when extra words on silence counts the "you" the spike saw on `pure_tl2`.
- [ ] Lane 9. Run `git status --porcelain fixtures/` after lane 2. Save `fixtures-ignored.png`. Pass when it prints nothing, because `/fixtures/` is in `.gitignore` and no CS-FLEURS audio can be committed.
- [ ] Lane 10. Run `dory-cli process` on "Pulong sa Tagalog" at head with `log stream` open. Save `heard-none.png`. Pass when every window logs `language heard=none`, which records the trunk bug in the log.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Wall time of `dory-cli process` on "Pulong sa Tagalog" and on "Two hour planning". The bench is a dev tool, so its own speed is reported, not gated.
- [ ] Probe. Run each recording three times at trunk and three times at head, alternating, on AC power with no other agent building.
- [ ] Baseline. Record the trunk medians first.
- [ ] Rule. Fail when a head median exceeds the trunk median by more than 2%, because B1 adds only two log lines per window.

**Review gate.** None. B1 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends B1 to the stack on `main`.

## Record the language Whisper hears in each window (B2)

**Depends on.** B1.

**Files.**

- [ ] Create `vendor/sherpa-onnx/`, a copy of the `sherpa-onnx` 2.0.1 crate whose `OfflineRecognizerResult` gains `#[serde(default)] pub lang: String`. Add `[patch.crates-io] sherpa-onnx = { path = "vendor/sherpa-onnx" }` to the root `Cargo.toml`.
- [ ] Edit `crates/dory-engines/src/sherpa/engine.rs`, `crates/dory-engines/src/sherpa/tests.rs`, and `crates/dory-engines/tests/sherpa_live.rs`.
- [ ] Edit `crates/dory-core/src/app/stages/mod.rs` and `crates/dory-core/src/models/fit.rs` for `Mismatch::NothingRecognized`.
- [ ] Edit `crates/dory-store/src/sqlite/codec.rs` to store the new mismatch, and `apps/dory-gpui/src/models_view.rs` for its text.
- [ ] Edit `apps/dory-cli/src/args.rs` and `apps/dory-cli/src/view.rs` for `show --languages`.
- [ ] Edit `crates/dory-core/tests/pipeline.rs`.

**Build.**

- [ ] Open the upstream pull request to k2-fsa/sherpa-onnx that adds `lang` to the Rust `OfflineRecognizerResult`, and link it in `vendor/sherpa-onnx/PATCH.md`. Delete `vendor/sherpa-onnx` and the patch line in the PR that bumps to the first release with the field. The vendored copy differs from 2.0.1 by that one field, and `PATCH.md` holds the diff.
- [ ] In `SherpaEngine::transcribe`, read `result.lang` for a Whisper layout and return it as `TranscribeOutcome::detected_language`, parsed with `LangTag::parse`. An empty or unparseable `lang` is `None`. Parakeet reports nothing, as today, because its result has no language.
- [ ] Rank detected languages by speech time, not by segment count. `rank_languages` takes `(LangTag, Ms)` pairs from each checkpoint's span and sorts by total time, ties by first appearance. Whisper gives one segment per window, so counting segments weighted a 3-second window like a 30-second one.
- [ ] Keep a heard language that is not a meeting language. It is what the engine reported, and B3 decides what to do with it. Log `language heard=<tag> window=<start>-<end>`.
- [ ] Stop saving an empty transcript for speech the model could not hear. When a transcribe run sent windows holding at least `HEARD_NOTHING_SPEECH` of VAD speech, 10 seconds, and every window came back without a word, return `Blocker::Incompatible { model, why: Mismatch::NothingRecognized }` instead of `TranscriptReplaced`. The rule is one pure function, `stages::heard_nothing(speech: Ms, words: usize) -> bool`. The blocker's text is "Heard speech but recognized no words. Add the meeting's languages in Settings, then transcribe again.", and its fix opens **Settings > Transcription > Meeting languages**. The output-language agent saw this live. With meeting languages left at any, Auto picked Parakeet for "Pulong sa Tagalog" and stored 0 segments, and with `tl` and `en` set it picked Whisper turbo. Appendix G question 9 asks whether to switch models by itself instead.
- [ ] `dory-cli show <id> --languages` prints the detected languages, most heard first.

**You see.**

- [ ] `dory-cli show <id> --languages` on "Pulong sa Tagalog" prints `tl`, where trunk prints nothing.
- [ ] The detail pane byline for "Pulong sa Tagalog" names Tagalog next to the duration, as UX-26 asks. Trunk names no language for any Whisper recording.
- [ ] The summary prompt for "Pulong sa Tagalog" names `tl`, where trunk says "the language the meeting was held in". `summary.rs` already builds that sentence from `main_language()`, so B2 changes no summary code.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-engines/src/sherpa/tests.rs` gains `whisper_result_lang_becomes_the_detected_language` and `an_empty_lang_is_none`, on a parsed JSON result. Run `cargo test -p dory-engines sherpa`.
- [ ] `crates/dory-core/src/app/stages/mod.rs` tests `rank_languages` by time. A 30-second `tl` window outranks three 3-second `en` windows. Run `cargo test -p dory-core stages`.
- [ ] `crates/dory-core/tests/pipeline.rs` gains `auto_transcript_records_the_heard_languages`, with a fake engine that reports `tl`, `en`, `tl` for three windows, and `speech_with_no_words_blocks_instead_of_saving_empty`, with a fake engine that returns nothing for 30 seconds of speech. `heard_nothing` gets a table test at 9 and 10 seconds of speech and at 0 and 1 words. Run `cargo test -p dory-core --test pipeline`.
- [ ] `crates/dory-engines/tests/sherpa_live.rs` gains an ignored case that transcribes a Whisper `test_wavs` clip and asserts a non-empty language. Run `DORY_SHERPA_MODELS=<models dir> DORY_SHERPA_WAV=<wav> cargo test -p dory-engines --test sherpa_live -- --ignored`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Process "Pulong sa Tagalog" with Language on Automatic at `baseline-bilingual` and at head. Save `regress-languages.png` of both `sqlite3 -readonly ... "select languages from transcripts"` outputs. Pass when trunk stores an empty list, head stores `tl`, and both transcripts have the same text.
- [ ] Lane 2. Open "Pulong sa Tagalog" in the app at head. Save `byline-tagalog.png`. Pass when the byline reads "Tagalog" next to the duration.
- [ ] Lane 3. Process "Weekly sync" with Whisper chosen in Settings. Save `byline-english.png`. Pass when the byline reads "English".
- [ ] Lane 4. Process "Weekly sync" with Parakeet. Save `parakeet-none.png`. Pass when the byline names no language and the transcript matches trunk, because Parakeet reports none.
- [ ] Lane 5. Process the turn-switch recording `mix_tl1_en3`. Save `mixed-languages.png` of `dory-cli show --languages`. Pass when it lists the language Whisper heard for the window, as Appendix A recorded for that clip.
- [ ] Lane 6. Clear the meeting languages with `dory-cli languages --clear`, choose Auto, and process "Pulong sa Tagalog" at trunk and at head. Save `heard-nothing.png` of both runs and of the app's needs-attention row at head. Pass when trunk stores a transcript with 0 segments, and head stores none and shows the blocker text with its Settings fix. Then run `dory-cli languages tl en`, process again, and pass when Whisper transcribes it.
- [ ] Lane 7. Re-summarize "Pulong sa Tagalog" with Gemma at trunk and at head. Save `summary-language.png` of both summaries. Pass when the head summary, headings included, is in Tagalog, and record which language trunk's fallback prompt produced.
- [ ] Lane 8. Process "Two hour planning". Save `long-shares.png`. Pass when the process exits 0 and the `language heard=` lines, weighted by window length, rank the languages in the stored order.
- [ ] Lane 9. Quit the app in the middle of transcribing "Two hour planning", relaunch, and let it finish. Save `resume.png`. Pass when the stored languages equal those of an uninterrupted run, because checkpoints carry the heard language.
- [ ] Lane 10. Operator lane. With VoiceOver on, open "Pulong sa Tagalog". Save `vo-byline.png`. Pass when VoiceOver reads the byline with "Tagalog" in it.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Wall time of `dory-cli process` on "Pulong sa Tagalog" and on "Two hour planning" with Whisper.
- [ ] Probe. Three runs each at `baseline-bilingual` and at head, alternating, on AC power with no other agent building.
- [ ] Baseline. Record the trunk medians first.
- [ ] Rule. Fail when a head median exceeds the trunk median by more than 2%. Reading `lang` from the JSON Dory already parses costs nothing measurable.

**Review gate.** None. B2 is not review-gated. It fills a byline that UX-26 already specifies and changes no control.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends B2 to the stack on B1's branch.

## Recover the turns Whisper drops when a window switches language (B3)

**Depends on.** B2.

**Files.**

- [ ] Edit `crates/dory-core/src/ports/speech.rs` for `TranscribeRequest::sentence_times`, and `crates/dory-core/src/models/fit.rs` for `SttCaps::sentence_times`.
- [ ] Edit `crates/dory-core/src/app/stages/windows.rs` and `crates/dory-core/src/app/stages/mod.rs`.
- [ ] Edit `crates/dory-core/src/domain/settings.rs` for the `switch_recovery` setting, and `crates/dory-store/src/sqlite/mod.rs` to store it as a settings key.
- [ ] Edit `crates/dory-engines/src/sherpa/engine.rs`, `crates/dory-engines/src/sherpa/catalog.rs`, `crates/dory-engines/src/sherpa/mod.rs`, and `crates/dory-engines/src/sherpa/tests.rs`.
- [ ] Edit `crates/dory-engines/Cargo.toml` to take `dory-store` as a dev-dependency, so the bench reads the same VAD spans the app does through the takes adapter. Make `windows::uncovered` and `stages::settle_language` public, because they are pure and the bench must apply them exactly as the stage does.
- [ ] Edit `crates/dory-engines/examples/bilingual-bench.rs` to add the `covered` mode, which applies the same rule, and keep its `chunks` mode for comparison.
- [ ] Edit `apps/dory-cli/src/args.rs` and `apps/dory-cli/src/settings.rs` for `dory-cli switch-recovery on|off|auto`.
- [ ] Edit `apps/dory-gpui/src/settings_window.rs` and `apps/dory-gpui/tests/settings.rs` for the toggle, and `docs/plans/ux-redesign.md` UX-26 to describe it.
- [ ] Edit `crates/dory-core/tests/pipeline.rs`.

**Build.**

- [ ] Name the data shapes first. `SttCaps` gains `sentence_times: bool`, true for Whisper's catalog row. `TranscribeRequest` gains `sentence_times: bool`, which asks the engine for one `RawSegment` per sentence with its own range. An engine without the capability ignores it. `Recovery` is `Off`, `On`, or `Auto`, stored as `Settings::switch_recovery`.
- [ ] In the sherpa engine, `Recipe` gains `sentence_times`, which sets `enable_segment_timestamps` and rebuilds the recognizer when it changes. With it set, emit one `RawSegment` per entry of `segment_texts`, timed by `segment_timestamps` and `segment_durations`, which the vendored `OfflineRecognizerResult` from B2 also gains. Token timestamps stay off. The turbo export has no cross-attention outputs, and sherpa-onnx warns that token timestamps are unavailable.
- [ ] Write the rule as pure functions.
  - [ ] `windows::recovers(caps: &SttCaps, recording: &LanguagePref, meeting: &[LangTag], switch: Recovery) -> bool`. `Auto` is true only when the recording's language is Automatic, the meeting languages hold two or more, and the engine has `sentence_times`. No branch names a model or a language.
  - [ ] `windows::uncovered(speech: &[TimeRange], said: &[TimeRange], min: Ms) -> Vec<TimeRange>` returns the stretches of VAD speech inside a window that no returned sentence overlaps, merged across pauses, each holding at least `min` of speech. `min` is 1.5 seconds.
  - [ ] `stages::settle_language(heard: Option<&LangTag>, meeting: &[LangTag]) -> Settle` returns `Keep` or `Redo(LangTag)`. A heard language outside a non-empty meeting list is redone pinned to the first meeting language.
- [ ] When `recovers` is true, the transcribe stage sends each window with `sentence_times`, runs `uncovered` over the window's VAD spans and the sentences it got back, and sends each uncovered stretch as its own Auto window. A stretch's sentences take the language heard for that stretch. A window or stretch heard outside the meeting languages is redone per `settle_language`. Everything a window produced is checkpointed together, so a resume never repeats a re-decode.
- [ ] A recovered stretch can drop the turn after its own switch, so `windows::recover(speech, said, decode)` runs in rounds. Each round runs `uncovered` over each stretch's own VAD speech and the sentences it got back, and sends what is still uncovered in the next round, for at most `RECOVERY_DEPTH` (3) rounds. A stretch whose call covers none of its speech is not sent again, because the same audio decodes the same way. The stage and the bench both call it.
- [ ] `Recovery` defaults to `Auto` when lanes 2, 3, and 4 meet the rule in Appendix F at the PR head. If any of them misses it, B3 merges with `Off` as the default and a decision row names the lane and the number. Sentence times stay off whenever recovery is off, because the spike's first pass with timestamps lost more on switching windows than the pass without them.
- [ ] Settings > Transcription gains a switch, **Recover speech lost when the language changes**, under Meeting languages. On sets `Auto` and off sets `Off`. Its one-line description reads "For meetings in more than one language. Transcribing takes a little longer." It is dimmed, with that reason, when fewer than two meeting languages are set.
- [ ] Drop Whisper's catalog window from 30 to 28 seconds. sherpa-onnx pads the tail, and the spike's 29.9-second window logged "Only waves less than 30 seconds are supported" and lost its last word. Today's 30-second windows can lose the end of every full window.
- [ ] Log `language chose=<tag> heard=<tag> window=<start>-<end> recovered=<n>` per window.

**You see.**

- [ ] With meeting languages `tl` and `en` and recovery on, `mix_tl3_en0` keeps its English turn after its Tagalog sentence, where trunk drops it.
- [ ] Segments in those meetings carry Whisper's sentence times, so a language change inside a window becomes a segment boundary that B4 can split a turn at.
- [ ] A recording pinned to Tagalog, and the English "Weekly sync" with English as its only meeting language, transcribe exactly as trunk does.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/src/app/stages/windows.rs` tests `recovers` over every combination of switch, recording language, meeting-language count, and capability. It tests `uncovered` with a sentence covering everything, a dropped tail, a dropped head, a gap shorter than `min`, and silence between sentences that the VAD did not mark as speech. It tests `recover` with a fake engine that drops the tail of every call, which recovers a four-turn window within the depth, a six-turn window that stops at the depth, a stretch that comes back empty, and a failed call. Run `cargo test -p dory-core windows`.
- [ ] `crates/dory-core/src/app/stages/mod.rs` tests `settle_language` with a heard language inside, outside, and missing from the meeting languages, and with no meeting languages. Run `cargo test -p dory-core stages`.
- [ ] `crates/dory-core/tests/pipeline.rs` gains `a_dropped_tail_is_decoded_again`, `an_off_list_language_is_redone_pinned`, `recovery_off_sends_no_sentence_times`, `resume_does_not_repeat_a_recovered_stretch`, and `a_recovered_stretch_that_drops_its_own_tail_is_decoded_again`, with a fake engine that drops the second half of a window. Run `cargo test -p dory-core --test pipeline`.
- [ ] `crates/dory-engines/src/sherpa/tests.rs` parses a recorded JSON result with `segment_texts` into timed segments, and asserts Whisper's window is 28 seconds. Run `cargo test -p dory-engines sherpa`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Process `mix_tl3_en0` and `mix_tl1_en3` with meeting languages `tl` and `en` at `baseline-bilingual` and at head with recovery on. Save `regress-dropped-turn.png` of both `dory-cli show` outputs. Pass when trunk drops one language's turn in each, as Appendix A recorded, and head keeps the English turn of both.
- [ ] Lane 2. Process all eleven turn-switch recordings at head with recovery on. Save `turns-score.png` of `bilingual-bench score --modes auto,covered,chunks`. Pass when Appendix F rule 1 holds.
- [ ] Lane 3. Process the 24 CS-FLEURS Taglish recordings at head with recovery on. Save `taglish-score.png`. Pass when Appendix F rule 2 holds.
- [ ] Lane 4. Process the six single-language turn-switch recordings with recovery on. Save `mono-junk.png`. Pass when Appendix F rule 3 holds, which proves the VAD gate keeps silent tails out where the spike's ungated gaps added "you", "Thank you.", and "Okay.".
- [ ] Lane 5. Process "Pulong sa Tagalog" with Language pinned to Tagalog, and "Weekly sync" with English as the only meeting language. Save `unchanged.png`. Pass when both transcripts and their logged windows equal trunk's.
- [ ] Lane 6. Process `mix_en1_tl3` with recovery on and print its segments with `sqlite3 -readonly`. Save `sentence-times.png`. Pass when it has more than one segment, every range lies inside the window, the ranges do not overlap, and the log shows `en` heard for the window and `tl` for its recovered stretch, as the spike saw.
- [ ] Lane 7. Process a 29-second single-language clip at trunk and head. Save `window-28.png`. Pass when trunk logs the sherpa "Only waves less than 30 seconds" line and head does not, and head keeps the last word.
- [ ] Lane 8. Operator lane. Without headphones, play `mix_tl1_en3` through the speakers during a 60-second Dory recording with mic and system audio, then process it with recovery on. Save `mic-system.png`. Pass when no turn appears twice from mic bleed.
- [ ] Lane 9. Quit `dory-cli process` on "Two hour planning" halfway with recovery on and run it again. Save `resume-recovery.png`. Pass when the second run skips every checkpointed window and its recovered stretches, and the result equals an uninterrupted run.
- [ ] Lane 10. In Settings > Transcription, turn the recovery switch off, process `mix_tl3_en0`, then turn it on and process it again. Save `switch-off.png` and `switch-on.png`, each with the transcript. Pass when off drops the English turn as trunk does, on keeps it, and the switch is dimmed with its reason after `dory-cli languages en`.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Speed as audio seconds per busy second, the share of windows that needed a re-decode, and the wall time of `dory-cli process` on "Two hour planning" and on a 60-minute recording built by looping the eleven turn-switch clips, with recovery off at trunk and on at head.
- [ ] Probe. Three runs each at trunk and head, alternating, on AC power with no other agent building and nothing else transcribing.
- [ ] Baseline. Record the trunk median first. The spike re-decoded 4 of 24 Taglish windows and 8 of 11 turn-switch windows, each re-decode one stretch shorter than its window.
- [ ] Rule. Ship `Auto` as the default only when the Appendix F rule holds on the public sets and a 60-minute two-language recording finishes within 1.5 times trunk's time. If `chunks` beats `covered` by 2 points or more on both sets, write a decision row and plan phrase windows as their own PR. Fail this PR when a one-language or pinned recording gets slower than trunk by more than 2%.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1 and lane 10 screenshots into `docs/design/review/b3-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of turning the switch off and on and processing `mix_tl3_en0` each way. Save it as `docs/design/review/b3-review.mp4`.
- [ ] Post the screenshots, the video, and the lane 2 to 4 scores in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends B3 to the stack on B2's branch.

## Label each turn with its language and read it aloud (B4)

**Depends on.** B3, and `feat/output-language` on trunk.

**Files.**

- [x] Edit `crates/dory-core/src/domain/transcript.rs` and `crates/dory-core/src/app/stages/mod.rs`.
- [x] Edit `crates/dory-core/src/assistant/turns.rs` and `apps/dory-mcp` wire types, so `get_transcript` returns each turn's language.
- [x] Edit `apps/dory-gpui/src/app/session.rs`, `apps/dory-gpui/src/transcript_view.rs`, `apps/dory-gpui/src/export.rs`, and `apps/dory-gpui/src/a11y.rs`.
- [x] Edit `apps/dory-gpui/tests/` with a new `turn_languages.rs`, and `crates/dory-core/tests/pipeline.rs`.
- [x] Edit `docs/plans/ux-redesign.md` UX-26 and A11Y-10.

**Build.**

- [x] Name the data shapes first. A segment's language is output-language's `Segment.language: Option<LangTag>`, set by the transcribe stage to the language heard for the window or recovered stretch the segment came from, after B3's `settle_language`. It is the segment's main language, the one its grammar follows. There is no "mixed" value and no list of languages per segment. Appendix B says why.
- [x] `Transcript::language_shares(&self) -> Vec<(LangTag, Fraction)>` sums each language's timed speech over the segments, most first. `detected_languages` is now derived from it at write time, and `main_language()` stays its first entry.
- [x] `Transcript::is_bilingual(&self) -> bool` is true when two or more languages each hold at least `BILINGUAL_SHARE` of timed speech. `BILINGUAL_SHARE` is 10%, the Appendix G default. A meeting with one English greeting in an hour of Tagalog is not bilingual and gets no tags.
- [x] `session::turns` starts a new turn when the speaker or the segment language changes. `Turn` gains `language: Option<LangTag>` and `tag: Option<SharedString>`. `tag` is the language's name when the transcript is bilingual and the turn's language is not the main language, and `None` otherwise.
- [x] The transcript view draws the tag after the speaker name, as a small text label in `text_secondary`, never as color alone (A11Y-8). The turn's accessible label becomes "Sam, 12 minutes 30 seconds, English, then the text" when it has a tag, and is unchanged when it does not (A11Y-10).
- [x] Set the turn text node's language to the segment's tag through accesskit's `Node::set_language`, so VoiceOver switches to a Tagalog or English voice for the turn. If `gpui-kit` 0.9.1 does not expose it, the owner adds the smallest setter in `apps/dory-gpui/src/a11y.rs` or logs a decision row that it stays a follow-up. Appendix C records the risk. Result: `gpui-kit` 0.9.1 builds each node from a private struct and no app code can reach the node, so the decision log records that it stays a follow-up.
- [x] Markdown and plain-text export write the tag after the speaker name, "Sam (English):", for the same turns the view tags. SRT is unchanged.
- [x] `assistant::turns` groups turns the same way, and `get_transcript` gains `language` per turn.
- [x] With an output language set, each turn's tag still names the spoken language, in both the Original and the Translated view. A Taglish segment whose main language is Tagalog is translated whole when the target is English and kept whole when the target is Tagalog. No part of a sentence is translated alone. A pipeline test pins both cases against output-language's `needs_translation`.

**You see.**

- [ ] In `mix_tl3_en0` processed with recovery on, and in the operator's bilingual meetings, the English turns of a mostly Tagalog meeting carry an "English" label after the speaker's name. A meeting held in one language shows no labels.
- [ ] VoiceOver reads a tagged turn as "Sam, 12 minutes 30 seconds, English, then the text", and reads the Tagalog turns with a Tagalog-capable voice if the language attribute lands.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] `crates/dory-core/src/domain/transcript.rs` tests `language_shares` and `is_bilingual` at 9%, 10%, and 50%, with untimed segments ignored. Run `cargo test -p dory-core transcript`.
- [x] `apps/dory-gpui/src/app/session.rs` tests that a language change splits a turn of one speaker, and that tags appear only on non-main turns of a bilingual transcript. Run `cargo test -p dory-gpui session`.
- [x] `apps/dory-gpui/tests/turn_languages.rs` checks the accessible label of a tagged and an untagged turn, and the export text. Run `cargo test -p dory-gpui --test turn_languages`.
- [x] `crates/dory-core/tests/pipeline.rs` gains `taglish_segment_is_translated_whole_or_not_at_all`. Run `cargo test -p dory-core --test pipeline`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Open "Weekly sync" at trunk and at head. Save `regress-mono.png`. Pass when both show the same turns with no language labels, because a one-language meeting is unchanged.
- [ ] Lane 2. Process `mix_tl3_en0` and `mix_en_tl3_en` with meeting languages `tl` and `en` and recovery on, and open each. Every later lane uses these processed copies. Save `tags-mixed.png`. Pass when each minority-language turn carries a label and each main-language turn carries none.
- [ ] Lane 3. Open a CS-FLEURS Taglish recording. Save `tags-taglish.png`. Pass when no turn carries a label, because every Taglish window's main language is Tagalog.
- [ ] Lane 4. Rename a speaker in `mix_tl3_en0`. Save `rename-tagged.png`. Pass when the label stays after the new name and the rename hint counts the speaker's turns across both languages.
- [ ] Lane 5. Export `mix_tl3_en0` as Markdown and as SRT. Save `export.png` of both files. Pass when Markdown has "(English)" after the tagged speaker and SRT has no tag.
- [ ] Lane 6. Set the output language to English, process `mix_tl3_en0`, and switch between Original and Translated. Save `translated-tags.png`. Pass when the English turn is not translated, the Tagalog turn is, and the labels name the spoken language in both views.
- [ ] Lane 7. Set the output language to Tagalog and process a CS-FLEURS Taglish recording. Save `taglish-kept.png`. Pass when no segment is translated, because their main language is already Tagalog.
- [ ] Lane 8. Operator lane. With VoiceOver on, move through the turns of `mix_tl3_en0`. Save `vo-tags.png` with `screencapture -x`. Pass when VoiceOver says "English" on the tagged turn, and note whether it switched voice for the Tagalog turns.
- [ ] Lane 9. Operator lane. Turn on **Increase contrast**, then set View > Bigger twice. Save `contrast-tags.png`. Pass when every label is readable and no turn text clips.
- [ ] Lane 10. Ask an assistant over `dory-mcp` for `get_transcript` of `mix_tl3_en0` through `scripts/mcp-probe.sh`. Save `mcp-language.png`. Pass when each turn carries `language`, and the tagged turn says `en`.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. The time of `session::turns` on "Two hour planning", from a `turns ms=<n> turns=<n>` log line that B4's first commit adds at trunk behavior, and the time from selecting the recording to the first paint of its transcript.
- [ ] Probe. Seven selections at trunk and seven at head, alternating, after processing it with meeting languages `tl` and `en`.
- [ ] Baseline. Record the trunk median first.
- [ ] Rule. Fail when the head median exceeds the trunk median by more than 5 ms, or when `session::turns` exceeds 2 ms on the 2-hour transcript.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 2, 6, and 9 screenshots into `docs/design/review/b4-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of moving through `mix_tl3_en0` with the labels, then switching to Translated. Save it as `docs/design/review/b4-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends B4 to the stack on B3's branch.

## Translate a summary on demand (B5)

**Depends on.** B4, and `feat/output-language` on trunk. B5 touches none of B3's or B4's files, so the root may move it below them if output-language lands first.

**Files.**

- [ ] Edit `crates/dory-core/src/domain/translation.rs` for `SummaryTranslation`, `crates/dory-core/src/domain/job.rs` for the goal bit and the stage, and `crates/dory-core/src/ports/store.rs` for the read and the changes.
- [ ] Edit `crates/dory-core/src/app/stages/translate.rs`, `crates/dory-core/src/app/stages/mod.rs`, `crates/dory-core/src/app/mod.rs`, and `crates/dory-core/src/testing/store.rs`.
- [ ] Edit `crates/dory-store/src/sqlite/mod.rs` and `crates/dory-store/src/sqlite/codec.rs`, and create `crates/dory-store/src/sqlite/schema_v10.sql`.
- [ ] Edit `apps/dory-cli/src/args.rs`, `apps/dory-cli/src/main.rs`, and `apps/dory-cli/src/view.rs`.
- [ ] Edit `apps/dory-gpui/src/summary_view.rs`, `apps/dory-gpui/src/menus.rs`, `apps/dory-gpui/src/actions.rs`, `apps/dory-gpui/src/languages.rs`, `apps/dory-gpui/src/processing.rs`, and `apps/dory-gpui/src/export.rs`.
- [ ] Create `apps/dory-gpui/tests/summary_translation.rs`. Edit `crates/dory-core/tests/pipeline.rs` and `crates/dory-store/tests/store_contract.rs`.
- [ ] Edit `docs/plans/ux-redesign.md` UX-4 for the Summary tab's switch and the menu item.

**Build.**

- [ ] Name the data shapes first. `SummaryTranslation` is `{ target: LangTag, produced_by: ModelRef, markdown: String }`, at most one per recording. `Goal` gains `translate_summary`, and `Stage` gains `TranslateSummary`. `Change` gains `SummaryTranslationWanted { id, target }`, which records the language asked for, and `SummaryTranslated { id, translation }`. `StoreRead` gains `summary_translation(&RecordingId) -> Option<SummaryTranslation>`.
- [ ] Make a translation impossible to show against the wrong summary. Both stores delete a recording's summary translation inside the same `apply_change` arm that handles `SummaryReplaced` and `SummaryRestored`, and the table cascades on recording delete. A new summary therefore never sits under an old translation. `schema_v10.sql` adds the table, after search's v8 and output-language's v9.
- [ ] `Library::translate_summary(&self, id, target: LangTag)` applies `SummaryTranslationWanted` and enqueues `Goal::TRANSLATE_SUMMARY` at the front of the queue, because the user is waiting. It refuses with a message when the recording has no summary, or when the summary's language is already `target`.
- [ ] Translate line by line, so the structure survives by construction. `translate::summary_lines(markdown) -> Vec<SummaryLine>` splits the Markdown into lines, each `{ markup: String, text: String }`. The markup is a heading's `#` run, a bullet, a number, or the indent, and a blank line has no text. The text parts go through output-language's `plan_batches` and `translate_batch` with its numbered markers and split-and-retry, then `translate::join_lines` puts each markup back in front of its translated text. Names, numbers, and dates are kept by output-language's `system_prompt`.
- [ ] The stage resolves the summary model exactly as Summarize does, so a missing or stopped model gives the same needs-attention row and fix. It reads the pace budget and publishes `StageProgress { stage: TranslateSummary }` before each batch, and commits `SummaryTranslated` with its `StageDone`.
- [ ] In the Summary tab, a summary with no translation shows **Translate…** at the right of the tab's action row. It opens a menu of the meeting languages, a separator, then every other language Dory names, without the summary's own language. **Recording > Translate Summary…** opens the same menu. While the job runs, the tab shows "Translating the summary into English…" with the processing card's progress.
- [ ] A translated summary shows an Original / Translated switch, the same control output-language puts on the transcript, with Translated selected. Under it, in secondary text, "Translated from Tagalog by gemma4:e4b". Copy Summary and Export use what is shown. **Translate…** stays available to pick another language, which replaces the translation.
- [ ] `dory-cli translate-summary <id> <tag>` runs the job and prints the translation. `dory-cli show <id> --summary-translation` prints a saved one.

**You see.**

- [ ] In "Pulong sa Tagalog", **Translate…** then English shows an English summary with the same three headings and the same bullets within about 10 seconds, and Original shows the Tagalog summary unchanged.
- [ ] After the summary is written again, by `dory-cli process <id> --summary` or an assistant's re-summarize, or after `dory-cli summary restore <id>`, the switch and the translation are gone, and **Translate…** is back.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/src/app/stages/translate.rs` tests `summary_lines` and `join_lines` on headings of each level, `*`, `-`, and numbered bullets, nested indents, blank lines, `None`, and a line with only bold text, and checks that joining the untranslated lines gives back the input byte for byte. Run `cargo test -p dory-core translate`.
- [ ] The store contract suite gains `summary_translation_round_trips`, `a_new_summary_drops_its_translation`, `restoring_a_summary_drops_its_translation`, and `deleting_a_recording_drops_its_translation`, run against `MemStore` and SQLite. Run `cargo test -p dory-store --test store_contract` and `cargo test -p dory-core --all-features`.
- [ ] `crates/dory-core/tests/pipeline.rs` gains `summary_is_translated_line_by_line`, `translating_into_the_summary_language_is_refused`, `no_summary_model_blocks_translation_like_summary`, and `a_quit_mid_translation_resumes_once`, with a fake LLM. Run `cargo test -p dory-core --test pipeline`.
- [ ] `apps/dory-gpui/tests/summary_translation.rs` checks the menu order, the switch, the note under it, and that Copy Summary copies what is shown. Run `cargo test -p dory-gpui --test summary_translation`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe. Lanes that call Gemma run with Ollama serving `gemma4:e4b`.

- [ ] Lane 1. Regression lane against trunk. Open "Pulong sa Tagalog" at trunk and at head. Save `regress-summary.png`. Trunk has no way to translate a summary, so record that. Pass when head translates it into English, and `sqlite3 -readonly` shows the stored summary byte-identical before and after.
- [ ] Lane 2. Compare the original and the English translation of lane 1. Save `structure.png` of both. Pass when both have the same number of headings, bullets, and lines, in the same order.
- [ ] Lane 3. Switch between Original and Translated, then use Copy Summary and Export Markdown in each. Save `switch-copy.png`. Pass when the clipboard and the file match the shown version each time.
- [ ] Lane 4. Translate, then run `dory-cli process <id> --summary` to write the summary again. Save `resummarized.png`. Pass when the translation and the switch are gone and **Translate…** is back.
- [ ] Lane 5. Translate, then restore the previous summary with `dory-cli summary restore <id>`. Save `restored.png`. Pass when the translation is gone and the restored summary shows.
- [ ] Lane 6. Stop Ollama and choose **Translate…** > English. Save `blocked.png`. Pass when the needs-attention row and fix match what Summarize shows without a model, and starting Ollama then finishing the job shows the translation.
- [ ] Lane 7. Quit the app while a translation runs, then relaunch. Save `resume.png`. Pass when the job finishes once and the translation shows.
- [ ] Lane 8. Operator lane. With VoiceOver on, open the menu, translate, and move to the switch. Save `vo-translate.png` with `screencapture -x`. Pass when VoiceOver reads the menu items by language name, announces the switch as Original and Translated with the selected one, and reads the note.
- [ ] Lane 9. Process "Two hour planning" with Whisper and Gemma, then translate its summary into Tagalog. Save `long-summary.png`. Pass when every line of the original has a translated line and the time meets the perf rule.
- [ ] Lane 10. Run `dory-cli translate-summary <id> tl` on the Tagalog summary of "Pulong sa Tagalog", then open its menu in the app. Save `same-language.png`. Pass when the CLI refuses with a message and the menu does not offer Tagalog.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Trunk has no on-demand translation. The diff-added work is the time from choosing a language to the translated summary on screen. The alternative the user has today is writing the summary again with `dory-cli process <id> --summary`, which B5 is measured against.
- [ ] Probe. On an idle M3 Pro with `gemma4:e4b` warm, run five translations and five re-summaries each on "Pulong sa Tagalog" and on "Two hour planning", alternating.
- [ ] Baseline. Record the re-summarize medians first. The spike measured 12.8 seconds to write a 90-second meeting's summary again and 7.3 seconds to translate it.
- [ ] Rule. Fail when a translation of a summary up to 2,000 tokens takes over 20 seconds, or when translating "Two hour planning" takes more than half the time of summarizing it again.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 3, and 6 screenshots into `docs/design/review/b5-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of translating a summary, switching back to Original, and copying it. Save it as `docs/design/review/b5-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends B5 to the stack on B4's branch.

## Close the program

- [ ] Every box above is checked with its evidence.
- [ ] Reply to the operator with the report the execution playbook names, including the Appendix F scores and the open questions still on their defaults.

## Appendix A. Prototype evidence

A throwaway spike settled the recognition and language questions before this plan. It ran on 2026-11-17 on the operator's M3 Pro with sherpa-onnx 2.0.1, Whisper large-v3 turbo int8, CPU, and 4 threads. Its source is on branch `spike/bilingual-asr` at `1af58b7`, which never merges, and is copied with its scorer and raw output into `docs/plans/bilingual-spike/`. To rerun it, check out that commit, then run `cargo run --release -p dory-engines --example bilingual-spike -- <whisper dir> <clips dir> <out.jsonl> 6 [max clips]`. Add `COVER=1` for the coverage mode. Score with `python3 -I docs/plans/bilingual-spike/score.py <out.jsonl> <refs.tsv> [--cover]`.

### The audio

- **Turn switches.** Eleven clips of 17 to 30 seconds, each sent as one Whisper window, built by `make_turns.py` from the test library's own takes. Six hold one language, from "Pulong sa Tagalog" or "Weekly sync". Five hold a Tagalog turn and an English turn, or the reverse, with 400 ms of silence between. The references are Whisper's pinned-`tl` text for the Tagalog parts and Parakeet's text for the English parts, so they are close but not hand-checked, and they favour whole-window decoding on the single-language clips.
- **Taglish.** The first 24 `tgl_eng` clips of 29 seconds or less from the CS-FLEURS MMS test set, 403 seconds and 763 reference words. Each is one Taglish sentence with the switches inside it. The voices are MMS text-to-speech, joined at each switch, so every switch also changes the voice. The references are the dataset's text. CS-FLEURS is CC BY-NC 4.0. `clips.tsv` lists each file with its SHA-256.
- "Pulong sa Tagalog" in the test library is FLEURS read Tagalog with no English, so it served only as a source for the turn-switch clips.

### What today's pipeline does

- No Auto transcript stores a language. `transcripts.languages` is empty for "Pulong sa Tagalog", which Whisper transcribed on Auto. `SherpaEngine::transcribe` sends `language: None` on every segment and returns `TranscribeOutcome::default()`.
- The language is in the recognizer's JSON result as `lang`. Whisper on Auto put `tl` or `en` there on all 35 whole clips. The Rust `OfflineRecognizerResult` in sherpa-onnx 2.0.1 drops the field, and 2.0.1 was the newest release on 2026-11-17.
- sherpa-onnx's `SpokenLanguageIdentification` fails on turbo. It logged an invalid mel dimension, 80 given where 128 was expected, and returned an empty language on every call.
- A 29.9-second window logged "Only waves less than 30 seconds are supported. We process only the first 30 seconds and discard the remaining data", and its text ended "That's all from" where the speaker said "That's all from me". Dory's Whisper windows run up to 30 seconds today.
- With meeting languages left at any, Auto picks Parakeet, the recommended row, for a Tagalog meeting, and the transcript comes back with 0 segments. The output-language agent saw this live on "Pulong sa Tagalog" and reported it to this plan. This run did not reproduce it. Setting `tl` and `en` made Auto pick Whisper turbo.
- Four ways to get a language were tried. Whisper's own `lang` in the recognizer JSON works, on all 35 whole clips. sherpa-onnx's standalone identifier fails on turbo, as above. macOS NaturalLanguage has no Tagalog model. It called both Tagalog samples Indonesian and the Taglish samples Croatian and Portuguese, and with its languages restricted to `tl` and `en` it called all five samples English, pure Tagalog included. The pinned language is what output-language now falls back to (`14d9efd` on `feat/output-language`), and it says nothing about a meeting on Auto.
- `segment_timestamps` and `segment_durations` work with the turbo int8 export when `enable_segment_timestamps` is set. "Weekly sync" came back as 7 timed sentences. Token timestamps do not work. sherpa-onnx warned that the decoder has no cross-attention outputs.

### Word errors by recognition mode

Word errors are after lowercasing and stripping punctuation and hyphens on both sides. "Kept" is the share of the reference's Tagalog or English words found in the output. A word counts as English when it is in `/usr/share/dict/words` and not in the spike's Tagalog function-word list. "Turned into English" counts clips whose output has under half the reference's share of Tagalog words.

| Mode | Turn switches, all 11 | Turn switches, the 5 mixed | Taglish, 24 clips | Taglish English words kept | Turned into English |
| --- | --- | --- | --- | --- | --- |
| Whole window, Auto (today) | 13.6% | 26.0% | 35.9% | 27% | 1 of 11, 0 of 24 |
| Whole window, pinned `tl` | 11.8% | 22.4% | 35.9% | 27% | 0 of 11, 0 of 24 |
| Whole window, pinned `en` | 42.9% | 32.7% | 90.2% | 19% | 6 of 11, 11 of 24 |
| 6-second chunks, Auto | 6.9% | 4.3% | 43.4% | 34% | 0 of 11, 0 of 24 |
| 6-second chunks, pinned `tl` | 7.7% | 4.3% | 42.7% | 33% | 0 of 11, 0 of 24 |
| Sentence times, first pass only | 28.7% | 55.1% | 36.2% | 26% | 3 of 11, 0 of 24 |
| Sentence times, then re-decode uncovered stretches | 10.2% | 17.7% | 35.9% | 29% | 1 of 11, 0 of 24 |
| Best of pinned `tl` and pinned `en` per window, an oracle | 11.8% | 22.4% | 35.6% | 32% | 0 of 11, 0 of 24 |

What the table shows:

- **The main failure is a dropped turn, not English.** In `mix_tl3_en0`, Whisper on Auto and pinned to `tl` wrote the Tagalog sentence and nothing of the 10-second English turn after it, keeping 45% of the words. In `mix_tl1_en3`, Auto heard English and kept only the English turn, losing 20 seconds of Tagalog. Windows that started in English kept both languages.
- **Pinning `tl` is safe for English, and pinning `en` is not safe for Tagalog.** Pinned `tl` transcribed both English-only clips word for word. Pinned `en` turned Tagalog into broken English on 6 of 11 turn clips and 11 of 24 Taglish clips.
- **Chunks fix turn switches and hurt Taglish.** Cutting at 6 seconds took the mixed windows from 26.0% to 4.3%, but took Taglish from 35.9% to 43.4%, because each cut splits a sentence.
- **Re-decoding the uncovered speech fixes most of the dropped turns and leaves Taglish alone.** Mixed windows went from 26.0% to 17.7%, Taglish stayed at 35.9%, and only 4 of 24 Taglish windows needed a re-decode. Sentence times alone made switching windows worse, so they only make sense with the re-decode.
- **Picking between two pinned decodes is not worth two decodes.** The oracle that knows the reference gained 0.3 points on Taglish and nothing on turn switches.
- **No mode recovers Taglish's English words.** Whole windows kept 27% of them. The voice change at every switch in this synthetic set may cause that. No real Taglish will be measured, so this stays open (Appendix F).

### Language per window

- Whole windows on Auto heard `tl` or `en` on all 35 clips. Each mixed window got one label for two languages.
- 6-second chunks of the turn clips got the right language on 62 of 63 chunks, 1.6% wrong. The wrong one was a 2-second near-silent tail heard as English and written "you".
- 6-second chunks of Taglish were heard as Chinese once and Japanese once in 98 chunks, 2.0%, on chunks of 3.2 and 2.0 seconds, with nonsense text. Whole windows never left the meeting languages.
- Re-decoded stretches were heard as `en` 11 times, `tl` twice, and `es` once, on a 2-second silent tail of a Tagalog clip.

### Speed

Other agents were building and transcribing during the runs, with load averages from 4 to 22 on 11 cores. Only ratios from the same run are meaningful. 6-second chunks took about 2 to 3 times as long as whole windows on the same clips, because Whisper pads every call to 30 seconds and decodes each chunk separately. The coverage run re-decoded 6 stretches across 24 Taglish windows and 8 across 11 turn windows. The speech evaluation's 3 times real time for whole windows on an idle M3 Pro is the absolute reference.

### Search

Of 18 hyphenated words in the 24 Taglish references, Whisper pinned to `tl` wrote 8 with the hyphen, 3 with a space, and 1 joined. The rest were misheard. Appendix E uses this.

### Translating a summary on demand

The probe is `docs/plans/bilingual-spike/summary_translate.py`. It sent the test library's "Pulong sa Tagalog" transcript to `gemma4:e4b` through Ollama on this Mac, warm, at temperature 0, with Dory's summary prompt.

| Run | Wall time | Input | Output |
| --- | --- | --- | --- |
| Write the Tagalog summary from the transcript, today's path | 14.1 s | 394 tokens | 1,079 tokens |
| A. Write it again in English from the transcript | 12.8 s | 394 tokens | 983 tokens |
| B. Translate the Tagalog summary into English | 7.3 s | 237 tokens | 669 tokens |

- Both English results were accurate. B kept the Tagalog summary's headings, bullets, and every number. A rewrote the points in its own words and added a level of heading.
- Gemma read input at about 800 tokens a second and wrote at about 80 to 96. Writing a summary again rereads the transcript, which for a 2-hour meeting is 25,000 to 30,000 tokens by the speech evaluation's count, plus the map-reduce passes Dory runs when a transcript passes the context budget. That puts A at a minute or more for a long meeting. This is an estimate from the measured rates, not a run. B reads only the summary, a few hundred to 2,000 tokens, whatever the meeting's length.
- Output counts run well above the visible text, which suggests the model spends tokens before answering. That cost is in the wall times above.

### What stays unproven

- Everything on real Taglish meetings. The Taglish set is synthetic speech with a voice change at every switch, and the turn set's references are not hand-checked. No real meeting will be measured, because the operator keeps recordings private. Appendix F decides B3 on public audio and names the gap.
- Whether the VAD gate removes the made-up words that the spike's ungated re-decodes added on silent tails (Appendix C).
- Speed on an idle Mac. B3's perf block measures it.
- Whether VoiceOver can switch voice per turn through `gpui-kit` (Appendix C).
- Gemma's Tagalog summaries.
- Summary translation on a long meeting. Only a 90-second meeting was measured. B5's perf block measures "Two hour planning".

## Appendix B. Alternatives rejected

- **Short phrase windows for every two-language meeting.** 6-second chunks cut the mixed turn windows from 26.0% to 4.3% word errors, the best of any mode, but raised Taglish from 35.9% to 43.4%, cost 2 to 3 times the compute, and twice heard Chinese or Japanese on short chunks. Recovery keeps Taglish flat and costs a re-decode only where text is missing. The bench keeps the `chunks` mode, and B3's perf rule brings phrase windows back as their own PR if they beat recovery by 2 points on both public sets.
- **Two pinned decodes per window, keeping the better one.** Even an oracle that knows the reference gained 0.3 points on Taglish and nothing on turn switches, for twice the compute. A real chooser has no reference, so it would gain less.
- **Pinning the meeting to Tagalog by default.** It is 1.8 points better than Auto on turn switches and never turns Tagalog into English. But it drops turns the same way and labels every window Tagalog, so B2's languages and B4's labels would say nothing. Appendix G question 8 keeps it a per-recording choice.
- **Sentence times on every Whisper window.** They give every Whisper transcript sentence-level seek, which is what `catalog.rs` says Whisper lacks today. But on switching windows the first pass with sentence times lost more than the pass without them, 55.1% against 26.0%. Until a later change measures them on one-language meetings, they come on only with recovery.
- **Writing the summary again in the chosen language, instead of translating it (B5).** It needs no new stored item, since the existing previous-summary slot keeps the old one. But it rereads the whole transcript, 12.8 seconds for a 90-second meeting and an estimated minute or more for two hours. It also replaces the summary the user may have edited or restored, and the two versions no longer match point for point.
- **Translating the whole Markdown in one call (B5).** The spike's one call kept the structure. Nothing forces a model to, and a dropped or merged bullet would go unnoticed. Translating line by line through output-language's numbered markers checks every line and puts the markup back by code.
- **Showing a translation without saving it (B5).** Each look would cost Gemma another 7 seconds or more, and Copy and Export would need it again.
- **A "mixed" language value, or several languages per segment.** Taglish follows Tagalog grammar with English words dropped in, so a sentence has one main language that decides how to read, translate, and summarize it. A list per segment would put a label on nearly every Taglish sentence, which is noise, and translating only the English words of a Tagalog sentence breaks the sentence. Text-level word labels would also need a word-language detector per language in the core. One main language per segment, and none of that.
- **Translating only the non-target words of a mixed sentence.** "Bagaman wala ako present" cannot be translated piece by piece into English or into Tagalog. Output-language translates whole segments, and B4 pins that a Taglish segment is translated whole or kept whole.
- **A bilingual initial prompt for Whisper.** sherpa-onnx 2.0.1's `OfflineWhisperModelConfig` has no prompt field, and `TranscribeRequest::hint` is ignored by the sherpa engine. A prompt needs a fork of the C++ decoder. It stays a follow-up if the operator's set shows that routing is not enough.
- **sherpa-onnx's `SpokenLanguageIdentification`.** It fails on turbo, which is the only model Auto picks for Tagalog (Appendix C). A second, 80-mel Whisper model for language ID alone would add a download for a guess that turbo already makes during its own decode.
- **macOS NaturalLanguage on the transcript text.** `NLLanguageRecognizer` has no Tagalog model. Restricted to Tagalog and English, it called a pure Tagalog sentence English with full confidence. It would also make language a macOS-only adapter for something Whisper already reports for free.
- **Another code-switching model.** No sherpa-onnx model lists Tagalog except Whisper. SenseVoice and the Chinese-English zipformer handle code-switching only for East Asian languages and English. Qwen3-ASR scored 45% word errors on Tagalog FLEURS in the speech evaluation.
- **Writing two summaries, one per language, for "Same as spoken".** It doubles Gemma's time on every bilingual meeting to answer a question the output-language setting already answers.
- **Asking the user which language to summarize in after each bilingual meeting.** It interrupts the moment the user wanted to stop thinking about the meeting.
- **A separate summary PR with a bilingual note in the prompt.** Once B2 fills the main language, "Same as spoken" already resolves to the most-heard language. A prompt change with no measured gain does not earn a PR.
- **Tagalog stemming or a Tagalog fold in search.** Appendix E.
- **Calling the C result through `sherpa-onnx-sys` from `dory-engines`.** The spike did it to read `lang`, by casting the wrapper's private stream pointer. That cast depends on a private struct layout. A one-field vendored patch with an upstream pull request is safer and goes away on the next release.

## Appendix C. Risks

- **The spike's Taglish is synthetic (B3).** CS-FLEURS `tgl_eng` joins MMS text-to-speech voices word by word, so every switch is also a change of voice, and Whisper may treat a short English word as noise. Real Taglish is one speaker. No real Taglish will be measured, because the operator keeps meetings private. B3's switch lets the operator turn recovery off if it does worse on their own meetings.
- **The speed numbers came from a loaded Mac (B1, B3).** Other agents were building and transcribing during the spike, with load averages from 4 to 22 on 11 cores. Ratios between modes on the same clip are sound, because each mode ran back to back on the same audio. Absolute speeds are not, and the B3 perf lane remeasures them on an idle Mac.
- **The vendored sherpa-onnx patch outlives its welcome (B2).** `vendor/sherpa-onnx/PATCH.md` names the upstream pull request. The owner of any sherpa-onnx bump checks for the `lang` field and deletes the vendored copy in the same PR.
- **sherpa-onnx's standalone language identifier is broken for turbo (B2, B3).** It feeds 80 mel bins and turbo needs 128, so every call returns an empty language with a caught exception. Dory reads the language from the recognizer's own result instead and never calls it.
- **Recovered stretches add made-up words (B3).** The spike re-decoded every uncovered gap of 2 seconds or more, with no VAD, and Whisper wrote "you", "Thank you.", "Okay.", and nonsense on silent tails, which raised word errors on single-language clips from 0.4% to 2.1%. B3 re-decodes only VAD speech of 1.5 seconds or more. That gate is unproven, and B3 lane 4 checks it.
- **Sentence times change Whisper's first pass (B3).** With segment timestamps on, the first pass on switching windows lost far more (55.1% word errors against 26.0%), and only the re-decodes brought it back to 17.7%. That is why sentence times are tied to recovery. A one-language meeting never asks for them until a later change measures them alone.
- **Whisper translates a turn instead of dropping it (B3).** In `mix_tl1_en3`, Whisper heard English and wrote the Tagalog sentence in English. Recovery does not detect that, because the text covers the speech. Only the 6-second chunks fixed it. Phrase windows come back as their own PR only under B3's perf rule. B4's lanes avoid `mix_tl1_en3` for this reason and use `mix_tl3_en0` and `mix_en_tl3_en`, where recovery worked.
- **Mic bleed detection across languages (B3).** `merge::is_bleed` compares the mic text with the system text. Each channel is decoded on its own, so Whisper can hear the mic's echo in one language and the system turn in another, or recover a stretch on one channel only. Then the echo's text no longer matches and shows up as a second turn. B3 lane 8 checks a mic and system recording, and the owner adds a language-blind comparison only if it fails.
- **VoiceOver language switching (B4).** accesskit 0.24.1 has `Node::set_language`, and accesskit_macos 0.26.3 maps it to `NSAccessibilityLanguageTextAttribute`. This run did not find a `gpui-kit` 0.9.1 setter for it. Without one, VoiceOver still says "English" on a tagged turn, but reads Tagalog with the English voice.
- **The empty-transcript blocker fires on real silence or music (B2).** A recording of music or of one cough can hold 10 seconds of VAD speech and no words. The blocker then asks for meeting languages when none would help. The B2 owner checks it on a music clip and raises `HEARD_NOTHING_SPEECH` or narrows it to Parakeet's capability, never its name, if it misfires.
- **A translated summary loses meaning between bullets (B5).** Lines are translated in batches with their neighbours, as output-language does for segments, but a batch boundary can split a list. Summary lines are short and self-contained, so the owner checks lane 9's long summary by eye and records what was lost.
- **The summary model writes Tagalog poorly (B5).** Translating into Tagalog is as good as Gemma's Tagalog, which is unmeasured. The note under the switch names the model, and the Original stays one click away.
- **Gemma writing Tagalog (B2).** Once B2 fills the main language, a mostly Tagalog meeting gets a Tagalog summary under "Same as spoken". Gemma's Tagalog summaries are unmeasured. The operator can set the output language to English, and Appendix G question 2 asks whether that should be the default for bilingual meetings.
- **Output-language lands with a different segment field (B4).** B4 builds on `Segment.language: Option<LangTag>` from `docs/plans/output-language.md`. If that branch changes the field before landing, B4's owner follows it and records the change.
- **Schema numbers (B4).** B4 adds no schema. If a later change needs per-segment language data beyond output-language's column, it takes the next free version after search's v8 and output-language's v9.

## Appendix D. Links and reading list

- `docs/plans/speech-model-evaluation.md` measured Whisper turbo at 14.6% word errors on Tagalog FLEURS and called for the code-switching set that Appendix F specifies.
- `docs/plans/output-language.md` on `feat/output-language` defines `Segment.language`, the translate stage, and its skip rule.
- `docs/plans/search.md` on `docs/search-plan` defines the core tokenizer and fold that Appendix E checks against Tagalog.
- `docs/plans/ux-redesign.md` defines UX-26, UX-29, A11Y-8, and A11Y-10. B4 updates UX-26 and A11Y-10.
- `docs/architecture/hexagonal.md` defines the speech port and the composition root.
- `docs/plans/bilingual-spike/` holds the spike source, the scorer, the clip list with checksums, and the raw results. The spike branch is `spike/bilingual-asr`, which never merges.
- `docs/plans/output-language.md` on `feat/output-language` defines `translate::plan_batches`, `translate_batch`, and the Original / Translated switch that B5 reuses.
- `docs/plans/decisions.tsv` is the decision trail per pstack `show-me-your-work/SKILL.md`.
- Run pstack `how/SKILL.md` over `crates/dory-core/src/app/stages/` before B3. Run pstack `interrogate/SKILL.md` on B3 before review, because it changes the speech port and the transcribe stage.
- CS-FLEURS is at `https://huggingface.co/datasets/byan/cs-fleurs`, CC BY-NC 4.0, read 2026-11-17.

## Appendix E. Search needs no Tagalog rule

The search plan folds case and accents with one core rule and matches word starts with no stemming. Tagalog fits that rule as it is.

- Tagalog is written in Latin script. The 349 CS-FLEURS `tgl_eng` references hold 9,531 words and only 5 accented letters, and `ñ` folds to `n` under NFKD. Accent folding needs nothing new.
- Stemming would need a Tagalog morphological analyzer. Tagalog marks verbs with prefixes, infixes, and reduplication, so "kain" (eat) appears as "kumain", "kinain", and "kakainin". Word-start matching finds "kain" in "kainin" but not in "kumain". English stemming is absent too, and a per-language stemmer would need the query's language, which a search box does not know. No stemming for either language.
- Hyphens are the one Tagalog-heavy case. 1.5% of the reference words are hyphenated, such as "pag-aaral" and "kalunos-lunos". The search tokenizer splits at a hyphen and makes a hyphenated query a phrase, so "pag-aaral" finds "pag-aaral" and "pag aaral". It does not find "pagaaral". Of 18 hyphenated reference words, Whisper wrote 8 with the hyphen, 3 with a space, and 1 joined, so the phrase rule finds 11 of the 12 it heard right. One joined word in 763 is not worth a fold that would also make "well-known" stop matching "known".
- Search needs no PR in this program. If the operator reports search misses, the fix belongs in `dory_core::search::tokens`, which bumps `tokens_version` and reindexes, per the search plan.

## Appendix F. Decide B3 on public audio only

The operator will not share meeting recordings, for privacy, and no reference transcript of a real meeting will be made. B3's default is decided on the two public sets the spike used, which B1's script rebuilds on any Mac.

- **The turn-switch set.** Eleven clips built from the test library's own takes, "Pulong sa Tagalog" (FLEURS read Tagalog) and "Weekly sync" (English). Six hold one language, and five switch language between turns inside one window.
- **The Taglish set.** The 24 CS-FLEURS `tgl_eng` clips in `docs/plans/bilingual-spike/clips.tsv`, one Taglish sentence each, CC BY-NC 4.0, downloaded by checksum and never committed.

B3 ships recovery on for two-language meetings only when all of these hold at its head, run with `bilingual-bench score --modes auto,covered,chunks`.

1. On the five mixed turn-switch clips, `covered` scores 18% word errors or less, against 26.0% for `auto` in Appendix A, and at most one of the five still drops or translates a turn. The spike left two, though it scored word errors only and never counted turns: `mix_tl1_en3` came back with its Tagalog turn translated to English, and `mix_en_tl3_en` lost its last English turn (`docs/plans/bilingual-spike/results/turns-cover.jsonl`).
2. On the Taglish set, `covered` is within 1 point of `auto` on word errors and keeps at least as many English words.
3. On the six single-language clips, `covered` adds no words on silence and stays within 0.5 points of `auto`.
4. B3's perf rule holds.

The cost of this choice is that no real Taglish meeting is ever measured. The Taglish set is synthetic speech with a voice change at every switch. Two ways to close the gap stay open, and neither is planned.

- The operator can try recovery on their own meetings in the app and turn the B3 switch off if it does worse. Nothing leaves the Mac, and nothing is recorded for anyone else.
- Liva AI's code-switching corpus on Hugging Face (`liva-ai/code-s`) holds about 10 hours of spontaneous English and Tagalog conversation with human transcripts. Access needs the operator to agree to share contact details with its publisher, so it is the operator's call. This run did not open it.

## Appendix G. Open questions for the operator

Each question has a default. The PR named builds the default unless the operator answers otherwise.

1. **Should "Same as spoken" write a bilingual meeting's summary in its most-heard language?** Default yes (B2). Most-heard is by speech time. Writing two summaries doubles Gemma's time, and asking after every meeting interrupts. The output-language setting already covers "always English".
2. **For a mostly Tagalog Taglish meeting, is a Tagalog summary what you want?** Default yes, following question 1. If you would rather read English summaries of Taglish meetings, set Write transcripts and summaries in to English, which output-language adds. A Taglish-specific default is not built.
3. **How much speech must a second language hold before turns get language labels?** Default 10% of the meeting's timed speech (B4). Below it, a greeting or a quoted phrase in the other language does not label the meeting as bilingual.
4. **Should a label name the language, or show a short code?** Default the language's name, "English", in secondary text after the speaker's name (B4). A code such as "EN" is shorter but VoiceOver reads it as letters.
5. **Do you want the Tagalog function-word list the bench uses checked by a Tagalog speaker?** Default no. It only scores the bench, never the app, and only the English-only and dropped-turn counts use it.
6. **Will you share meeting recordings for the evaluation?** Answered no on 2026-11-17, for privacy. B3 is decided on public audio only, per Appendix F, and B3's switch lets you turn recovery off if it does worse on your own meetings.
7. **Is recovering dropped turns worth slower processing?** Default yes, up to 1.5 times today's time for a two-language meeting (B3 perf rule). A dropped turn is lost silently, and processing runs after the meeting. The spike re-decoded one short stretch in 1 of 6 Taglish windows.
8. **If B3 ships with recovery off, should a Tagalog and English meeting default to Language pinned to Tagalog?** Default no. Pinning `tl` scored 1.8 points better than Auto on turn switches and the same on Taglish, and it never turned Tagalog into English. But it still drops turns, it needs no new code, and you can already pick it per recording. B2's language labels would then all read Tagalog.
9. **When Auto's model hears nothing, should Dory switch to Whisper by itself?** Default no (B2). Whisper is a 1 GB download that needs your consent, and switching silently would hide that your meeting languages are missing. B2 shows a needs-attention row whose fix opens Meeting languages.
10. **Should a translated summary be saved?** Default yes (B5). It is saved beside the original, shown with the Original / Translated switch, and deleted when the summary is written again, so it never describes an older summary.
11. **Which languages should Translate Summary offer?** Default the meeting languages first, then every other language Dory names (B5). The output language, when set, is not offered for a summary already written in it.
12. **Should the plan use Liva AI's gated Tagalog and English corpus to measure real Taglish?** Default no. It needs you to share contact details with its publisher (Appendix F).
