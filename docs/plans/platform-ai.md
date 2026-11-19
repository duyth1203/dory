# Platform AI plan

Dory gains two engines built into macOS, so a new user gets transcripts and summaries with nothing to install. Apple Intelligence (the on-device Foundation Models framework) becomes a summary model, and Apple Speech (SpeechAnalyzer with SpeechTranscriber) becomes a speech model.
Users whose Mac has Apple Intelligence on get summaries with no install, best on short meetings, and a fast transcript when the meeting has one supported language. Users with Tagalog in a meeting keep Whisper and Gemma, because no Apple framework supports Tagalog on this Mac (Appendix A).
The rule the program enforces is that Dory never hands a meeting to a model that cannot read all of its languages, it says plainly why an Apple model is unavailable, and every transcript and summary names the engine that made it. No PR lands without unit, live, and perf evidence at its head.
The recommendation is to adopt for some roles. Apple Intelligence is the summary fallback when Gemma is not ready, good for short meetings and lossy for long ones (4 of 40 action items on a 2-hour meeting, against Gemma's 36), and Apple Speech is a speech row that Auto uses only for single-language meetings. Gemma, Parakeet, and Whisper stay the defaults until P5's numbers say otherwise. The Translation and NaturalLanguage frameworks are not adopted, and neither is Private Cloud Compute.
PRs P1, P2, P3, P4, and P5 land in that order as one linear stack. P0, the spikes, is done and never merges.
The measurements are in Appendix A, the architecture in Appendix E, and the operator's open questions in Appendix G.

## How to read this

One box is one unit of work. Every box names the evidence that checks it. A nested box is a sub-step of the box above it. Check a box only when its evidence exists, a file, a log line, a screenshot, a test run, or a SHA. The body is a how-to. The appendices explain and record.

The program runs `pstack/skills/poteto-mode/playbooks/autopilot-stack.md`. The root agent builds one linear stack from P1 to P5, and the operator reviews and lands it. P4 changes an interaction in the app and stops at merge-ready for the operator's review. P5 changes which engine Auto picks, so its decision rows wait for the operator's go even though it has no review gate (Appendix G, question 1).

Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

Requirement ids such as `UX-26` point into `docs/plans/ux-redesign.md`. Decision rows live in `docs/plans/decisions.tsv`.

A lane marked **Operator lane** needs a person at the Mac, because it turns Apple Intelligence off or on in System Settings, or needs a Mac on another macOS version. The root prepares the exact steps and the evidence to capture, and the operator runs it. Every other lane is agent-runnable.

pstack is a Claude Code plugin, not part of this repo. Paths that start with `pstack/` mean `~/.claude/plugins/cache/cursor-plugins/pstack/<version>/`. Validate this file with `node <pstack>/skills/poteto-mode/scripts/check-plan.mjs docs/plans/platform-ai.md`.

## Program checklist

### Arm the program

- [ ] State the protocol and this plan to the operator, then stop. Start execution only on the operator's explicit go.
- [ ] Get the operator's answers to Appendix G, or their go on the recommendations there. Log each answer as a decision row.
- [ ] Read these from trunk at program start. Re-read them at every tick.
  - [ ] `git show origin/main:docs/plans/platform-ai.md`
  - [ ] `git show origin/main:docs/architecture/hexagonal.md`
  - [ ] `git show origin/main:docs/plans/speech-model-evaluation.md`
  - [ ] `git show origin/main:docs/plans/ux-redesign.md`
  - [ ] `git show origin/main:AGENTS.md`
  - [ ] The bilingual plan and the output-language branch, once either lands. P1 binds to how a transcript reports its languages (Appendix C, risk 6).
  - [ ] The pstack copies of `poteto-mode/playbooks/autopilot-stack.md`, `poteto-mode/playbooks/opening-a-pr.md`, `swarm/SKILL.md`, `interrogate/SKILL.md`, `blast-radius/SKILL.md`, and `benchmark-checklist/SKILL.md`. pstack lives outside the repo, so `git show origin/main:` applies only to the repo files above.
- [ ] Re-check the Apple facts in Appendix D against the SDK on the build Mac before P2 starts, with `xcrun --show-sdk-version` and a grep of the `FoundationModels` and `Speech` `.swiftinterface` files. Apple changes these per OS release. Log what changed.
- [ ] On the operator's go, arm the audit tick as `/loop 1h` with the tick prompt below. Never leave the cadence to memory.
- [ ] Use this tick prompt, verbatim. "Re-read the execution playbook from trunk. Audit the operation against it and fix drift in this tick. Probe every active lane and judge progress by side effects only. Stand down a stuck lane and dispatch its replacement now. Then post a short status message to the operator in chat only when the audit found a tracked change that no earlier status message reported, such as a PR opened, a code-ready head, a round launched or closed, a verdict, a merge, a stuck agent and the action taken, a blocker added or cleared, or a decision only the operator can make. Name every such change and nothing else. Do not repeat a table, the merged list, or an unchanged blocker. If the audit found none, end the turn with no reply text. Either way, log this tick's row in your decision trail. The row names the items reported, or none."
- [ ] On the operator's hold or stand-down, send every owner a zero-writes order at once.

### Spawn owners

- [ ] Spawn one owner per PR with the full lifecycle the execution playbook names. Owners use `pstack:poteto-agent` on the `feature, refactoring` model from `~/.claude/rules/pstack-models.md`. P1 and P2 use the `hardest tasks` model, because P1 changes model selection for every job and P2 owns the only new `unsafe` and the Swift build.
- [ ] Follow this dependency graph. Each owner branches from its parent's branch, because the stack is linear.
  - [ ] P1 is first and branches from `feat/gpui-rewrite`, the program's trunk until it merges to `main`.
  - [ ] P2 after P1.
  - [ ] P3 after P2.
  - [ ] P4 after P3.
  - [ ] P5 after P4. P5's measuring can start on P3's head, because it needs only the two providers. Its commit stacks on P4.
- [ ] Hold the file boundaries. Each PR's **Files** list is its boundary. A PR that needs a file outside its list stops and asks the root, which either adds a small PR below it in the stack or amends this plan with a logged decision.
- [ ] Hold the hexagon. `dory-core` stays `#![forbid(unsafe_code)]` and gains no platform crate. Swift code and every `unsafe` line for it live in `crates/dory-macos`. Only `crates/dory` names the new providers. `dory-mcp` links no Swift. A review lane checks `cargo tree -p dory-core` and `otool -L` on the `dory-mcp` binary at every head from P2 on.
- [ ] Hold the review gate. P4 changes an interaction. It waits for the operator's review in chat with screenshots and a video before merge.

### PR mechanics, for every PR

- [ ] Resolve the forge once. Default to `gh`; if `command -v origin` succeeds and Origin can resolve the repository, use `origin pr` for every PR operation. Record any fallback to `gh`. Never require `gt`.
- [ ] Open the PR ready, never draft, per **Opening a PR**. Use the run's built-in PR tool when it has one, else `origin pr create --status open --base <base-branch>` or `gh pr create --base <base-branch>` according to the resolved forge. A stack child targets its parent branch.
- [ ] Run `cargo fmt --check`, `cargo clippy --workspace --all-features --all-targets -- -D warnings`, and `cargo test --workspace --all-features` once before the PR-facing push. Push with hooks on.
- [ ] Build with the shared target folder and per-crate codegen units, as `AGENTS.md` and the session brief say. Check `df -h /` first and keep at least 10 GB free. Never create `target/` inside a worktree.
- [ ] Run `/deslop` before each commit and `/no-comments` before review. Both come from `cursor-team-kit`. If the plugin is missing, write `Skipped in Claude Code` with the reason in the PR body.
- [ ] Triage every Bugbot and security-reviewer comment per `pstack/skills/poteto-mode/references/bugbot-triage.md`.
- [ ] Rebase onto current trunk before the code-ready report and babysit. Keep that merge base in fix rounds. Rebase again only at merge prep, on a `git merge-tree` conflict with trunk, or on a CI failure that comes from a change on trunk.
- [ ] Name branches `<type>/<slug>` per `AGENTS.md`, for example `feat/language-aware-fit` for P1 and `feat/apple-intelligence-summaries` for P2. Sign commits. No AI trailer in commit messages.
- [ ] Append a row to `docs/plans/decisions.tsv` for every real decision with the pstack `show-me-your-work/scripts/log.sh` helper. Mark any Apple fact you did not re-check this session as unverified in the row.
- [ ] Run `pstack/skills/blast-radius/SKILL.md` before the code-ready report on P1 and P2, because P1 changes how every job picks a model and P2 changes the build and the bundle.
- [ ] Run `pstack/skills/interrogate/SKILL.md` on P1 and P2 at the code-ready head. P1's attack focus is a meeting handed to a model that cannot read it. P2's is memory safety across the C ABI and cancellation.

### Verdict and merge, for every PR

- [ ] A full swarm runs once, at the code-ready head. Another full swarm runs only for new behavior, a redesign, or a large new diff. A rebase, a merge-conflict resolution, or a minor fix gets a single verifier at the new head. Run each full swarm per `pstack/skills/swarm/SKILL.md`. One gates lane. The ten live lanes from the PR's **Verify, live** block. The perf lane from its **Verify, perf** block. Two or more audit lanes, each with its own focus, that read the diff and the receipts and distrust the PR body. From P2 on, one audit lane always reads every `unsafe` block and every Swift `@_cdecl` function for ownership, lifetime, and thread rules.
- [ ] Clean only when every lane is `PASS`. An Operator lane that has not run is `NOT VERIFIED`, never `PASS`. Findings go back to the owner, including a defect that a lane filed as a note. A new head gets a fresh verdict, except for results that stay valid under the patch-id rule in `playbooks/shipping.md`.
- [ ] The root appends each clean PR to the stack with its base set to its parent's branch. The operator lands the stack bottom-up. A rebase that leaves `git patch-id` unchanged keeps the verdict. A changed patch-id needs a single verifier at the new head.

### Boot recipe, for every live lane

Live lanes run on the operator's Mac, an M3 Pro on macOS 26.5.2 with Apple Intelligence on, unless a lane names another Mac. They never touch the real data folder or the source test library. Lanes that drive the GPUI window run one at a time, because they share the screen. CLI lanes may run in parallel, each on its own library copy, except lanes that time an Apple engine, which run alone because the system services are shared (Appendix A, finding 9).

- [ ] `git fetch origin <head-branch> && git checkout <head SHA>`.
- [ ] Build with `CARGO_TARGET_DIR=<shared target> cargo build --release -p dory-cli`, and also `-p dory-gpui` for lanes that open the app. From P2 on the build needs Xcode with the macOS 26.4 SDK or later, because `build.rs` runs `swiftc`.
- [ ] Copy the test library with `cp -cR "$HOME/Library/Application Support/dory-dev/live-data" /tmp/dory-ai-lane-<n>`, then `rm -f /tmp/dory-ai-lane-<n>/.lock`. The copy is an APFS clone and costs no space. Never write to the source folder. Export `DORY_DATA_DIR=/tmp/dory-ai-lane-<n>`.
- [ ] The library holds three recordings. `Weekly sync` is 42.6 seconds of English, `Pulong sa Tagalog` is 90.5 seconds of Tagalog, and `Two hour planning` is 132 minutes of English with 40 action items that repeat four times. Lanes that need a Taglish meeting build `taglish.flac` once with `scripts/fixtures/taglish.sh` (created in P1), which joins the English and Tagalog takes with ffmpeg, and add it with `dory-cli add`.
- [ ] Choose engines with `dory-cli models choose stt|llm <id>` and `dory-cli languages <tags>`. Stop Ollama for a lane that needs it gone with `brew services stop ollama` or by quitting the Ollama app, and start it again at the end.
- [ ] From P2 on, pretend Apple's state with `DORY_APPLE=off|ineligible|preparing|old-os`, which is honored only when `DORY_DATA_DIR` is set, the same way `DORY_MACHINE` works. The real toggle is an Operator lane.
- [ ] Launch the app only with its environment passed explicitly, as `open -g --env DORY_DATA_DIR=/tmp/dory-ai-lane-<n> [--env DORY_APPLE=<state>] target/bundle/Dory.app`, or with `cargo run --release -p dory-gpui` from a shell that exports them. A bare `open` drops the shell's environment and would open the real data folder.
- [ ] `dory-cli process` has no Control-C handler, so a lane that cancels does it with the app's Cancel button.
- [ ] Score summaries with `scripts/score-summary.py <summary.md>` and transcripts with `scripts/score-items.py <transcript.txt>` (created in P5, copied from the spike's `score.py` in Appendix A). Run them with `python3 -I`.
- [ ] Drive the GPUI window only through the Claude Code computer-use tools (`app_click`, `app_key`, `app_type`, `app_screenshot`, `app_ax_find`), because no `control-ui` skill drives a native macOS window. CLI lanes follow `control-cli` from `cursor-team-kit` when it is installed. Capture only Dory's own window with `screencapture -l <window id>`. Never change System Settings from a lane.
- [ ] Read-only diagnostics are `dory-cli show <id>`, `dory-cli models --about`, `sqlite3 -readonly "$DORY_DATA_DIR/dory.sqlite"` (including the `jobs` table), and `ps -axo pid,time,rss,comm` for the `localspeechrecognition` and `TGOnDeviceInferenceProviderService` system services. Dory writes no unified log, so `log stream` shows nothing.
- [ ] Save every screenshot or transcript to `/tmp/swarm-<pr-id>/worker-<n>/<slug>` and return the paths with the report.

## Rule out models that cannot read every meeting language (P1)

**Depends on.** None.

**Files.**

- [x] Edit `crates/dory-core/src/models/fit.rs` and `crates/dory-core/src/models/mod.rs`.
- [x] Edit `crates/dory-core/src/app/models.rs` (`listing_needs` for both kinds).
- [x] Edit `crates/dory-core/src/app/stages/mod.rs` and `crates/dory-core/src/app/events.rs`.
- [x] Edit `crates/dory-core/src/ports/error.rs`.
- [x] Edit `crates/dory-store/src/sqlite/codec.rs`, `apps/dory-cli/src/view.rs`, `apps/dory-gpui/src/processing.rs`, and `apps/dory-gpui/src/models_view.rs`, for the new `UnavailableReason` variant only.
- [x] Edit `crates/dory/src/fake.rs`.
- [x] Edit `crates/dory-core/tests/pipeline.rs`.
- [x] Create `scripts/fixtures/taglish.sh`.

**Build.**

- [x] Replace `LlmNeeds::language` with `LlmNeeds { main: Option<LangTag>, all: Vec<LangTag> }`. `all` is the union of the recording's pinned language, `Settings::meeting_languages`, and `transcript.detected_languages`. Today the sherpa engines report no language (`crates/dory-engines/src/sherpa/engine.rs` returns `language: None` and a default outcome), so `detected_languages` is empty and the settings carry the signal. `listing_needs` fills `all` the same way from settings, so Settings shows the fit the job will get. Done differently: `all` became `spoken`, and the language the summary is written in no longer makes a meeting's languages known (2026-11-18).
- [x] `Llm::fit` returns `Yes` for a model whose `LlmCaps::languages` is `LangSet::Any`. A model with a limited language set fits only when `all` is not empty and every tag in it is supported. It returns `No(LanguageUnsupported)` for a tag outside the set, and `No(LanguagesUnknown)` when `all` is empty, with the text "needs your meeting languages set in Settings". This holds for `Auto` and `Pinned` alike. Apple accepts a Taglish transcript with no meeting languages set and summarizes only the English half (P2 live lane 4), and the sherpa engines detect no language, so Settings is the only guard. Gemma's `LangSet::Any` keeps it fitting every meeting.
- [x] Let a `FixedLocale` speech model fit `Auto` when `SttNeeds::expected` is not empty and every tag in it shares one primary language that the row covers. Add `pub fn run_language(caps: &SttCaps, needs: &SttNeeds) -> LanguagePref` in `fit.rs`, which returns `Pinned(<that language>)` for such a row and `needs.language` otherwise. The transcribe stage uses it for `TranscribeRequest::language` and `CheckpointKey::language`. An empty expected list keeps today's `No(AutoLanguageUnsupported)`.
- [x] Add `EngineError::Busy(String)`, classed `Transient`, for an engine that asks the caller to come back later. It is retried up to `MAX_ATTEMPTS` (3), and each retry reruns the whole summary, because the summary stage keeps no checkpoints.
- [x] Add `UnavailableReason::Preparing { detail: String }` for a model the OS is still downloading. A job blocked on it is rechecked by the triggers that free a job blocked on a stopped Ollama today, which are launch, a finished download, and Check again (`pipeline::unblock_all`). Add the variant to the store's JSON codec, the CLI view, and the GPUI processing and model views.
- [x] Fall back on `Auto`. When a summary engine chosen by `Auto` fails with `UnsupportedLanguage` or `InputRejected`, the stage re-resolves with that model excluded and restarts the summary from the transcript. It publishes a new `Event::DraftReset` first, so the UI drops the draft the first model streamed. Notes from two models never mix, and `produced_by` names the model that wrote the summary. A `Pinned` choice fails as it does today.
- [x] Add two fake rows to `crates/dory/src/fake.rs`, `fake/en-only-llm` (languages `en`, context 4096) and `fake/en-locale-stt` (`FixedLocale` English), and three knobs honored only when `DORY_DATA_DIR` is set. `DORY_FAKE_LLM=reject-language|busy:<n>` makes the fake summary engine fail that way, `DORY_FAKE_STT_LANGS=en,tl` sets the languages the fake transcriber detects, and `DORY_FAKE_ROWS=limited` keeps only the two limited rows in the fake catalogs.
- [x] Write `scripts/fixtures/taglish.sh`, which joins the `Weekly sync` and `Pulong sa Tagalog` takes into one FLAC with ffmpeg's concat filter and prints its path. Done differently: the script joins the takes with afconvert and Python's wave module, not ffmpeg.

**You see.**

- [x] `DORY_DATA_DIR=<copy> dory-cli models --fake-engines` after `dory-cli languages en tl` lists `fake/en-only-llm` as `does not fit: does not support this language`, and after `dory-cli languages en` lists it as fitting.
- [x] With `DORY_FAKE_STT_LANGS=en,tl`, `dory-cli process <id> --summary --fake-engines` on Auto never calls `fake/en-only-llm` and uses `fake/llm`.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] `crates/dory-core/src/models/fit.rs` tests gain `an_llm_must_read_every_known_language`, `a_limited_llm_needs_known_languages`, `a_fixed_locale_row_fits_auto_for_one_language`, `a_fixed_locale_row_never_fits_two_languages`, and `run_language_pins_the_one_language`. Run `cargo test -p dory-core models::fit`.
- [x] `crates/dory-core/src/models/resolve.rs` tests gain `auto_prefers_a_ready_fallback_over_an_unreachable_server`. Run `cargo test -p dory-core models::resolve`.
- [x] `crates/dory-core/tests/pipeline.rs` gains `auto_summary_falls_back_when_the_engine_rejects_the_language`, `the_draft_resets_on_fallback`, `a_pinned_summary_does_not_fall_back`, `busy_is_retried_then_fails_after_max_attempts`, and `preparing_unblocks_on_check_again`. Run `cargo test -p dory-core --test pipeline`.
- [x] `crates/dory-store` codec tests gain `preparing_round_trips`. Run `cargo test -p dory-store`.
- [x] `crates/dory-core/src/app/stages/summary.rs` keeps its map-reduce tests green and gains `a_4096_context_summarizes_a_two_hour_transcript` with a word counter. Run `cargo test -p dory-core stages::summary`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [x] Lane 1. Regression lane against trunk. With Ollama running, pin `sherpa/whisper-turbo`, set Auto for summaries, and run `dory-cli process <weekly id> --summary` at trunk and head. Save `regression.txt` with both `dory-cli show` outputs. Pass when both summaries come from `ollama/gemma4:e4b` with the same three headings and the same transcript `produced_by`.
- [x] Lane 2. Fake engines with `DORY_FAKE_STT_LANGS=en,tl` and `dory-cli models choose llm auto` on the weekly sync. Save `fake-taglish.txt`. Pass when the summary's `produced_by` is `fake/llm` and the job log never names `fake/en-only-llm`.
- [x] Lane 3. Same as lane 2 with `DORY_FAKE_ROWS=limited`. Save `fake-taglish-blocked.txt`. Pass when the summary step is blocked with "does not support this language" and no summary is stored. Done differently: the lane sets meeting languages to en first, because the limited speech row needs one language to transcribe at all.
- [x] Lane 4. Pin `fake/en-only-llm` with `DORY_FAKE_STT_LANGS=en,tl`. Save `fake-pinned.txt`. Pass when the job is blocked as incompatible and no fallback ran.
- [x] Lane 5. Run `dory-cli languages en`, then `dory-cli process <weekly id> --fake-engines` with Auto speech and `DORY_FAKE_ROWS=limited`. Save `fixed-locale-auto.txt`. Pass when `fake/en-locale-stt` is chosen and its checkpoint key names `en`.
- [x] Lane 6. Run `dory-cli languages en tl` and the same command. Save `fixed-locale-two.txt`. Pass when `fake/en-locale-stt` is not chosen and the step is blocked with a language sentence.
- [x] Lane 7. Clear meeting languages with `dory-cli languages --clear` and run the same command. Save `fixed-locale-unknown.txt`. Pass when `fake/en-locale-stt` is not chosen.
- [x] Lane 8. Run `dory-cli languages en`, then set `DORY_FAKE_LLM=reject-language` on Auto with `fake/en-only-llm` ranked first. Save `fallback.txt`. Pass when the summary's `produced_by` is `fake/llm`, the event log shows one `DraftReset`, and the stored summary holds no text from the first model.
- [x] Lane 9. Set `DORY_FAKE_LLM=busy:2`, then `busy:5`. Save `busy.txt`. Pass when the first job finishes after two retries and the second fails after three attempts with a retryable message.
- [x] Lane 10. With real engines, build `taglish.flac`, add it with `dory-cli add`, run `dory-cli languages en tl`, and process it on Auto with Ollama running. Save `taglish-real.txt`. Pass when Whisper transcribes it and Gemma summarizes it, exactly as on trunk.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] Metric. Wall time of `dory-cli models` on a migrated copy, which resolves every kind over the whole catalog.
- [x] Probe. `hyperfine --warmup 3 --runs 30 'dory-cli models'` on trunk and head release builds, interleaved trunk, head, trunk, head.
- [x] Baseline. Record the trunk median first.
- [x] Rule. Head fails if the median is more than 10 percent or 5 ms above trunk, whichever is larger.

**Review gate.** None. P1 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA. Open: one adversarial review covered P1 to P4, and its findings were fixed (2026-11-17). No clean verdict at an exact SHA.
- [ ] Bugbot triage done. Open: no PR yet, so no Bugbot.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged. Open: rebased onto 808e6a6 with the gates rerun, but no patch-id check logged.
- [ ] The root appends P1 to the stack with base `feat/gpui-rewrite`. The operator lands it. Open: merged locally into `feat/gpui-rewrite` in 67c50fb, with no PR stack.

## Summarize with Apple Intelligence (P2)

**Depends on.** P1.

**Files.**

- [x] Create `crates/dory-macos/build.rs`, `crates/dory-macos/swift/DoryAI.swift`, `crates/dory-macos/src/ai/mod.rs`, `crates/dory-macos/src/ai/ffi.rs`, and `crates/dory-macos/src/ai/foundation_models.rs`.
- [x] Edit `crates/dory-macos/Cargo.toml` and `crates/dory-macos/src/lib.rs`.
- [x] Edit `crates/dory/src/lib.rs` and `crates/dory/Cargo.toml`.
- [x] Edit `scripts/bundle-macos.sh` and `apps/dory-gpui/bundle.sh` (build prerequisites only).
- [x] Edit `docs/architecture/hexagonal.md` (the adapter rows and the Swift paragraph).
- [x] Create `crates/dory-macos/tests/foundation_models_live.rs` and `crates/dory/tests/providers.rs`.

**Build.**

- [x] Write `DoryAI.swift` as `@_cdecl` functions over a small C ABI, per Appendix E. Every call blocks the calling Rust thread, takes a `DoryTask` handle that another thread can cancel, and reports text pieces through a `(context, callback)` pair. Swift owns every string it returns until Rust calls `dory_string_free`.
- [x] Write `build.rs` to compile it with `xcrun swiftc -parse-as-library -emit-library -static -target arm64-apple-macosx26.0`, link `FoundationModels` and `Foundation`, and add `-rpath /usr/lib/swift`. It runs only for `target_os = "macos"`. The spike's build took 3.7 seconds and the binary linked only system Swift libraries (Appendix A, finding 8). Done differently: the bridge builds behind a feature with MACOSX_DEPLOYMENT_TARGET=26.0, and the rpath flag turned out to have no effect (2026-11-17).
- [x] Keep every `unsafe` line in `crates/dory-macos/src/ai/ffi.rs`. `foundation_models.rs` is safe Rust over it.
- [x] Add `dory_macos::FoundationModels`, a `ModelProvider<Llm>` with provider id `apple` and one row, `apple/foundation-model`, shown as "Apple Intelligence". Its caps read `contextSize` and `supportedLanguages` at runtime. `recommended` is false and `rank` is below Gemma's, so Gemma wins when both are Ready and Apple wins when Gemma is not Ready.
- [x] Map availability per Appendix E. `appleIntelligenceNotEnabled` and `deviceNotEligible` are `NotEligible` with their own sentences, `modelNotReady` is `Preparing`, and macOS below 26.4 is `OsTooOld { required: "macOS 26.4" }`, because `contextSize` and `tokenCount` need 26.4. Done differently: Apple Intelligence turned off maps to the new `UnavailableReason::TurnedOff` (2026-11-17).
- [x] Count tokens without calling Apple per line. `count_tokens` returns `ceil(utf8_bytes / 2.7)`, which stayed above Apple's `tokenCount` on every fixture in the spike, while a `tokenCount` call costs 37 to 378 ms and adds 8 tokens of its own (Appendix A, finding 6).
- [x] Run each completion in a fresh `LanguageModelSession` with `.permissiveContentTransformations` guardrails, `temperature` from the request, and `maximumResponseTokens` from `max_output_tokens`. A process-wide mutex serializes Dory's calls.
- [x] Map Apple's errors per Appendix E. `rateLimited` and `concurrentRequests` become `Busy`, `unsupportedLanguageOrLocale` becomes `UnsupportedLanguage`, `guardrailViolation`, `refusal`, and `exceededContextWindowSize` become `InputRejected`, and `assetsUnavailable` becomes `ModelMissing`.
- [x] Register the provider in `crates/dory/src/lib.rs` under `#[cfg(target_os = "macos")]` and the `app` feature, so `dory-mcp` and builds without `app` link no Swift.
- [x] Honor `DORY_APPLE=off|ineligible|preparing|old-os` only when `DORY_DATA_DIR` is set, for lanes.
- [x] Make `scripts/bundle-macos.sh` fail early with one sentence when `xcrun swiftc` or the 26.4 SDK is missing. The bundle keeps two executables and no dylibs. Entitlements do not change.

**You see.**

- [x] `dory-cli models` lists `apple/foundation-model  Apple Intelligence  ready  fits` on this Mac, and "Apple Intelligence is turned off" with `DORY_APPLE=off`.
- [x] With Ollama stopped, `dory-cli process <weekly id> --summary` writes a three-section summary whose `produced_by` is `apple/foundation-model`.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] `crates/dory-macos/src/ai/foundation_models.rs` tests gain `availability_maps_every_apple_state`, `errors_map_to_engine_errors`, and `the_estimate_never_undercounts`, which checks `count_tokens` against recorded `tokenCount` values from Appendix A. Run `cargo test -p dory-macos ai::`.
- [x] `crates/dory-macos/tests/foundation_models_live.rs` gains `a_short_summary_round_trips` and `cancel_stops_a_completion_within_two_seconds`, both `#[ignore]` unless `DORY_APPLE_LIVE=1`, because CI Macs may lack Apple Intelligence. Run `DORY_APPLE_LIVE=1 cargo test -p dory-macos --test foundation_models_live -- --ignored`.
- [x] `crates/dory/tests/providers.rs` gains `apple_rows_appear_only_with_the_app_feature`. Run `cargo test -p dory --test providers`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [x] Lane 1. Regression lane against trunk. Stop Ollama and run `dory-cli process <weekly id> --summary` at trunk and head. Save `regression.txt` with both `dory-cli show` outputs. Pass when trunk's job is blocked on the unreachable Ollama and head writes a summary with three sections and `produced_by` `apple/foundation-model`.
- [x] Lane 2. Start Ollama with Gemma ready and run the same command at head. Save `gemma-wins.txt`. Pass when `produced_by` is `ollama/gemma4:e4b`.
- [x] Lane 3. Transcribe `Two hour planning` with Parakeet, then pin `apple/foundation-model` and run the summary. Save `long-apple.md` and `long-apple.txt` with the wall time. Pass when the job finishes with no error, the summary has all three sections, and `scripts/score-summary.py`'s recall is recorded beside the spike's 4 of 40 (Appendix A, finding 5). Recall is recorded, not gated, because P2 does not change the prompts.
- [x] Lane 4. Add the Taglish recording, transcribe it with Whisper, and summarize it on Auto with Ollama stopped, first after `dory-cli languages en tl`, then after `dory-cli languages --clear`. Save `taglish.txt` with `TGOnDeviceInferenceProviderService` CPU time before and after each. Pass when the first run never calls Apple (its CPU time does not grow) and the second is refused by Apple, falls back, and ends blocked on Ollama with no summary stored. Done differently: Taglish failed at first, because Apple accepts it. After the unknown-languages fix the rerun passed, with Apple never called (2026-11-17).
- [x] Lane 5. Pin Apple on the Taglish recording. Save `taglish-pinned.txt`. Pass when the job is blocked with "Apple Intelligence doesn't support Tagalog" and no partial summary is stored. Done differently: the job blocked without the sentence at first. The 2026-11-18 fix names the language ("does not support Tagalog"), not re-run live.
- [x] Lane 6. Run `dory-cli models` with each `DORY_APPLE` value. Save `states.txt`. Pass when each prints its own sentence and fix, per Appendix E.
- [ ] Lane 7. Start the long summary in the app and press Cancel after 20 seconds. Save `cancel.txt` with `ps` samples of `TGOnDeviceInferenceProviderService` CPU time every second and `cancel.png`. Pass when its CPU time stops growing within 2 seconds and the job reads as cancelled. Open: the Cancel click in the app was not run. The live test `cancel_stops_a_completion_within_two_seconds` passes.
- [x] Lane 8. Background lane. Open the bundled app with `open -g --env DORY_DATA_DIR=<copy>` with Ollama stopped, keep another app in front, and queue the long summary. Save `background.txt` with each pass's result and `background.png` of Dory's window after. Pass when the summary finishes, and any `Busy` retries are counted in the file (Appendix C, risk 2).
- [x] Lane 9. Bundle with `scripts/bundle-macos.sh`, then run `codesign --verify --deep --strict`, `otool -L` on both executables, and the app on a copy. Save `bundle.txt` and `bundle-summary.png`. Pass when the signature verifies, `dory-gpui` links only `/usr/lib/swift` and system frameworks, `dory-mcp` links no Swift, and the app, opened with `open --env DORY_DATA_DIR=<copy>`, writes an Apple summary.
- [ ] Lane 10. Operator lane. The operator turns Apple Intelligence off in System Settings, Apple Intelligence & Siri, runs `dory-cli models` and `dory-cli show <weekly id>`, then turns it back on and runs `dory-cli models` again. Save `ai-off.txt`. Pass when the row reads "Apple Intelligence is turned off" with the fix, the stored summary still says `apple/foundation-model`, and the row is Ready again after. Open: operator lane, not run yet.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Wall time of the summary stage on the `Two hour planning` transcript, and the app's cold launch to first window. Open: the summary time is measured, app launch time is not.
- [ ] Probe. Run the summary on head with Apple pinned and on trunk with Gemma, three runs each, interleaved, with nothing else running, per `pstack/skills/benchmark-checklist/SKILL.md`. Time launch with `hyperfine --runs 10` on trunk and head bundles. Open: summaries ran interleaved, 5 Apple and 3 Gemma, but launch was not timed.
- [ ] Baseline. Record trunk's Gemma summary time and launch time first. Open: trunk's Gemma median is 152.2 s, with no launch baseline.
- [ ] Rule. Head fails if the Apple summary median is above trunk's Gemma median, because the fallback must not make users wait longer than the model it stands in for, or if launch is more than 5 percent slower than trunk. Under contention the spike measured 458 seconds for Apple and 511.6 for Gemma (Appendix A, finding 5). Open: failed, Apple 274.1 s against Gemma 152.2 s. Apple stays the fallback by the 2026-11-17 decision.

**Review gate.** None. P2 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA. Open: one adversarial review covered P1 to P4, and its findings were fixed (2026-11-17). No clean verdict at an exact SHA.
- [ ] Bugbot triage done. Open: no PR yet, so no Bugbot.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged. Open: no rebase or patch-id check logged.
- [ ] The root appends P2 to the stack with base P1's branch. The operator lands it. Open: merged locally into `feat/gpui-rewrite` in 67c50fb, with no PR stack.

## Transcribe with Apple Speech (P3)

**Depends on.** P2.

**Files.**

- [x] Edit `crates/dory-macos/swift/DoryAI.swift`, `crates/dory-macos/src/ai/ffi.rs`, and `crates/dory-macos/src/ai/mod.rs`.
- [x] Create `crates/dory-macos/src/ai/speech.rs` and `crates/dory-macos/tests/speech_live.rs`.
- [x] Edit `crates/dory/src/lib.rs`.
- [x] Edit `apps/dory-gpui/src/models_view.rs` (grouping the per-language rows only).

**Build.**

- [x] Add `dory_macos::AppleSpeech`, a `ModelProvider<Stt>` with provider id `apple` and one row per primary language that `SpeechTranscriber.supportedLocales` returns, `apple/speech-en` through `apple/speech-zh`, shown as "Apple Speech (English)" and so on. One row per language keeps availability honest, because each language has its own downloaded assets and `availability` takes no locale.
- [x] Give each row `SttCaps { languages: FixedLocale { tags: <its locales> }, diarization: None, max_input: None, window: Some(Ms(600_000)) }`. The 10-minute window gives checkpoints and resume points on long meetings, and diarization stays the separate diarizer's job. `recommended` is false and `rank` is below Parakeet's. Done differently: Apple rows also set a 30-second lead-in before each window (2026-11-18).
- [x] Map assets to availability. Installed is `Ready`, supported but absent is `NeedsProvision { bytes: None }`, and an unsupported language is not listed at all.
- [x] Implement `provision` with `AssetInventory.assetInstallationRequest`, reporting its `Progress` through `ProvisionSink`. When the 5-locale reservation limit is hit, fail with "Dory can keep 5 Apple Speech languages. Remove one to add another." Done differently: the limit is read from `maximumReservedLocales`, with 5 as the fallback, and was not tried at the limit.
- [x] Implement `transcribe` over `SpeechAnalyzer` with `SpeechTranscriber` and the `audioTimeRange` attribute, reading the clip's file and range with `AVAudioFile`. Map each result to a `RawSegment` with clip-relative times and the pinned language. Cancel with `cancelAndFinishNow`.
- [x] Group the per-language rows in `models_view.rs` under one "Apple Speech" heading, and hide rows whose fit is `No` unless one is pinned.

**You see.**

- [x] After `dory-cli languages en`, `dory-cli models` lists `apple/speech-en  Apple Speech (English)  ready  fits`.
- [x] With `apple/speech-en` pinned, `dory-cli process <weekly id>` writes 16 timed segments, and `dory-cli show` names `apple/speech-en`.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] `crates/dory-macos/src/ai/speech.rs` tests gain `rows_group_locales_by_language`, `asset_states_map_to_availability`, and `segments_are_clip_relative`. Run `cargo test -p dory-macos ai::speech`.
- [x] `crates/dory-macos/tests/speech_live.rs` gains `the_weekly_sync_transcribes` and `cancel_stops_within_two_seconds`, `#[ignore]` unless `DORY_APPLE_LIVE=1`. Run `DORY_APPLE_LIVE=1 cargo test -p dory-macos --test speech_live -- --ignored`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [x] Lane 1. Regression lane against trunk. Run `dory-cli languages en` and `dory-cli process <weekly id>` on Auto at trunk and head. Save `regression.txt`. Pass when both pick `sherpa/parakeet-tdt-0.6b-v3` and their segments are byte-identical.
- [ ] Lane 2. Pin `apple/speech-en` and transcribe `Two hour planning`. Save `long-apple.txt` with wall time, segment count, and `scripts/score-items.py` output. Pass when it finishes, every segment has a time range inside the take, and the score is at least the spike's 105 of 160 (Appendix A, finding 3). Open: scored 94, then 104 with the 30-second lead-in, against a bar of 105 (2026-11-18).
- [x] Lane 3. Rename `models/sherpa` in the lane copy so no sherpa model is Ready, set `dory-cli languages en`, and run the weekly sync on Auto. Save `fresh-install.txt`. Pass when Auto picks `apple/speech-en` and the transcript is written with no download.
- [x] Lane 4. Run `dory-cli languages en tl` and the weekly sync on Auto. Save `two-languages.txt`. Pass when no Apple row is chosen.
- [x] Lane 5. Run `dory-cli languages tl` and `dory-cli models`. Save `tagalog.txt`. Pass when no Apple Speech row claims Tagalog.
- [ ] Lane 6. Operator lane, because it installs German speech assets system-wide and they stay. Run `dory-cli languages de`, then `dory-cli models provision stt apple/speech-de`. Save `provision.txt` with the progress lines. Pass when progress rises to done and the row becomes Ready. Open: operator lane, not run yet.
- [x] Lane 7. Start the long transcription in the app, quit the app at 12 minutes of audio processed, and open it again. Save `resume.txt` with the `checkpoints` rows before and after. Pass when `localspeechrecognition` CPU stops within 2 seconds of quitting and the second run starts from the first 10-minute checkpoint instead of from zero. Done differently: checked at the CLI with SIGTERM instead of quitting the app. The rerun skipped the 3 saved windows and wrote an identical transcript.
- [x] Lane 8. Transcribe a 5-second silent FLAC made with `ffmpeg -f lavfi -i anullsrc=r=16000:cl=mono -t 5 silent.flac`. Save `silent.txt`. Pass when the job finishes with an empty transcript and no hang.
- [x] Lane 9. Run `dory-cli languages en-GB` and transcribe the weekly sync on Auto with no sherpa model Ready. Save `en-gb.txt`. Pass when `apple/speech-en` is chosen, runs on the installed `en-GB` assets with no download, and writes 16 timed segments.
- [x] Lane 10. Open Settings, Speech in the GPUI app with meeting languages `en`. Save `speech-rows.png`. Pass when Apple Speech shows as one group with the English row Ready, and rows that do not fit are hidden.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] Metric. Real-time factor and CPU-seconds, including the `localspeechrecognition` service, for `Two hour planning`.
- [x] Probe. Pin `apple/speech-en` at head and `sherpa/parakeet-tdt-0.6b-v3` at trunk, three runs each, interleaved, with nothing else running. Sample service CPU every second with the spike's sampler (Appendix A).
- [x] Baseline. Record trunk's Parakeet RTFx and CPU-seconds first. The spike measured 4.3 under heavy contention, against 14 on short clips in the 2026-11-16 run for Parakeet under load.
- [x] Rule. Head fails if Apple Speech is not at least twice Parakeet's RTFx, or uses more CPU-seconds than Parakeet. The spike measured RTFx 73 and 79 for Apple and 128 CPU-seconds.

**Review gate.** None. P3 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA. Open: one adversarial review covered P1 to P4, and its findings were fixed (2026-11-17). No clean verdict at an exact SHA.
- [ ] Bugbot triage done. Open: no PR yet, so no Bugbot.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged. Open: no rebase or patch-id check logged.
- [ ] The root appends P3 to the stack with base P2's branch. The operator lands it. Open: merged locally into `feat/gpui-rewrite` in 67c50fb, with no PR stack.

## Show Apple Intelligence where users set up summaries (P4)

**Depends on.** P3.

**Files.**

- [x] Edit `apps/dory-gpui/src/set_up_summaries.rs`.
- [x] Edit `apps/dory-gpui/src/settings_window.rs`.
- [x] Edit `apps/dory-gpui/src/onboarding.rs`.
- [x] Edit `apps/dory-gpui/src/models_view.rs` (wording only).
- [x] Edit `apps/dory-gpui/tests/settings.rs`.
- [x] Create `apps/dory-gpui/tests/set_up_summaries.rs`.

**Build.**

- [x] Add `Readiness::System { name }` to Set Up Summaries. When Auto resolves to a Ready Apple row, the dialog says "Summaries use Apple Intelligence on this Mac. Nothing to install." and offers "Use Gemma for long meetings" as the secondary path into today's two Ollama steps. Done differently: a `set_up_summaries::System` state replaces `Readiness::System` (2026-11-17).
- [x] When Apple Intelligence is off, step 1 reads "Turn on Apple Intelligence" with an Open System Settings button that opens the Apple Intelligence & Siri pane by URL, and the Ollama path stays beside it. When the Mac is not eligible, the dialog is today's Ollama flow.
- [x] Show the "Ollama is not running" section in Settings only when the chosen or resolved summary model is an Ollama row.
- [x] Change onboarding's "Summaries are optional and need Ollama." to "Summaries run on this Mac with Apple Intelligence." when Apple is Ready, and keep today's sentence otherwise.
- [x] Fill each Apple row's `ModelAbout`. Apple Intelligence is good at "No download, private, and fast on short meetings" and watch out for "23 languages and no Tagalog. Meetings longer than a few minutes are summarized in parts and can lose action items." Apple Speech is good at "No download for English, and fast" and watch out for "One language per meeting, and no Tagalog".

**You see.**

- [x] On a copy with Ollama stopped and Apple Ready, Set Up Summaries opens on the System path with no install step.
- [x] With `DORY_APPLE=off`, it opens on "Turn on Apple Intelligence" with the Ollama path beside it.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] `apps/dory-gpui/tests/set_up_summaries.rs` gains `apple_ready_needs_no_setup`, `apple_off_offers_both_paths`, and `ineligible_shows_the_ollama_flow`. Run `cargo test -p dory-gpui --test set_up_summaries`.
- [x] `apps/dory-gpui/tests/settings.rs` gains `ollama_section_only_for_ollama_choices`. Run `cargo test -p dory-gpui --test settings`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Open Set Up Summaries with Ollama stopped at trunk and head. Save `regression-trunk.png` and `regression-head.png`. Pass when trunk shows "Install Ollama" and head shows the System path. Open: head passes, the trunk side was skipped with no trunk GPUI build.
- [x] Lane 2. Open Set Up Summaries with `DORY_APPLE=off`. Save `ai-off.png`. Pass when "Turn on Apple Intelligence" and the Ollama path both show, and VoiceOver reads both buttons by name through `app_ax_find`.
- [ ] Lane 3. Press Open System Settings. Save `opens-settings.txt` with the frontmost app name after. Pass when System Settings is frontmost, and close it without changing anything. Open: needs clicks, not verified live.
- [x] Lane 4. Open Set Up Summaries with `DORY_APPLE=ineligible`. Save `ineligible.png`. Pass when it is today's Ollama flow with no Apple wording.
- [x] Lane 5. Open Set Up Summaries with `DORY_APPLE=preparing`. Save `preparing.png`. Pass when it says Apple is getting the model ready and offers no button that does nothing.
- [ ] Lane 6. Choose "Use Gemma for long meetings" with Ollama running and Gemma absent. Save `gemma-path.png`. Pass when it lands on today's Download step. Open: needs clicks, not verified live.
- [x] Lane 7. Open Settings, Summary with Ollama stopped and Apple Ready. Save `settings-summary.png`. Pass when no "Ollama is not running" section shows and the Apple row reads Ready.
- [x] Lane 8. Pin Gemma with Ollama stopped and open Settings. Save `settings-ollama.png`. Pass when the "Ollama is not running" section shows, as on trunk.
- [x] Lane 9. Run onboarding on a fresh copy with Apple Ready. Save `onboarding.png`. Pass when the summary sentence names Apple Intelligence.
- [x] Lane 10. Open each Apple row's About in Settings. Save `about-apple.png`. Pass when the pros and cons match the Build box word for word.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Time from opening Set Up Summaries to its first settled frame, and Settings open time. Open: not run, because it times clicks.
- [ ] Probe. Ten opens each on trunk and head bundles, interleaved, timed from the click to the dialog's first accessibility element appearing, polled with `app_ax_find`. Open: not run, because it times clicks.
- [ ] Baseline. Record trunk's medians first. Open: not run, because it times clicks.
- [ ] Rule. Head fails if either median is more than 50 ms or 10 percent above trunk, whichever is larger, because the Apple availability probe is a 79 ms call that must not run on the UI thread. Open: not run, because it times clicks.

**Review gate.** The operator reviews before merge.

- [x] Copy lane 1, 2, 4, and 7 screenshots into `docs/plans/media/p4-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of Set Up Summaries in the System, off, and Gemma states. Save it as `docs/plans/media/p4-review.mp4`. Open: no video, because recording Dory's window alone needs clicks between states.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click. Open: the screenshots are saved, but no operator review was recorded before the 67c50fb merge.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA. Open: one adversarial review covered P1 to P4, and its findings were fixed (2026-11-17). No clean verdict at an exact SHA.
- [ ] Bugbot triage done. Open: no PR yet, so no Bugbot.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged. Open: no rebase or patch-id check logged.
- [ ] The root appends P4 to the stack with base P3's branch. The operator lands it after the review. Open: merged locally into `feat/gpui-rewrite` in 67c50fb, with no PR stack.

## Measure Apple models and set the Auto order (P5)

**Depends on.** P4.

**Files.**

- [x] Edit `crates/dory-engines/examples/wer-bench.rs`, or move it to `crates/dory/examples/wer-bench.rs` if it must name an Apple provider, since only the composition root names adapters.
- [x] Create `scripts/score-summary.py`, `scripts/score-items.py`, and `crates/dory/tests/catalog_order.rs`.
- [x] Edit `docs/plans/speech-model-evaluation.md` (a new measured section).
- [x] Edit `crates/dory-macos/src/ai/foundation_models.rs` and `crates/dory-macos/src/ai/speech.rs` (`recommended` and `rank` only, if the rule below says so). Not needed: the rule kept the order on 2026-11-18.

**Build.**

- [x] Let the bench run every Ready speech row through `dory::open`'s providers, not only sherpa, with `--model <id>` to pick one.
- [x] Score English WER on the same 43 FLEURS clips as the 2026-11-16 run, for Apple Speech, Parakeet, and Whisper. Tagalog needs no run, because no Apple row covers it.
- [x] Time the 2-hour file for all three, three runs each, with CPU-seconds including system services.
- [x] Score summaries for Gemma and Apple on the same 10 transcripts with the E5 rubric in `docs/plans/speech-model-evaluation.md`, plus action-item recall on `Two hour planning` with `scripts/score-summary.py`.
- [ ] Repeat every Apple number on a Mac running macOS 27, because Apple rebuilt the on-device model for 27 (Appendix C, risk 1). This is an Operator lane if no such Mac is at hand.
- [x] Apply this rule and log it. Apple Speech becomes `recommended` for its languages only if its FLEURS English WER is within 1 point of Parakeet's and its RTFx is at least twice Parakeet's on the 2-hour file. Apple Intelligence outranks Gemma only if its rubric score is within 0.25 points of Gemma's and it keeps at least 90 percent of Gemma's action items. Otherwise the rows keep P2's and P3's order.

**You see.**

- [ ] `docs/plans/speech-model-evaluation.md` gains a table with WER, RTFx, CPU-seconds, rubric, and recall for all five engines on macOS 26.5 and 27.
- [x] `dory-cli models` shows the order the rule chose.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] `scripts/score-summary.py` and `scripts/score-items.py` get a doctest each on the spike's saved summaries in Appendix A. Run `python3 -I -m doctest scripts/score-summary.py scripts/score-items.py`.
- [x] `crates/dory/tests/catalog_order.rs` gains `auto_order_matches_the_logged_rule`. Run `cargo test -p dory --test catalog_order`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [x] Lane 1. Regression lane against trunk. Run the bench for Parakeet and Whisper at trunk and head. Save `regression.tsv`. Pass when their WER matches trunk to the word.
- [x] Lane 2. Run the bench for Apple Speech on the 43 English clips. Save `apple-wer.tsv`. Pass when every clip produced text and the WER is recorded.
- [x] Lane 3. Time `Two hour planning` for the three speech engines. Save `speed.tsv`. Pass when each has three runs with a spread under 10 percent and a named limiter.
- [x] Lane 4. Summarize the 10 rubric transcripts with Gemma and Apple. Save `rubric.tsv`. Pass when every transcript has both summaries and a score.
- [x] Lane 5. Score action-item recall on `Two hour planning` for both summary engines. Save `recall.txt`. Pass when both numbers are recorded with the transcript engine named.
- [ ] Lane 6. Operator lane. Repeat lanes 2 to 5 for Apple on a Mac running macOS 27. Save `macos27.tsv`. Pass when every Apple number has a macOS 27 value or a recorded reason it could not run.
- [x] Lane 7. Run `dory-cli models` on a copy after the rule is applied. Save `order.txt`. Pass when the order matches the decision row.
- [x] Lane 8. Run the Taglish recording on Auto after the change. Save `taglish.txt`. Pass when Whisper and Gemma still handle it.
- [x] Lane 9. Run the English weekly sync on Auto after the change, with `dory-cli languages en`. Save `english.txt`. Pass when the chosen engines match the rule.
- [ ] Lane 10. Operator lane. Unplug the Mac at full charge, transcribe `Two hour planning` with Apple Speech and then with Parakeet, and summarize it with Apple Intelligence and then Gemma, recording `pmset -g batt` before and after each and the CPU-seconds of each run including system services. Save `battery.txt`. Pass when every run has a battery drop and CPU-seconds recorded.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [x] Metric. The bench's own wall time for the full English set, so the bench stays cheap to rerun.
- [x] Probe. Run the bench on trunk (sherpa only) and head (sherpa only), interleaved, three times.
- [x] Baseline. Record trunk's median first.
- [x] Rule. Head fails if the sherpa-only bench is more than 10 percent slower than trunk.

**Review gate.** None. P5 is not review-gated. Its decision rows wait for the operator's go on Appendix G, question 1.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P5 to the stack with base P4's branch. The operator lands it.

## Close the program

- [ ] Every box above is checked with its evidence.
- [x] `docs/architecture/hexagonal.md` names `dory_macos::FoundationModels` and `dory_macos::AppleSpeech` with the C ABI and the rows, as P2 and P3 left it.
- [ ] Reply to the operator with the report the execution playbook names.

## Appendix A. Prototype evidence

P0 is a set of throwaway spikes run on 2026-11-17 against worktree HEAD 0a9f5fe. They never merge. Their sources, inputs, and outputs are in the session notes folder, `side-session-notes/local_8d5803dc-a234-43d2-a561-9658a1233a92/platform-ai-spike/` under `~/Library/Application Support/Claude/`. `probe.swift` reports what the Mac supports. `stt.swift` transcribes a file with SpeechTranscriber. `summ.swift` is a line-for-line port of `summary.rs` (its prompts, budgets, and map-reduce) that runs on Apple Intelligence or on Gemma through Ollama. `score.py` scores action-item recall. `shim/` is the Rust and Swift C ABI spike. The library was an APFS clone of the test library with `.lock` removed, and the source was never written.

The Mac is an M3 Pro with 18 GB on macOS 26.5.2 (25F84), with Xcode's macOS 26.5 SDK and Swift 6.3.3. Apple Intelligence is on. It was on AC power. Another session's `bilingual-spike` used 3 to 4 cores during most runs, and some of this plan's runs overlapped each other, so every time below carries contention unless it says otherwise. Recall and text results do not depend on contention. P5 re-times each engine alone.

1. [x] **Foundation Models is available here with a 4,096-token window and no Tagalog.** `SystemLanguageModel.default.availability` is `available`, `contextSize` is 4096, and `supportedLanguages` has 23 entries (Danish, German, three English, three Spanish, two French, Italian, Japanese, Korean, Norwegian, Dutch, two Portuguese, Swedish, Turkish, Vietnamese, and three Chinese). `supportsLocale` is false for `tl_PH` and `fil_PH`. Evidence is `probe.out`.
2. [x] **SpeechTranscriber covers 30 locales here and no Tagalog.** The locales are German, English, Spanish, French, Italian, Japanese, Korean, Portuguese, Cantonese, and Chinese variants. English in 9 regions is already installed. `supportedLocale(equivalentTo:)` returns nil for `tl-PH`, `fil-PH`, and `tl`, and maps `en-PH` to `en-US`. `DictationTranscriber` has 54 locales and no Tagalog either. An app may reserve 5 locales. No permission prompt appeared. Evidence is `probe.out`.
3. [x] **SpeechTranscriber is fast and runs outside Dory's process.** The 42.6-second English clip took 0.7 and 1.4 seconds on two runs, and its 16 sentences match Parakeet's stored transcript except "$5000" for "$5,000" and one split sentence. The 132-minute meeting took 99.6 and 108.1 seconds on two runs (RTFx 79 and 73) and gave the same 2,507 segments and 23,665 words both times. The work ran in the system service `localspeechrecognition`, which used 128 CPU-seconds and peaked at about 100 MB (102,128 KB resident), while the client used 0.06 CPU-seconds. The limiter is that service, which averaged about 1.2 cores. On the 160 action-item sentences it got the exact owner and item in 105 (66 percent). Parakeet on the same file through `dory-cli process` took 1,830 seconds (RTFx 4.3), 3,913 CPU-seconds, 2.76 GB resident at most, and a 4.13 GB peak footprint, and got the exact owner and item in 87 of 158 sentences (55 percent). Parakeet's run overlapped the other session's spike and both long summary runs far more than SpeechTranscriber's runs did, so the wall-time ratio overstates Apple's lead. The CPU-seconds gap of about 30 times is the sturdier number. Owner spellings such as Priya and Aisha are this plan's guesses, so the item score only compares engines. Evidence is `long.st.log`, `long.st.run2.log`, `st.samples`, `long.st.tsv`, `parakeet.log`, `long.parakeet.text`, and `raw-reruns.txt`.
4. [x] **Translation and NaturalLanguage have no Tagalog.** `LanguageAvailability` reports English to `tl` and to `fil` as `unsupported` among 38 languages. `NLLanguageRecognizer` read a Tagalog sentence as Portuguese 0.68 and Indonesian 0.22, and a Taglish one as English 0.99. Evidence is `probe.out`.
5. [x] **Apple Intelligence summarizes short meetings fast and loses action items on long ones.** On the weekly sync its one pass took 2.8 and 4.2 seconds on two runs, against Gemma's 10.9 and 103.1 seconds (the second a cold model load under contention). It filed Maria's task under Decisions and missed the budget decision, which Gemma got right. On the 132-minute transcript it ran 36 passes in 458 seconds with no error and kept 4 of 40 action items. Gemma on the same transcript through the same harness took 511.6 seconds over 29 passes and kept 36, 29 with the right owner. The first loss is in the notes passes. On a 7-minute chunk holding 8 action items, the notes prompt made the model copy the transcript line by line until the 512-token cap cut it off after 2. A stricter notes prompt kept all 8 on two runs and added one wrong item each time. With the stricter prompt the whole meeting kept 6 of 40, in 378.7 seconds over 31 passes, because the merge passes (512 tokens out) and the summary pass (1,024 tokens out) also drop items. One pass holds about 6 minutes of this meeting, which runs about 450 tokens a minute. Evidence is `long.fm.md`, `long.gemma.md`, `long.fm-strict.md`, `chunk1.fm.notes`, `raw-reruns.txt`, and the pass logs.
6. [x] **Apple's token count has a per-call cost and overhead.** `tokenCount(for: "")` is 8. Over 200 transcript lines the per-line counts summed to 6,312 against 4,720 for the joined text. Both measurements ran while a generation ran in another process, and a call took 37 ms on the short weekly lines and 378 ms on the long transcript's lines, so a per-line count of a 2-hour transcript costs minutes either way. Dory's Ollama estimate, bytes divided by 3, gave 295 where Apple counted 322 for the weekly sync, 8 percent low, and 4,752 against 4,720 for the 200 lines. Bytes divided by 2.7 gives 328 and 5,280, above Apple on both. Evidence is `tokencount.txt` and `tc.swift`.
7. [x] **Guardrails let ordinary sensitive meetings through, and Tagalog is refused.** Nine English snippets about layoffs, an overdose, ransomware, harassment, profanity, a murder podcast, cannabis, a campaign ad, and guns got three-section summaries with no refusal under both `.default` and `.permissiveContentTransformations`. The Taglish snippet and the library's pure Tagalog recording both failed in 0.1 to 3.1 seconds with `unsupportedLanguageOrLocale` under both settings. Evidence is `guard/` and `raw-reruns.txt`.
8. [x] **Rust can call Swift through a C ABI with no runtime to ship.** A `build.rs` compiled a `@_cdecl` Swift file with `swiftc -emit-library -static` and linked it into a Rust binary in 3.7 and 3.0 seconds on two builds. The 438,608-byte binary links only `/usr/lib/swift` system libraries and `FoundationModels.framework`, with `-rpath /usr/lib/swift` for `libswift_Concurrency`. The availability probe took 79 and 91 ms, and a first completion 6.2 and 4.7 seconds. FoundationModels and SpeechAnalyzer ship no Objective-C headers, so `objc2` cannot reach them. Evidence is `shim/` and `shim/run.txt`.
9. [x] **Timings here are contended.** Another session's `bilingual-spike` used 3 to 4 cores for most of the session, and this plan's long runs overlapped. The slowdowns in findings 5 and 6 suggest Apple's services are shared by every process on the Mac, which is an inference, not a measurement. Lanes that time an Apple engine run alone.
10. [x] **Background rate limiting did not appear.** A small app launched with `open -g`, so it was never active, ran 25 calls as a regular app, 25 as a menu-bar app (`LSUIElement`), and 120 more as a menu-bar app. All 170 succeeded, with no `rateLimited`, in 27.8, 17.5, and 83.6 seconds. The slowest call was the regular app's first, at 10.09 seconds while the model loaded, and after that no call took over 1.92 seconds. Apple does not say what counts as background on macOS, and this does not prove a limit never applies, so P2 lane 8 repeats it with the real app. A first attempt sent filler words ("item1 item2") and every call failed at once with `unsupportedLanguageOrLocale`, so text the model cannot place in a language is refused like Tagalog. Evidence is `bg/`.
11. [x] **Guided generation looped.** A `@Generable` notes type with an action-item array repeated one item until the 512-token cap and threw `decodingFailure`. Evidence is `notes2.swift`.
12. [ ] **Unproven here.** FLEURS English WER for SpeechTranscriber, because the clips were not on this Mac (P5 lane 2). Every number on macOS 27 (P5 lane 6). Battery drain, because the Mac was on AC and `powermetrics` needs root (P5 lane 10). Apple Intelligence turned off (P2 lane 10). A build with Command Line Tools and no Xcode, which no lane covers (Appendix C, risk 8). Whether the real app, as opposed to the spike app, is ever rate-limited in the background (P2 lane 8).

## Appendix B. Alternatives rejected

- **A bundled Swift helper process speaking JSON lines over stdio.** A second design runner built this shape in full, and it is the strongest alternative. It isolates crashes, needs no `unsafe`, keeps `swiftc` out of `cargo build`, and lets CI test the adapter against a fake helper. It loses on three counts. It is a third executable to bundle, sign, and version against the app. It does not escape the background rate limit, because the limit follows the app the user sees, and Dory is usually behind the meeting app either way. And most of the crash isolation already exists, because the speech work runs in the system's `localspeechrecognition` service and generation in `TGOnDeviceInferenceProviderService`, not in Dory's process (Appendix A, findings 3 and 5). `docs/architecture/hexagonal.md` already chose one Swift static library in `dory-macos`. The helper's best ideas are grafted here. One is the fake switch for lanes (`DORY_APPLE`). Another is one speech row per language. The last is restarting a summary on the next model instead of mixing notes.
- **`objc2` bindings.** FoundationModels and SpeechAnalyzer ship no Objective-C headers, only Swift interfaces (Appendix A, finding 8). `objc2` can reach only the old `SFSpeechRecognizer` and `NLLanguageRecognizer`, and neither earns a place.
- **A new crate for the Apple adapters.** The architecture doc names `dory-macos` as the home for SpeechAnalyzer and Foundation Models. A new crate would add a seam with one caller.
- **Putting Apple providers in `dory-engines`.** That crate is documented as portable. Swift would break it on every other platform.
- **Apple Intelligence as the default summary model.** On the 2-hour meeting it kept 4 of 40 action items against Gemma's 36 (Appendix A, finding 5). It is the fallback that needs no install, not the default, until P5's rule says otherwise.
- **A stricter notes prompt.** It made one chunk keep 8 of 8 action items instead of 2, but the whole 2-hour meeting kept only 6 of 40, because the merge passes and the 1,024-token summary also drop items. It would change Gemma's output for little gain. Appendix G, question 4 is the fix that addresses the cause.
- **Guided generation (`@Generable`) to extract action items.** It looped on one item until the output ran out and threw `decodingFailure` (Appendix A, finding 11). A stricter text prompt did better. Revisit on macOS 27's rebuilt model.
- **Using `tokenCount` for every chunk.** Each call costs 37 ms idle and 378 ms during a generation, and adds 8 tokens of its own. A 2-hour transcript has about 2,500 lines, so counting each would add minutes. A byte estimate that never undercounts is cheaper and safe.
- **Requiring the user to turn on Apple Intelligence from inside Dory.** No API turns it on, and Dory must not change System Settings. Dory opens the pane and says what to do.
- **The Translation framework for output language.** It does not support Tagalog or Filipino (Appendix A, finding 4), and the output-language branch already translates with Gemma.
- **NaturalLanguage for language ID.** `NLLanguageRecognizer` has no Tagalog and read a Tagalog sentence as Portuguese at 0.68 (Appendix A, finding 4). Meeting languages come from Settings, and from the speech model once the bilingual plan makes sherpa report them.
- **Private Cloud Compute (`PrivateCloudComputeLanguageModel`, macOS 27).** It sends the transcript to Apple's servers. That breaks Dory's promise that meetings never leave the Mac, which Set Up Summaries states today. It also needs an entitlement and has a daily quota. See Appendix G, question 3.
- **`DictationTranscriber` as a Tagalog fallback.** It covers 54 locales here, and none is Tagalog or Filipino.

## Appendix C. Risks

1. **Apple changes the model with the OS.** macOS 27 shipped on 2026-09-14 with a rebuilt on-device model, and Apple's sample output shows `contextSize` 8192 there without stating it. Every quality number in Appendix A is from macOS 26.5.2. P2 reads `contextSize` and languages at runtime instead of hard-coding them, `produced_by` names the engine but not the OS model version, and P5 repeats the numbers on macOS 27. Watch in P2 and P5.
2. **Background rate limiting.** Apple documents `rateLimited` only for apps in the background. Dory usually works behind Zoom or Meet, during and after the meeting. The spike measured no rate limit in 170 back-to-back calls from never-activated apps, more than four times a 2-hour summary's 36 calls (Appendix A, finding 10). P1's `Busy` makes a rate limit a retry, up to 3 attempts that each rerun the whole summary, and P2 lane 8 measures it with the real app hidden. If it is common, Apple summaries finish later, and P4's wording must not promise "right away". Watch in P2.
3. **Short context loses detail on long meetings.** At 4,096 tokens one pass holds about 6 minutes of meeting, notes passes may write 512 tokens, and the summary 1,024. Forty action items do not fit through that, whatever the prompt (Appendix A, finding 5). P4's wording says so, Set Up Summaries offers Gemma for long meetings, and Appendix G, question 4 proposes the real fix. macOS 27's model may have a larger window, which P5 measures. Watch in P4 and P5.
4. **Guardrail refusals.** Nine sensitive but ordinary meeting snippets got no refusal under either guardrail setting (Appendix A, finding 7). Real meetings are wider than nine snippets. P1's fallback moves a refused summary to Gemma on Auto, and a pinned Apple choice fails with the cause. Watch in P1 and P2.
5. **Taglish reaches Apple.** Apple Intelligence refuses mixed Tagalog and English at once with `unsupportedLanguageOrLocale`. P1 rules it out when the transcript lists Tagalog, and falls back when detection missed it. If Whisper labels a Taglish window English, the fallback is the only guard. Watch in P1 and P2.
6. **The bilingual plan changes how languages are reported.** P1 reads `transcript.detected_languages`. If the bilingual plan replaces it with per-segment languages, P1's owner reads that plan first and binds to its shape.
7. **A Swift trap kills the app.** In-process Swift aborts the whole process on a trap. The shim stays small, catches every thrown error, and does no force-unwraps, and the P2 audit lane reads every `@_cdecl` function. The heavy work runs in Apple's services, not in Dory.
8. **Build needs Xcode.** `build.rs` runs `swiftc`, so a contributor without Xcode cannot build `dory-macos`. P2's bundle script and `build.rs` fail with one sentence naming the fix. Command Line Tools alone are unverified.
9. **The availability probe blocks the UI.** It took 79 ms in the spike. P4's perf gate keeps it off the UI thread.
10. **Locale reservations run out.** An app may reserve 5 speech locales. P3 says so when the sixth is asked for. Releasing a reservation is not built, and becomes a follow-up if users hit the limit.
11. **Apple's language lists change.** The supported lists here are runtime values on macOS 26.5.2. P2 and P3 read them at runtime, so a new language appears without a release, and a removed one turns a row away instead of failing a job.

## Appendix D. Links and reading list

Read before editing.

- `docs/architecture/hexagonal.md` (the adapter table and the Swift paragraph), `AGENTS.md`, `docs/plans/speech-model-evaluation.md` (the measured section and the E5 rubric), and `docs/plans/ux-redesign.md`.
- `crates/dory-core/src/models/fit.rs`, `crates/dory-core/src/models/resolve.rs`, `crates/dory-core/src/models/mod.rs`, `crates/dory-core/src/ports/provider.rs`, `crates/dory-core/src/ports/llm.rs`, `crates/dory-core/src/ports/speech.rs`, `crates/dory-core/src/ports/error.rs`, `crates/dory-core/src/app/stages/summary.rs`, `crates/dory-core/src/app/stages/mod.rs`, and `crates/dory/src/lib.rs`.
- `apps/dory-gpui/src/set_up_summaries.rs`, `apps/dory-gpui/src/settings_window.rs`, `apps/dory-gpui/src/models_view.rs`, and `apps/dory-gpui/src/onboarding.rs`.
- The spike folder named in Appendix A.

Apple sources read on 2026-11-17. The SDK interfaces are the ground truth, because they are what the build compiles against.

- The macOS 26.5 SDK interfaces, `FoundationModels.swiftmodule/arm64e-apple-macos.swiftinterface` and `Speech.swiftmodule/arm64e-apple-macos.swiftinterface`, under `xcrun --show-sdk-path`. They give `contextSize` and `tokenCount` as macOS 26.4 and later, the three `UnavailableReason` cases, and the nine `GenerationError` cases.
- Foundation Models documentation through Context7 (`/websites/developer_apple_foundationmodels`), for the 4,096-token window that counts instructions, prompts, and outputs together, and for `rateLimited` applying only to apps in the background. https://developer.apple.com/documentation/foundationmodels/managing-the-context-window and https://developer.apple.com/documentation/foundationmodels/languagemodelsession/generationerror/ratelimited(_:)
- Speech documentation through Context7 (`/websites/developer_apple_speech`), for `AssetInventory` reservations and the note that the permission flow applies to `SFSpeechRecognizer`. https://developer.apple.com/documentation/speech/asking-permission-to-use-speech-recognition
- WWDC26 session 241, What's new in the Foundation Models framework, for the rebuilt model, the `LanguageModel` protocol, and Private Cloud Compute with a 32,000-token window. https://developer.apple.com/videos/play/wwdc2026/241/
- macOS 27's release date, 2026-09-14, from press coverage, not re-checked against Apple. https://www.macworld.com/article/3139330/macos-27-golden-gate-has-arrived-heres-what-s-new-on-your-mac.html
- Apple Intelligence hardware, language, and region limits, from secondary sources only, which disagree on storage. Apple silicon is required, mainland China is excluded, and the language list grew in 26.1. Re-check against Apple's own page before P4's wording lands. https://www.macrumors.com/2025/09/22/ios-26-1-apple-intelligence-languages/

P1 and P2 get `pstack/skills/how/SKILL.md` before code and `pstack/skills/interrogate/SKILL.md` at the code-ready head. The decision trail for this plan is the rows dated 2026-11-17 that mention platform AI in `docs/plans/decisions.tsv`, per `pstack/skills/show-me-your-work/SKILL.md`.

## Appendix E. Architecture

```
dory-gpui, dory-cli            apps, unchanged except P4's wording
        |
dory (composition root)        #[cfg(target_os = "macos")] + feature "app"
        |   pushes FoundationModels into llm, AppleSpeech into stt
dory-macos
  src/ai/foundation_models.rs  ModelProvider<Llm>, LanguageModel   (safe)
  src/ai/speech.rs             ModelProvider<Stt>, Transcriber     (safe)
  src/ai/ffi.rs                extern "C" block, callback trampolines (the only unsafe)
  swift/DoryAI.swift           @_cdecl shim, built by build.rs into libdoryai.a
        |
FoundationModels.framework, Speech.framework   (work runs in Apple's XPC services)
```

The C ABI. Every call blocks the calling Rust thread. Swift bridges its async APIs with a task and a semaphore, which the spike proved. A `DoryTask` handle lets `CancelToken`'s hook cancel from another thread. Strings are UTF-8, and Swift-allocated strings go back through `dory_string_free`.

```c
typedef struct DoryTask DoryTask;
DoryTask *dory_task_new(void);
void dory_task_cancel(DoryTask *);
void dory_task_free(DoryTask *);
void dory_string_free(char *);

typedef struct { int32_t state; uint32_t context_size; char *languages_csv; } DoryFmInfo;
int32_t dory_fm_info(DoryFmInfo *out);
int32_t dory_fm_complete(DoryTask *, const char *system, const char *user, uint32_t max_out,
                         float temperature, void *ctx,
                         bool (*on_piece)(void *ctx, const char *piece, size_t len),
                         char **full, char **error);

int32_t dory_speech_locales(char **csv);
int32_t dory_speech_asset_state(const char *locale);
int32_t dory_speech_install(DoryTask *, const char *locale, void *ctx,
                            bool (*on_progress)(void *ctx, double fraction), char **error);
int32_t dory_speech_transcribe(DoryTask *, const char *path, uint64_t start_ms, uint64_t end_ms,
                               const char *locale, void *ctx,
                               bool (*on_segment)(void *ctx, uint64_t start_ms, uint64_t end_ms, const char *text),
                               bool (*on_progress)(void *ctx, double fraction), char **error);
```

How Apple's states map to Dory's.

| Apple | Dory |
| --- | --- |
| `.available` on macOS 26.4 or later | `Ready` |
| macOS below 26.4 | `Unavailable(OsTooOld { required: "macOS 26.4" })` |
| `.appleIntelligenceNotEnabled` | `Unavailable(NotEligible)` "Apple Intelligence is turned off", fix "Turn it on in System Settings, Apple Intelligence & Siri" |
| `.deviceNotEligible` | `Unavailable(NotEligible)` "This Mac can't run Apple Intelligence", no fix |
| `.modelNotReady` | `Unavailable(Preparing)` "Apple is getting the model ready", retried on its own |
| Speech locale installed | `Ready` |
| Speech locale supported, not installed | `NeedsProvision { bytes: None }` |
| Speech language not supported | no row |

How Apple's errors map to `EngineError`.

| Apple | Dory | Class |
| --- | --- | --- |
| `rateLimited`, `concurrentRequests` | `Busy` | Transient, retried after `not_before` |
| `unsupportedLanguageOrLocale` | `UnsupportedLanguage(<main language, else und>)` | NeedsUser, and a fallback on Auto |
| `guardrailViolation`, `refusal`, `exceededContextWindowSize`, `decodingFailure`, `unsupportedGuide` | `InputRejected` | Permanent, and a fallback on Auto |
| `assetsUnavailable` | `ModelMissing` | NeedsUser |
| task cancelled | `Cancelled` | Cancelled |

How Auto picks after P1, with Gemma `recommended` and the Apple rows ranked below.

| Meeting | Speech | Summary |
| --- | --- | --- |
| English only, everything installed | Parakeet | Gemma |
| English only, no Ollama | Parakeet | Apple Intelligence |
| English only, meeting languages set to `en`, no sherpa model downloaded | Apple Speech (English) | Apple Intelligence |
| Tagalog, or Tagalog and English | Whisper | Gemma, or skipped |
| Languages not set | Parakeet or Whisper, as today | Gemma, else blocked or skipped, never Apple |

`produced_by` stays an opaque `provider/name` string, so stored values such as `apple/foundation-model` and `apple/speech-en` need no schema change. A summary keeps its `produced_by` when Apple Intelligence is turned off later. The next job re-resolves.

## Appendix F. What each framework can do for Dory

| Framework | macOS | Languages here | Tagalog | Context or limits | Where it runs | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| Foundation Models, on-device | 26.0, with `contextSize` and `tokenCount` from 26.4 | 23 | No | 4,096 tokens in and out together | On device, through `TGOnDeviceInferenceProviderService` | Adopt as the summary fallback (P2) |
| SpeechAnalyzer with SpeechTranscriber | 26.0 | 30 locales in 10 languages | No | One fixed locale per run, no diarization, 5 reserved locales | On device, through `localspeechrecognition` | Adopt as a speech row for one-language meetings (P3) |
| DictationTranscriber | 26.0 | 54 locales | No | Same as above | On device | Not adopted, SpeechTranscriber covers the same need |
| Translation | Present on 26.5, first version not re-checked | 38 | No | Pairs only between its languages | On device after a download | Not adopted |
| NaturalLanguage | Present on 26.5, first version not re-checked | Many | No | Text only | On device | Not adopted |
| Private Cloud Compute model | 27.0 | Not stated | Not stated | 32,000 tokens, daily quota, entitlement | Apple's servers | Not adopted, see Appendix G |

Apple Intelligence needs Apple silicon, a supported device and Siri language, and a region outside mainland China, per secondary sources (Appendix D). This Mac meets all of them.

## Appendix G. Open questions for the operator

1. **Should P5 change Auto's order on its own when the numbers pass the rule?** Recommend yes. The rule is written before the numbers exist, so applying it is not a judgment call, and you review the table before you land P5. Say no if you want to approve each switch by hand.
2. **Should Apple rows show at all when your meetings include Tagalog?** Recommend yes. They never get picked for a Tagalog meeting, they say why in Settings, and other users of Dory need them. Hiding them by meeting language would make Settings change under the user.
3. **Should Dory ever offer Private Cloud Compute, Apple's server model on macOS 27?** Recommend no. It leaves the Mac, and Dory promises meetings never do. If you want it later, it is one more `LanguageModel` row behind an explicit opt-in with its own wording, in its own plan.
4. **Should a follow-up plan carry action items and decisions through the merge in code?** Recommend yes, after P5, as its own plan. Notes passes would write each action item as one parsed line, and the core would dedupe them and write the final Action Items section itself, so no merge pass or output cap can drop one. It is the only change the spikes found that could make Apple summaries of long meetings usable, it would also lift Gemma's 29 of 40 owners, and it touches every summary, so it needs its own rubric run. Say no to keep Apple Intelligence as a short-meeting fallback only.
5. **Should the Apple summary row require macOS 26.4?** Recommend yes. `contextSize` and `tokenCount` arrived in 26.4, Dory's floor is 26.0, and a fixed 4,096 on older systems is a path nobody would test. Users on 26.0 to 26.3 see "Needs macOS 26.4" and keep Gemma.
6. **Should a fresh install transcribe English with Apple Speech while no sherpa model is downloaded?** Recommend yes. Auto does this after P1 and P3 when the meeting languages are set to English, so the first meeting gets a transcript with no 640 MB Parakeet download. Auto still never downloads.
7. **Can the P5 Operator lane run on a Mac with macOS 27?** This Mac is on macOS 26.5.2. Recommend running lane 6 on any Apple silicon Mac on 27, or updating this one when you choose to. No agent updates macOS.
