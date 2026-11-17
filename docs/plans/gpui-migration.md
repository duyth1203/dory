# GPUI migration plan

Dory moves from Tauri 2 and React to a native Rust app built on GPUI, and gets the redesign in `docs/plans/ux-redesign.md` at the same time.
Users get one-action recording without BlackHole, automatic processing, a sidebar and detail layout, export, and keyboard and VoiceOver support. The first platform adapter targets macOS 26.
The code follows the ports-and-adapters design in `docs/architecture/hexagonal.md`. The new core is built from scratch next to the Tauri app, which keeps running unchanged until P8.
The rule the program enforces is that no PR lands without unit, live, and perf evidence at its head.
PR P0 is a go or no-go spike on GPUI accessibility and runs alongside P1. Then P2, P3, P4, P5, P6, P6b, P7, and P8 land in that order. P7 and P8 are built in parallel from P6b, and P8 is verified only after it rebases onto P7.
If P0 says no-go, P1 to P3 still land unchanged and the operator picks a SwiftUI shell over the same core instead (Appendix B).

## How to read this

One box is one unit of work. Every box names the evidence that checks it. A nested box is a sub-step of the box above it. Check a box only when its evidence exists, a file, a log line, a screenshot, a test run, or a SHA. The body is a how-to. The appendices explain and record.

The program runs `pstack/skills/poteto-mode/playbooks/autopilot-stack.md`. The root agent builds one linear stack of PRs from P1 to P8 and the operator reviews and lands it. The one exception is the build step of P7 and P8, which runs in parallel; their merge order stays linear. P0 never merges. Its branch and findings feed Appendix A and the operator's go or no-go call.

Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

Requirement ids such as `UX-7` and `A11Y-3` point into `docs/plans/ux-redesign.md`.

A lane marked **Operator lane** needs a person at the Mac, because it changes System Settings, permissions, or hardware, or runs with VoiceOver on. The root prepares the exact steps and the evidence to capture, and the operator runs it. Every other lane is agent-runnable.

pstack is a Claude Code plugin, not part of this repo. Paths that start with `pstack/` mean `~/.claude/plugins/cache/cursor-plugins/pstack/<version>/`. Validate this file with `node <pstack>/skills/poteto-mode/scripts/check-plan.mjs docs/plans/gpui-migration.md`.

## Program checklist

### Arm the program

- [ ] State the protocol and this plan to the operator, then stop. Start execution only on the operator's explicit go.
- [ ] Read these from trunk at program start. Re-read them at every tick.
  - [ ] `git show origin/main:docs/plans/gpui-migration.md`
  - [ ] `git show origin/main:docs/plans/ux-redesign.md`
  - [ ] `git show origin/main:AGENTS.md`
  - [ ] The pstack copies of `poteto-mode/playbooks/autopilot-stack.md`, `poteto-mode/playbooks/autopilot-full.md`, `poteto-mode/playbooks/opening-a-pr.md`, and `swarm/SKILL.md`. pstack lives outside the repo, so `git show origin/main:` applies only to the repo files above.
- [ ] Tag P1's first commit as `baseline-tauri` once it exists. That commit adds only log lines and the `DORY_DATA_DIR` override to the Tauri app, so it behaves like `main`. Every "trunk" measurement in this plan runs on that tag, so the baseline does not move while the stack is open.
- [ ] Back up the real data folder with `ditto ~/Library/Application\ Support/com.dory.app ~/dory-backup-$(date +%Y%m%d)` before any lane runs. No lane may use the real data folder.
- [ ] On the operator's go, arm the audit tick as `/loop 1h` with the tick prompt below. Never leave the cadence to memory.
- [ ] Use this tick prompt, verbatim. "Re-read the execution playbook from trunk. Audit the operation against it and fix drift in this tick. Probe every active lane and judge progress by side effects only. Stand down a stuck lane and dispatch its replacement now. Then post a short status message to the operator in chat only when the audit found a tracked change that no earlier status message reported, such as a PR opened, a code-ready head, a round launched or closed, a verdict, a merge, a stuck agent and the action taken, a blocker added or cleared, or a decision only the operator can make. Name every such change and nothing else. Do not repeat a table, the merged list, or an unchanged blocker. If the audit found none, end the turn with no reply text. Either way, log this tick's row in your decision trail. The row names the items reported, or none."
- [ ] On the operator's hold or stand-down, send every owner a zero-writes order at once.

### Spawn owners

- [ ] Spawn one owner per PR with the full lifecycle the execution playbook names. Owners use `pstack:poteto-agent` on the `feature, refactoring` model from `~/.claude/rules/pstack-models.md`. P2 and P7 use the `hardest tasks` model.
- [ ] Follow this dependency graph. Each owner branches from its parent's branch, because the stack is linear.
  - [ ] P0 and P1 are independent and first. Both branch from `main`.
  - [ ] P2 after P1.
  - [ ] P3 after P2. P3 needs only P1's code, but the stack is linear.
  - [x] Before P4, the operator sends the Claude Design request in `docs/plans/claude-design-brief.md`, picks a direction, and saves it to `docs/design/`. Evidence is `docs/design/handoff-direction-c.md` and `docs/design/canvas-direction-c.html`, direction C, 2026-11-15.
  - [ ] P4 after P3, after the operator records a go on P0, and after `docs/design/` holds the chosen direction.
  - [ ] P5 after P4.
  - [ ] P6 after P5.
  - [ ] P6b after P6.
  - [ ] P7 and P8 after P6b, built in parallel. P8 branches from P6b, not from P7, and never edits a file under `apps/dory-gpui/src/`.
  - [ ] P8 merges after P7. Before its live and perf lanes and its review gate, P8 rebases onto P7's branch, so every P8 lane runs on a head with P7's fixes. A P8 lane run before that rebase does not count.
  - [ ] Both P7 and P8 edit the root `Cargo.toml`. P7 owns `[profile.release]` and P8 owns `[workspace] members`, so the rebase is mechanical.
- [ ] Hold the file boundaries. Each PR's **Files** list is its boundary. A PR that needs a file outside its list stops and asks the root, which either adds a small PR below it in the stack or amends this plan with a logged decision.
- [ ] Hold the review gate. P4, P5, P6, P6b, P7, and P8 change an interaction. They wait for the operator's review in chat with screenshots and a video before merge.

### PR mechanics, for every PR

- [ ] Resolve the forge once. Default to `gh`; if `command -v origin` succeeds and Origin can resolve the repository, use `origin pr` for every PR operation. Record any fallback to `gh`. Never require `gt`.
- [ ] Open the PR ready, never draft, per **Opening a PR**. Use the run's built-in PR tool when it has one, else `origin pr create --status open --base <base-branch>` or `gh pr create --base <base-branch>` according to the resolved forge. A stack child targets its parent branch.
- [ ] Run `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` once before the PR-facing push. Before P8 also run `pnpm vite:build`. Push with hooks on.
- [ ] Run `/deslop` before each commit and `/no-comments` before review. Both come from `cursor-team-kit`. If the plugin is missing, write `Skipped in Claude Code` with the reason in the PR body.
- [ ] Triage every Bugbot and security-reviewer comment per `pstack/skills/poteto-mode/references/bugbot-triage.md`.
- [ ] Rebase onto current trunk before the code-ready report and babysit. Keep that merge base in fix rounds. Rebase again only at merge prep, on a `git merge-tree` conflict with trunk, or on a CI failure that comes from a change on trunk.
- [ ] Name branches `<type>/<slug>` per `AGENTS.md`, for example `refactor/dory-core` for P1 and `feat/gpui-shell` for P4. No AI trailer in commit messages.
- [ ] Append a row to `docs/plans/decisions.tsv` for every real decision with the pstack `show-me-your-work/scripts/log.sh` helper. Mark any external fact you did not re-check this session as unverified in the row.

### Verdict and merge, for every PR

- [ ] A full swarm runs once, at the code-ready head. Another full swarm runs only for new behavior, a redesign, or a large new diff. A rebase, a merge-conflict resolution, or a minor fix gets a single verifier at the new head. Run each full swarm per `pstack/skills/swarm/SKILL.md`. One gates lane. The ten live lanes from the PR's **Verify, live** block. The perf lane from its **Verify, perf** block. Two or more audit lanes, each with its own focus, that read the diff and the receipts and distrust the PR body. The root audits the receipts in the merge-ready report before the verdict.
- [ ] Clean only when every lane is `PASS`. An Operator lane that has not run is `NOT VERIFIED`, never `PASS`. Findings go back to the owner, including a defect that a lane filed as a note. A new head gets a fresh verdict, except for results that stay valid under the patch-id rule in `playbooks/shipping.md`.
- [ ] The root appends each clean PR to the stack with its base set to its parent's branch. The operator lands the stack bottom-up. A rebase that leaves `git patch-id` unchanged keeps the verdict. A changed patch-id needs a single verifier at the new head.

### Boot recipe, for every live lane

Live lanes run on the operator's Mac, an Apple Silicon machine on macOS 26 or later. Cloud VMs cannot grant microphone or System Audio Recording permission, so no lane runs in the cloud. Lanes that record audio share the audio hardware and the TCC state, so the root runs them one at a time. Lanes that install, uninstall, reboot, or upgrade run in a second macOS user account named `dorytest`, never in the operator's account.

- [ ] `git fetch origin <head-branch> && git checkout <head SHA>`.
- [ ] Run `scripts/fixtures.sh` once per machine. It downloads the AMI meeting clips into `fixtures/` (git-ignored) and checks their SHA-256.
- [ ] Seed a throwaway data folder with `scripts/seed-lane.sh <n> <count>`, which writes `<count>` recordings to `/tmp/dory-lane-<n>`. Export `DORY_DATA_DIR=/tmp/dory-lane-<n>` before launching. `baseline-tauri`, `dory-cli`, and the GPUI app all honor it.
- [ ] Start the trunk Tauri app with `pnpm prebuild:sona && pnpm install && pnpm dev`. Run the CLI with `cargo run --release -p dory-cli -- <command>`, bundled through `scripts/bundle-cli.sh` for lanes that need permissions. From P4 on, start the native app with `cargo run --release -p dory-gpui` and wait for the `first-frame` log line.
- [ ] Deliver input only through the Claude Code computer-use tools (`app_click`, `app_key`, `app_type`, `app_screenshot`, `app_ax_find`), because no `control-ui` skill drives a native macOS window. Use `screencapture -x` for system windows such as the VoiceOver caption panel.
- [ ] To test system-audio capture, play audio with `afplay` and record with `--layout system`, so speaker sound reaching the mic cannot fake a pass. To test the mic, record with `--layout mic` while `say` speaks.
- [ ] Judge loudness only with numbers from `sox <file> -n stat`, never by ear. To measure app playback, record with `dory-cli capture --layout system` while the app plays.
- [ ] Read-only diagnostics are `log stream --predicate 'subsystem == "com.dory.app"'`, `scripts/rss-tree.sh <pid>` (sums RSS over the process and its children, which counts WKWebView helpers on trunk), and the files in the lane's data folder.
- [ ] Save every screenshot to `/tmp/swarm-<pr-id>/worker-<n>/<slug>.png` and return the paths with the report.

## Prove GPUI can carry an accessible Dory window (P0)

**Depends on.** None.

**Files.**

- [ ] Create `spikes/gpui-a11y/` on branch `spike/gpui-a11y`. The branch never merges.

**Build.**

- [ ] Build one GPUI Kit window with `gpui-component` 0.9.x and its pinned `gpui-pre` snapshot. It has a sidebar list of three recordings, a toolbar record button, a search input, a player slider, a Summary tab with Markdown, and a toggle. Give each element an accessibility id, role, and label through the GPUI AccessKit API.
- [ ] Add an `NSStatusItem` menu-bar extra through `objc2-app-kit` next to the GPUI event loop.
- [ ] Post one VoiceOver announcement through `NSAccessibilityAnnouncementRequestedNotification` when the record button is pressed.
- [ ] Write `spikes/gpui-a11y/RESULT.md` with a go or no-go verdict against the predicate in **You see**.

**You see.**

- [ ] With VoiceOver on, VO-Right moves through all seven elements and VoiceOver speaks each element's label and role.
- [ ] Tab moves focus through every control and a focus ring is visible on each.
- [ ] The menu-bar extra and the GPUI window run in one process and both respond.
- [ ] The go predicate is all three boxes passing with no crash in 30 minutes of VoiceOver use. A no-go sends the operator to Appendix B. Capture is not part of P0, because P2 builds and proves it on the real adapter.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `spikes/gpui-a11y/tests/a11y_tree.rs` uses `TestAppContext` to assert that each of the seven elements has a role and a non-empty label. Run `cargo test -p gpui-a11y-spike`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Trunk has no GPUI window, so record that and gate the end state. Open the spike and run `app_ax_find` for the button "Start recording". Save `ax-button.png`. Pass when the AX tree returns a button with that exact label.
- [ ] Lane 2. Run `app_ax_find` for each of the seven elements. Save `ax-all.png`. Pass when all seven return with the expected role and label.
- [ ] Lane 3. Operator lane. Turn on VoiceOver with Command-F5 and step with VO-Right through the window. Save `vo-caption.png` with `screencapture -x`. Pass when each caption names the element and its role.
- [ ] Lane 4. Press Tab nine times from launch. Save `tab-ring-<n>.png` for each press. Pass when every control receives a visible focus ring in visual order.
- [ ] Lane 5. Focus the search input and type "design". Save `input.png`. Pass when `app_ax_find` reports the field's value as "design".
- [ ] Lane 6. Focus the slider and press Right Arrow three times. Save `slider.png`. Pass when the AX value increases each time.
- [ ] Lane 7. Click the menu-bar extra, then the window. Save `status-item.png` with `screencapture -x`. Pass when both respond in the same process.
- [ ] Lane 8. Operator lane. With VoiceOver on, click record. Save `vo-announce.png` with `screencapture -x`. Pass when the caption shows "Recording started".
- [ ] Lane 9. Operator lane. Run the window for 30 minutes with VoiceOver on, switching tabs and scrolling every minute. Save `soak.png`. Pass when the process is alive at the end and the log has no panic.
- [ ] Lane 10. Operator lane. Turn on **Reduce motion** and **Increase contrast**, relaunch, and screenshot. Save `prefs.png`. Pass when the result is recorded either way, because this lane sizes the A11Y-5 and A11Y-6 work in P7.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Idle RSS summed over the process tree, and Apple's hitch time ratio (milliseconds of hitch per second of scrolling), for the Tauri app on `baseline-tauri` and the spike at head.
- [ ] Probe. Launch each app, wait 10 seconds, and run `scripts/rss-tree.sh`. Then scroll for 10 seconds under `xctrace record --template 'Animation Hitches'`. Alternate trunk and head three times.
- [ ] Baseline. Record the trunk Tauri idle RSS and hitch time ratio first.
- [ ] Rule. The spike fails if its idle RSS is above the trunk value or its hitch time ratio is above 5 ms per second. The spike has less UI than trunk, so this is a sanity bound, not a ratio claim between equal apps.

**Review gate.** None. P0 is not review-gated.

**Merge.**

- [ ] P0 never merges. Push the branch, link `RESULT.md` and the lane screenshots in Appendix A, and record the operator's go or no-go in `docs/plans/decisions.tsv`.

## Build the hexagonal core, store, and CLI (P1)

**Depends on.** None.

**Files.**

- [ ] Create the root `Cargo.toml` workspace with `src-tauri` excluded, so the Tauri app keeps building on its own until P8.
- [ ] Create `crates/dory-core/**` per `docs/architecture/hexagonal.md`. The pure layer (domain, `Job::on`, `recorder::step`, model `fit` and `resolve`, `Catalog`, port traits, frame ring, fakes) exists on branch `feat/gpui-rewrite`. P1 adds the actors and the `Dory` facade.
- [ ] Create `crates/dory-store/**` with `SqliteStore`, `FsTakes`, and the legacy importer.
- [ ] Create `crates/dory/**`, the composition root.
- [ ] Create `apps/dory-cli/**` with `import`, `list`, and `show`.
- [ ] Edit `src-tauri/src/lib.rs` and `src/main.tsx` in one instrumentation-only commit (see **Build**). Nothing else in the Tauri app changes.
- [ ] Create `.github/workflows/ci.yml`, `scripts/fixtures.sh`, `scripts/seed-lane.sh`, `scripts/rss-tree.sh`, `scripts/bench-pipeline.sh`, and `fixtures.sha256`.

**Build.**

- [ ] Make the first commit instrumentation only. In the Tauri app, read `DORY_DATA_DIR` in place of `app_data_dir()` when it is set, and log `first-frame`, `record-requested`, `first-sample`, `transcribing`, and `summarizing` with timestamps under the `com.dory.app` subsystem. The root tags this commit `baseline-tauri`.
- [ ] Add the recorder actor, the take writer thread, the pipeline worker skeleton, the event hub, and the `Dory` facade to `dory-core`. Commands are synchronous. Probes return futures that await `async-channel` replies.
- [ ] `SqliteStore` implements `Store` with one writer connection and one transaction per `apply`. Its DTOs are private and versioned. `FsTakes` writes a crash-tolerant WAV, seals to FLAC, and cuts 16 kHz mono clips.
- [ ] The legacy importer reads each `metadata/<id>.json` once, keyed by content hash, and points at the old audio file in place. It parses `[Speaker N]` lines into untimed segments.
- [ ] `dory::open(Config)` wires the store, the takes, and the fakes for every port that has no real adapter yet.
- [ ] Run every bench script against `baseline-tauri` and commit the numbers to `docs/plans/baselines.md`.

**You see.**

- [ ] `dory-cli import ~/dory-backup-<date>` then `dory-cli list` prints every legacy recording with its title, duration, and status, and the Tauri app still runs on the original folder.
- [ ] `cargo test --workspace` passes, and CI runs fmt, clippy, and tests on every push.
- [ ] `docs/plans/baselines.md` holds every trunk number that later perf blocks compare against.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-store/tests/store_contract.rs` runs the core's store contract suite against both `MemStore` and `SqliteStore`, including atomicity of a stage output with its job event. Run `cargo test -p dory-store`.
- [ ] `crates/dory-store/tests/legacy.rs` imports a trunk-written folder fixture twice and checks the second import changes nothing. Run `cargo test -p dory-store --test legacy`.
- [ ] `crates/dory-core/tests/facade.rs` drives a fake record-to-summary run through `Dory` with fakes for every port. Run `cargo test -p dory-core --all-features`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Seed a lane folder with 5 recordings in the `baseline-tauri` app, then run `dory-cli import` on a copy. Save `import.png`. Pass when `dory-cli list` shows all 5 with the same titles, durations, and summaries as the Tauri app.
- [ ] Lane 2. Run `dory-cli import` twice on the same folder. Save `import-twice.png`. Pass when the second run reports 0 new recordings.
- [ ] Lane 3. Run `dory-cli show <id>` on a legacy recording. Save `show.png`. Pass when the transcript prints as speaker turns without timestamps and the summary prints as Markdown.
- [ ] Lane 4. Open the Tauri app on the original folder after the import. Save `tauri-still.png`. Pass when every recording plays and nothing in the folder changed, checked by `shasum -a 256` before and after.
- [ ] Lane 5. `kill -9` the CLI in the middle of an import of 200 recordings, then run it again. Save `import-crash.png`. Pass when the final count is 200 with no duplicates.
- [ ] Lane 6. Corrupt one `metadata/<id>.json` in the source folder. Save `import-corrupt.png`. Pass when that file is reported and skipped and the rest import.
- [ ] Lane 7. Run `dory-cli fake-record --seconds 30` through `FakeCapture`. Save `fake-record.png`. Pass when the recording appears with a 30-second duration and a sealed FLAC.
- [ ] Lane 8. Run `dory-cli fake-record --seconds 30 --pause-at 10 --resume-at 20`. Save `fake-pause.png`. Pass when the duration is 20 seconds.
- [ ] Lane 9. Run a fake pipeline with `dory-cli process <id> --fake-engines`. Save `fake-process.png`. Pass when the job ends `Succeeded` with a transcript and a summary.
- [ ] Lane 10. Run `scripts/bench-pipeline.sh` on `baseline-tauri`. Save `baseline.png`. Pass when `docs/plans/baselines.md` has every metric later perf blocks name.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Time to import 200 legacy recordings, and `dory-cli list` latency on 500 recordings.
- [ ] Probe. `scripts/seed-lane.sh 1 500` then `time dory-cli import` and `time dory-cli list`, five runs each.
- [ ] Baseline. Trunk has no import, so record that and gate the absolute budget. Record the Tauri app's `list_recordings` time on the same folder from the `baseline-tauri` log for comparison.
- [ ] Rule. Fail when importing 200 recordings takes over 5 seconds or listing 500 takes over 100 ms or the trunk `list_recordings` time.

**Review gate.** None. P1 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P1 to the stack on `main` and the operator lands it.

## Capture natively on macOS 26 (P2)

**Depends on.** P1.

**Files.**

- [ ] Create `crates/dory-macos/**` with `CoreAudioCapture`. A first version exists on branch `feat/gpui-rewrite`.
- [ ] Create `crates/dory-macos/Info.plist` for the CLI bundle, with `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription`.
- [ ] Edit `crates/dory/src/lib.rs` to use `CoreAudioCapture` on macOS 26 and later.
- [ ] Edit `apps/dory-cli/**` to add `record` and the `capture` harness.
- [ ] Create `scripts/bench-capture.sh` and `scripts/bundle-cli.sh`, which wraps `dory-cli` in a signed `.app` so permissions attach to Dory instead of the terminal.

**Build.**

- [ ] `CoreAudioCapture` creates a global stereo process tap that excludes Dory, puts it and the chosen mic in one private aggregate device with the default output as clock source, and pushes `[mic, system]` frames into the core's `FrameSink` from one IOProc with no allocation.
- [ ] It resamples inside the adapter when a device runs at another rate, and never changes a device's rate or the user's default devices.
- [ ] It reports `SystemAudioSilent` after 3 seconds of digital silence on the tap while output runs, and rebuilds the tap and aggregate on that issue and on output-device changes.
- [ ] The recorder actor in `dory-core` runs `recorder::step` and executes its effects. Pause and grace stop the device. Resume starts it with a fresh sink into the same take.
- [ ] `dory-cli record` records until Enter, supports `p` for pause and `u` for undo within 5 seconds, and prints the recording id.

**You see.**

- [ ] `dory-cli record` in the bundled CLI records the mic and the system audio, prints levels once per second, and leaves the default input and output devices unchanged.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-macos` unit tests cover channel mapping, downmix, resampling, and the silence detector without hardware. `crates/dory-core/tests/recorder_actor.rs` drives the actor with `FakeCapture` through pause, undo, cap, and capture failure. Run `cargo test -p dory-macos -p dory-core --all-features`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Record 60 seconds of the AMI clip in the `baseline-tauri` app inside `dorytest`, and with `dory-cli record` at head. Save `levels.png` with both files' `sox stat` output. Trunk has one mixed track, so compare the trunk mix with the sum of the head channels. Pass when the head sum RMS is within 3 dB of the trunk mix.
- [ ] Lane 2. Record with `--layout system` while Safari, a Zoom test call, and Music play in turn. Save `apps.png`. Pass when the system channel RMS is above -30 dBFS in each app's window.
- [ ] Lane 3. Operator lane. Record with AirPods as input and output. Save `airpods.png`. Pass when both channels are above their thresholds and the log shows the adapter resampling the 24 kHz headset mic.
- [ ] Lane 4. Operator lane. Revoke System Audio Recording in System Settings and record. Save `denied.png`. Pass when `SystemAudioSilent` is reported within 3 seconds.
- [ ] Lane 5. Operator lane. Revoke Microphone and record. Save `mic-denied.png`. Pass when the record command reports mic denied and still records system audio.
- [ ] Lane 6. Operator lane. Switch the output device mid-recording. Save `output-switch.png`. Pass when the log shows a `Rebuilt` issue and the system channel continues after the gap.
- [ ] Lane 7. Pause for 10 seconds and resume while `afplay` loops. Save `pause.png`. Pass when the duration excludes the pause and the menu-bar recording indicator was off during it.
- [ ] Lane 8. `kill -9` the recorder after 47 seconds, then start the CLI again. Save `crash.png`. Pass when the orphaned take is recovered as a recording of at least 40 seconds.
- [ ] Lane 9. Record 2 hours while a 1 kHz click plays through the speakers every 10 minutes. Save `cap.png`. Pass when the warning fires at 1:55:00, the recorder stops at 2:00:00, and the click offset between channels stays under 20 ms.
- [ ] Lane 10. Read `system_profiler SPAudioDataType` before, during, and after a recording. Save `devices.png`. Pass when the default devices never change and no Dory device remains after stop.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. CPU percent and RSS of the recording process tree, averaged over 10 minutes, and the stored FLAC size per hour.
- [ ] Probe. `scripts/bench-capture.sh` samples `ps -o pcpu=,rss=` every second over the process tree. Run the `baseline-tauri` app and `dory-cli record` interleaved, twice each.
- [ ] Baseline. Record the trunk average CPU and RSS in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when head CPU exceeds 3% of one core or exceeds trunk. Fail when the FLAC exceeds 400 MB per hour.

**Review gate.** None. P2 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P2 to the stack on P1's branch.

## Transcribe and summarize through swappable engines (P3)

**Depends on.** P2.

**Files.**

- [ ] Create `crates/dory-engines/**` with the `Sherpa` speech provider and the `Ollama` summary provider. Sona's repo is gone (404 on 2026-11-16), so speech runs in process through sherpa-onnx instead of a sidecar.
- [ ] Edit `crates/dory-core/src/pipeline/**` for the worker, windowing, checkpoints, merge, and summary policy.
- [ ] Add a `Playback` port to `crates/dory-core/src/ports/` and an `AVAudioPlayer` adapter in `crates/dory-macos/src/playback.rs`.
- [ ] Edit `crates/dory/src/lib.rs` to register the providers.
- [ ] Edit `apps/dory-cli/**` to add `process`, `models`, and `play`.

**Build.**

- [ ] `Sherpa` implements `ModelProvider<Stt>` with rows for Parakeet TDT 0.6B v3 (recommended), Qwen3-ASR 0.6B, and Whisper turbo, each with pros and cons for the UX-28 explainer. It provisions a pinned archive on request and frees the recognizer when the engine is dropped. A measured Tagalog and English run decides which rows stay.
- [ ] `Ollama` implements `ModelProvider<Llm>` with `gemma4:e4b` and `gemma4:12b` rows. It sets `num_ctx` explicitly and reports it as `context_tokens`.
- [ ] The pipeline worker resolves models with `resolve`, plans clips from the layout, cuts windows at silence, skips silent windows, checkpoints each window, merges by time, drops mic bleed, and commits the transcript with its job event in one `apply`.
- [ ] Summaries use one pass or map-reduce from `context_tokens`, in the meeting's main language.
- [ ] `Playback` covers play, pause, seek, and a position stream. The system channel gets the 1.85 gain for two-channel takes.

**You see.**

- [ ] `dory-cli process <id>` transcribes a recording made by `dory-cli record` with "You" on mic segments, then summarizes it. `dory-cli models` lists every row with its availability. Switching the speech model is `dory-cli models choose stt sona/large-v3-turbo` and changes nothing else.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/tests/pipeline.rs` covers window planning, silent-window skipping, checkpoint resume after a simulated crash, bleed dedupe, merge order, map-reduce sizing, and `Blocked` for a missing model, all with fakes. `crates/dory-engines/tests/` covers Sherpa's provisioning and model configs and Ollama's request shape against recorded fixtures. Run `cargo test --workspace`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Transcribe and summarize the 10-minute AMI fixture in the `baseline-tauri` app and with `dory-cli process` at head, using the same whisper model. Save `e2e.png`. Pass when the head WER against the AMI reference is no worse than trunk's.
- [ ] Lane 2. Process a two-channel recording. Save `you.png`. Pass when every mic segment is labelled "You" and remote segments are not.
- [ ] Lane 3. `kill -9` the CLI halfway through a 2-hour transcription, then rerun. Save `resume.png`. Pass when the log shows the run resuming at the last checkpoint instead of the start.
- [ ] Lane 4. Stop Ollama and process. Save `blocked.png`. Pass when the job is `Blocked` with an "Ollama is not running" fix and finishes after Ollama starts.
- [ ] Lane 5. Remove the selected whisper model file and process. Save `needs-provision.png`. Pass when the job is `Blocked(NeedsProvision)` and nothing downloads on its own.
- [ ] Lane 6. Switch the speech model with `dory-cli models choose` and reprocess. Save `switch.png`. Pass when the transcript's `produced_by` names the new model and no other setting changed.
- [ ] Lane 7. Process a 2-hour recording with `gemma4:e4b` and with a forced 4096-token context. Save `map-reduce.png`. Pass when both produce a summary and the log shows map-reduce only for the small context.
- [ ] Lane 8. Process a French recording with language `Auto`. Save `french.png`. Pass when the transcript and summary are in French.
- [ ] Lane 9. Run `dory-cli play <id> --seek 5400`. Save `play.png`. Pass when capture of the system output shows audio starting within 200 ms at that position.
- [ ] Lane 10. Cancel a running job. Save `cancel.png`. Pass when transcription stops within 1 second and the job is `Cancelled`.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Transcription wall time for the 10-minute AMI fixture with the same whisper model, from job start to transcript commit.
- [ ] Probe. `scripts/bench-pipeline.sh` at `baseline-tauri` and head, interleaved, five runs each.
- [ ] Baseline. Record the trunk median and spread in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when the head median exceeds the trunk median by more than 5% and by more than the trunk spread. Two-channel takes are measured separately against an absolute budget of 1.6 times the one-channel time.

**Review gate.** None. P3 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P3 to the stack on P2's branch.

## Build the GPUI shell with the library view (P4)

**Depends on.** P3, the operator's go on P0, and the chosen design in `docs/design/`.

**Files.**

- [ ] Create `apps/dory-gpui/src/main.rs`, `app.rs`, `theme.rs`, `menus.rs`, `sidebar.rs`, `detail.rs`, `player_view.rs`, `transcript_view.rs`, `summary_view.rs`, `export.rs`, and `prefs.rs`.
- [ ] Edit the root `Cargo.toml` to add `dory-gpui` and to set `panic = "unwind"` in `[profile.release]` until P7 proves the accessibility tree is stable.

**Build.**

- [ ] Name the data shapes first. `AppState` holds `recordings: Vec<RecordingRow>`, `selection: Selection`, `search: SearchQuery`, and `prefs: Prefs`. One GPUI `Entity` owns it and subscribes to the `CoreEvent` channel. Views read it and never call the store directly.
- [ ] Theme tokens come from section 2 of `docs/design/handoff-direction-c.md`. `theme.rs` has `light()` and `dark()` now and a high-contrast set in P7. Derive `accent`, `accent_text`, and `on_accent` from `NSColor.controlAccentColor` with the darkening rule in A11Y-8.
- [ ] Build the start screen for no recordings and for no selection (UX-6), and the direction C layout. It has an inset sidebar panel, a centered 720 pt column, the player card that docks to a slim bar when it scrolls away, and a sticky tab row.
- [ ] Build the sidebar with date groups, search at its top, and the status filter (UX-1), the detail pane with tabs (UX-2 to UX-4), transcript turns with click-to-seek and speaker rename stored in `Transcript.speakers` (UX-3), the toolbar (UX-5), empty states (UX-6), and the sidebar material (UX-25).
- [ ] Build the menu bar for the commands that exist in P4 (UX-21). Later PRs add their own menu items.
- [ ] Add export to Markdown, text, and SRT through `prompt_for_new_path` (UX-23). SRT uses segment times and is disabled for legacy transcripts with no times. Add delete to Trash with undo (UX-22). Restore window state (UX-24).
- [ ] Give every element an accessibility id, role, and label as it is built. P7 audits, it does not retrofit.

**You see.**

- [ ] `cargo run -p dory-gpui` opens a window that lists every recording in the data folder, plays it, shows its transcript and summary, and exports it.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `apps/dory-gpui/tests/library.rs` uses `TestAppContext` with the handoff's sample recordings (section 6) to cover grouping by date, search over transcript text, filter, selection after delete, undo, and speaker rename. Run `cargo test -p dory-gpui`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Open copies of the same 20-recording lane folder in the `baseline-tauri` app inside `dorytest` and in the head GPUI app. Save `library-trunk.png` and `library-head.png`. Pass when both list the same 20 titles and the head groups them by date.
- [ ] Lane 2. Search for a word that appears in only one transcript. Save `search.png`. Pass when only that recording remains.
- [ ] Lane 3. Select a recording and switch tabs with Command-1 and Command-2. Save `tabs.png`. Pass when Summary renders Markdown headings and lists and Transcript shows speaker turns.
- [ ] Lane 4. Click a transcript timestamp on a recording processed at head. Save `seek-from-transcript.png`. Pass when the player position jumps to that time.
- [ ] Lane 5. Export a recording as Markdown, text, and SRT. Save `export.png`. Pass when the three files exist and their text matches the stored transcript and summary.
- [ ] Lane 6. Delete a recording, then press Command-Z. Save `undo.png`. Pass when the recording returns with all its files.
- [ ] Lane 7. Switch the system between light and dark. Save `light.png` and `dark.png`. Pass when the app follows within one second and matches `docs/design/`.
- [ ] Lane 8. Resize the window to 800 by 600, select a recording, and relaunch. Save `restore.png`. Pass when size, sidebar width, and selection persist.
- [ ] Lane 9. Rename "Speaker 1" to "Sam" and relaunch. Save `speaker-rename.png`. Pass when every turn of that speaker reads "Sam".
- [ ] Lane 10. Run `app_ax_find` for the sidebar rows, the toolbar button, the tabs, and the player. Save `ax.png`. Pass when each has a role and label.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Cold start to the `first-frame` log line, idle RSS over the process tree, and hitch time ratio while scrolling 500 recordings.
- [ ] Probe. `scripts/seed-lane.sh 4 500` creates 500 recordings. Launch trunk Tauri and head GPUI alternately three times. Measure start from the log, RSS with `scripts/rss-tree.sh`, and hitches with `xctrace`.
- [ ] Baseline. Record the trunk numbers in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when head cold start exceeds 500 ms or trunk, when head RSS exceeds trunk, or when the hitch time ratio exceeds 5 ms per second.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 3, and 7 screenshots into `docs/design/review/p4-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of browsing, searching, playing, and exporting. Save it as `docs/design/review/p4-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P4 to the stack on P3's branch.

## Record and process from the GPUI app (P5)

**Depends on.** P4.

**Files.**

- [ ] Create `apps/dory-gpui/src/record_control.rs`, `status_item.rs`, `queue_popover.rs`, and `notify.rs`.
- [ ] Edit `apps/dory-gpui/src/app.rs`, `menus.rs`, `sidebar.rs`, and `detail.rs`.

**Build.**

- [ ] The toolbar record control drives `dory.recorder()` and shows the red elapsed-time pill with the level meter, Pause, and Stop (UX-7, UX-8). Add the **Recording** menu.
- [ ] Stop moves the recorder to `Grace` and shows the Undo toast for 5 seconds (UX-9). The recording joins the queue only when the grace period ends, or the finish card appears when automatic processing is off (UX-11). The cap and warning post notifications (UX-10).
- [ ] Add the live transcript (UX-27) behind its setting, only after the speech plan's streaming step picks a recognizer and its 60-minute CPU and energy check passes. Until then the detail pane shows the recording state without draft lines.
- [ ] Enqueue on finalize when **Process recordings automatically** is on (UX-11). Progress, estimates, the per-recording live transcript, and failure actions follow UX-12 and UX-13. A missing summary model shows **Set up summaries**, which opens a sheet with the `ollama pull` command until P6 adds the one-click pull.
- [ ] Post a system notification when a job finishes and the window is not key (UX-14). Add the **Processing Queue** popover with **Move to top** and **Cancel** (UX-15).
- [ ] Add the menu-bar extra with `objc2-app-kit` `NSStatusItem`, because GPUI has no tray API. It starts and stops recording and shows elapsed time.

**You see.**

- [ ] Command-Shift-R starts a recording, Command-Period stops it, and after 5 seconds the recording processes to a summary with no other input.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `apps/dory-gpui/tests/record_flow.rs` uses a fake recorder and fake core to cover start, pause, stop, undo within 5 seconds, no undo after 5 seconds, no enqueue before the grace period ends, cap reached, and auto-enqueue on and off. Run `cargo test -p dory-gpui --test record_flow`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Record the 60-second AMI clip and wait for a summary, in the `baseline-tauri` app inside `dorytest` and in the head GPUI app. Save `flow-trunk.png` and `flow-head.png`. Pass when both end with a summary and head needs no click after Stop.
- [ ] Lane 2. Start from the menu-bar extra. Save `status-item.png` with `screencapture -x`. Pass when the extra shows a red dot and the elapsed time.
- [ ] Lane 3. Pause and resume with Command-Shift-P. Save `pause.png`. Pass when the pill shows paused and the saved duration excludes the pause.
- [ ] Lane 4. Stop, then click Undo within 5 seconds. Save `stop-undo.png`. Pass when recording continues into the same file and the queue stays empty.
- [ ] Lane 5. Watch the detail pane during transcription. Save `progress.png`. Pass when it shows the stage, a percentage, and streaming lines for that recording only.
- [ ] Lane 6. Stop Ollama and record. Save `fail-action.png`. Pass when the row shows one sentence and a **Set up summaries** button that opens the sheet.
- [ ] Lane 7. Send the window to the background during processing. Save `notification.png` with `screencapture -x`. Pass when a notification appears and clicking it selects the recording.
- [ ] Lane 8. Turn auto-processing off and record. Save `manual.png`. Pass when the recording stays idle and shows **Transcribe**.
- [ ] Lane 9. Press the volume keys during a recording. Save `volume.png` with `screencapture -x`. Pass when the volume HUD appears, which proves the output device was not switched.
- [ ] Lane 10. Open the Processing Queue with three jobs and use **Move to top** on the last. Save `queue.png`. Pass when the moved job runs next.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Time from the record shortcut to the first captured sample, and app CPU while recording with the window visible.
- [ ] Probe. Read the `record-requested` and `first-sample` log lines, which `baseline-tauri` also emits. Sample CPU over the process tree every second for 5 minutes. Run trunk and head interleaved three times.
- [ ] Baseline. Record the trunk numbers in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when head start latency exceeds 300 ms or trunk, or when head CPU exceeds trunk.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 2, and 5 screenshots into `docs/design/review/p5-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of record, pause, stop, undo, and the summary arriving. Save it as `docs/design/review/p5-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P5 to the stack on P4's branch.

## Onboard and configure in native windows (P6)

**Depends on.** P5.

**Files.**

- [ ] Create `apps/dory-gpui/src/onboarding.rs`, `settings_window.rs`, and `models_view.rs`.
- [ ] Create `crates/dory-core/src/models.rs` and `crates/dory-core/tests/models.rs`.
- [ ] Edit `apps/dory-gpui/src/menus.rs` and `detail.rs`.

**Build.**

- [ ] Name the data shapes first. `ModelCatalog` is a table of `ModelEntry { id, kind: Speech | Summary, size_bytes, url, sha256, runtime }`. The speech model plan fills in its rows. `OnboardingStep` is an enum `Welcome`, `Permissions`, `Models`, `Done`, persisted so a quit resumes at the same step.
- [ ] Build the three-step first run (UX-16). Recording works before models finish (UX-17). Models download into the data folder with progress, resume, and checksum (UX-18).
- [ ] Build the Settings window with five tabs (UX-19, UX-20), including the Language setting with **Automatic** as the default (UX-26). **Add model file...** uses `prompt_for_paths`.
- [ ] Show the detected language in the detail pane, and add **Re-transcribe in...** to pick a language for one recording (UX-26).
- [ ] Detect Ollama and the summary model. **Set up summaries** now runs `ollama pull` with progress.

**You see.**

- [ ] In `dorytest`, a first launch reaches a working first recording with no Terminal and no file paths.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/tests/models.rs` covers download resume, checksum mismatch, and catalog parsing. `apps/dory-gpui/tests/onboarding.rs` covers step persistence and the permission state table. Run `cargo test --workspace`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. In fresh `dorytest` profiles, count user actions from first launch to the first finished transcript in `baseline-tauri` and head, with models pre-downloaded. Save `onboard-trunk.png` and `onboard-head.png`. Pass when head needs fewer actions and no Terminal command.
- [ ] Lane 2. Operator lane. Deny the microphone in the prompt. Save `mic-denied.png`. Pass when the step shows the denial and an **Open System Settings** button.
- [ ] Lane 3. Operator lane. Grant System Audio Recording in System Settings and return. Save `tap-granted.png`. Pass when the status updates without a relaunch.
- [ ] Lane 4. Quit during the model download and relaunch. Save `resume.png`. Pass when the download resumes from its byte offset at the same step.
- [ ] Lane 5. Corrupt the downloaded model file. Save `checksum.png`. Pass when the app rejects it and offers a retry.
- [ ] Lane 6. Skip summaries and record. Save `no-summary.png`. Pass when the transcript finishes and the Summary tab offers **Set up summaries**.
- [ ] Lane 7. Record before the speech model finishes downloading. Save `queued-on-model.png`. Pass when the recording processes once the model is ready.
- [ ] Lane 8. Open Settings with Command-Comma and visit every tab. Save `settings-<tab>.png`. Pass when each tab matches UX-19 and UX-20.
- [ ] Lane 9. Add a custom model file through **Add model file...**. Save `custom-model.png`. Pass when it appears in the list and transcribes a recording.
- [ ] Lane 10. Run first launch with networking off through `networksetup -setairportpower en0 off` in `dorytest`. Save `offline.png`. Pass when the app explains the missing model and still records.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. User actions and seconds from first launch to the first finished transcript, excluding download time.
- [ ] Probe. Lane 1's scripted run counts clicks and keystrokes and reads timestamps from the log. Run trunk and head interleaved twice.
- [ ] Baseline. Record the trunk count and time in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when head needs more than 6 user actions or more time than trunk.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 2, and 8 screenshots into `docs/design/review/p6-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of the full first run. Save it as `docs/design/review/p6-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P6 to the stack on P5's branch.

## Limit the load while processing (P6b)

**Depends on.** P6.

**Files.**

- [ ] Create `crates/dory-core/src/app/pace.rs`, `crates/dory-core/tests/pace.rs`, and `crates/dory-macos/src/machine.rs`.
- [ ] Edit `crates/dory-core/src/domain/settings.rs`, `ports/`, `app/pipeline.rs`, `app/events.rs`, and `testing/`.
- [ ] Edit `crates/dory-engines/src/sherpa/engine.rs` and `crates/dory-engines/src/ollama/`.
- [ ] Edit `crates/dory/src/lib.rs` and `config.rs`, `apps/dory-cli`, and `apps/dory-gpui/src/settings_window.rs` and `processing.rs`.

**Build.**

- [ ] Name the data shapes first, per `docs/architecture/hexagonal.md`, "Processing pace". `Settings::processing` is `Processing::{Auto, Fast, Light}`, default `Auto`. `Conditions { power: Ac | Battery | Unknown, low_power: bool, thermal: Nominal | Fair | Serious | Critical }`. `Budget { pace: Fast | Light, threads, reason }`.
- [ ] Write `pace` test-first as a pure function, including the 2-minute hold back to fast.
- [ ] Add the `Machine` port, `FakeMachine`, and `dory_macos::MacMachine`. The port only reads conditions. Keep all `unsafe` in `machine.rs` with safety notes.
- [ ] The pipeline worker evaluates the pace before each speech window and passes `threads` to the speech engine. The current thread count stays the `Fast` count, so nothing gets slower by default. Light changes the thread count only, never the priority, because engine threads do not inherit a QoS (measured, see `decisions.tsv`).
- [ ] Sherpa rebuilds its recognizer when `threads` changes. Summaries are not paced, because Ollama ignores `num_thread` (measured).
- [ ] `Progress` carries the `Budget`. The row and detail pane show the reason (UX-30). Add **Processing** to **Settings > Transcription** (UX-30).
- [ ] `dory-cli process` takes `--pace fast|light` for the bench. `DORY_MACHINE=battery,low-power,thermal=serious` overrides the conditions for lanes, refused unless `DORY_DATA_DIR` is set, like the `--fake-*` flags.

**You see.**

- [ ] Unplug the Mac while a 60-minute meeting transcribes. Within one window the row reads "Light, on battery", the fans stay quiet, and the transcript still finishes.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/tests/pace.rs` covers every (setting, condition, recording) row, the 2-minute hold with a `ManualClock`, a pace change mid-transcribe that rebuilds once, `threads` reaching `TranscribeRequest`, and a summary pass carrying no light reason. Engine tests check the sherpa rebuild on a threads change. Run `cargo test --workspace`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against P6's head. Transcribe the 60-minute AMI set with `--pace fast` on both. Save `pace-regress.txt`. Pass when head's word error rate and wall time are within the parent's spread.
- [ ] Lane 2. Transcribe the same set with `--pace light`. Save `pace-light.txt`. Pass when the word error rate is within 0.5 points of Fast.
- [ ] Lane 3. Operator lane. Unplug power mid-transcription. Save `battery.png` with `screencapture -x`. Pass when the row reads "Light, on battery" within one window.
- [ ] Lane 4. Operator lane. Turn on Low Power Mode on power. Save `low-power.png`. Pass when the row reads "Light, Low Power Mode".
- [ ] Lane 5. Set `DORY_MACHINE=thermal=serious`. Save `thermal.png`. Pass when the job runs light with the thermal reason.
- [ ] Lane 6. Record for 3 minutes while a job transcribes on power. Save `recording-light.png`. Pass when the job runs light during the recording, returns to fast 2 minutes after Stop, and the take has no `CaptureIssue`.
- [ ] Lane 7. Plug back in, then unplug and replug twice within 2 minutes. Save `hold.txt` from the log. Pass when the pace returns to fast once, 2 minutes after the last plug-in, and the recognizer rebuilds at most twice.
- [ ] Lane 8. Set **Light** on power and run `ps -M -p <pid>` during transcription. Save `light-threads.txt`. Pass when the engine has 2 busy threads, both at normal priority.
- [ ] Lane 9. Summarize with `DORY_MACHINE=battery`. Save `summary-pace.png`. Pass when the transcribe stage shows the light reason, the summary stage shows none, and the summary finishes.
- [ ] Lane 10. Set **Fast** with `DORY_MACHINE=battery`, then open **Settings > Transcription** by keyboard. Save `settings-processing.png`. Pass when the job runs fast and VoiceOver's label for each choice includes its one-line effect.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Wall time, average CPU percent over the process tree, and package energy per audio minute, for the 60-minute AMI set at each pace.
- [ ] Probe. `dory-cli process --pace <p>` with `ps -o pcpu=` sampled every second. Energy is an operator lane with `sudo powermetrics --samplers cpu_power`. Run Fast and Light interleaved, three times each.
- [ ] Baseline. Record P6 head's Fast time and CPU in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when Fast is more than 5% slower than the baseline. Fail when Light averages more than 200% CPU, uses more energy per audio minute than Fast, or takes more than 4 times Fast's wall time. The 4-times bound is a starting guess; record the measured ratio and tighten it.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 3, 6, and 10 screenshots into `docs/design/review/p6b-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of unplugging during transcription and the row switching to light. Save it as `docs/design/review/p6b-review.mp4`.
- [ ] Post the screenshots, the video, and the perf table in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P6b to the stack on P6's branch.

## Make the app accessible end to end (P7)

**Depends on.** P6b.

**Files.**

- [ ] Create `apps/dory-gpui/src/a11y.rs`, `system_prefs.rs`, `tests/a11y_tree.rs`, `tests/contrast.rs`, and `tests/targets.rs`.
- [ ] Edit every view file under `apps/dory-gpui/src/` that the audit flags.
- [ ] Edit `apps/dory-gpui/src/onboarding.rs`, `app/setup.rs`, `settings_window.rs`, and `app/models.rs` for the two items moved here from P8.
- [ ] Edit the root `Cargo.toml` to restore `panic = "abort"` once lane 9 passes.

**Build.**

- [ ] Audit every view against A11Y-1 to A11Y-10 and fix the gaps.
- [ ] `system_prefs.rs` reads `NSWorkspace` `accessibilityDisplayShouldReduceMotion`, `accessibilityDisplayShouldIncreaseContrast`, and `accessibilityDisplayShouldDifferentiateWithoutColor`, observes their change notification, and applies them (A11Y-5, A11Y-6).
- [ ] `a11y.rs` posts announcements through AppKit for record start, stop, done, and failed (A11Y-4).
- [ ] Add text scaling with View > Bigger and View > Smaller from 85% to 200% (A11Y-7).
- [ ] Add F6 landmark navigation and the rest of the shortcut table from the UX spec.
- [ ] File upstream issues for GPUI bugs found, with a minimal repro, and link them in Appendix C.
- [ ] Moved from P8 so P8 never edits a view. Onboarding detects a data folder that already holds recordings imported from the Tauri app and shows upgrading users only the permissions step, because TCC matches the code signature and the new app re-prompts.
- [ ] Moved from P8. **Settings > Storage** offers **Move original WebM files to Trash** through `Library::trash`'s receipt, with Undo. Nothing deletes them automatically.

**You see.**

- [ ] A VoiceOver user can record, find, play, read, rename, export, and delete a meeting without the pointer.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `tests/a11y_tree.rs` walks the accessibility tree of every view and fails on any focusable element without a role or label. `tests/contrast.rs` checks every theme token pair against 4.5 to 1 (A11Y-8). `tests/targets.rs` fails on any hit target under 24 by 24 points (A11Y-9). `tests/onboarding.rs` covers the upgrade path showing only permissions, and `tests/settings.rs` covers moving the WebM files to the Trash and Undo. Run `cargo test -p dory-gpui`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Count interactive elements with a role and label through `app_ax_find` on the main window of the `baseline-tauri` app and the head app. Save `ax-count.png`. Pass when head exposes every interactive element, with trunk's count recorded for comparison.
- [ ] Lane 2. Operator lane. With VoiceOver on, record and stop a meeting using only the keyboard. Save `vo-record.png` with `screencapture -x`. Pass when VoiceOver speaks "Recording started" and "Recording saved".
- [ ] Lane 3. Operator lane. With VoiceOver on, find a recording by search and play it. Save `vo-play.png`. Pass when the slider value is spoken as time.
- [ ] Lane 4. Operator lane. Navigate a transcript with the VoiceOver rotor. Save `vo-rotor.png`. Pass when turns read as speaker, time, then text.
- [ ] Lane 5. Tab through every view. Save `tab-<view>.png`. Pass when focus order matches visual order and the ring is always visible.
- [ ] Lane 6. Operator lane. Turn on **Reduce motion**. Save `reduce-motion.png`. Pass when the meter is static and no transition runs.
- [ ] Lane 7. Operator lane. Turn on **Increase contrast** and **Differentiate without color**. Save `contrast.png`. Pass when the high-contrast theme applies without a relaunch and every status shows a glyph and a word.
- [ ] Lane 8. Set text to 200% with Command-Plus. Save `text-200.png`. Pass when no text clips and every control is reachable.
- [ ] Lane 9. Operator lane. Run 30 minutes of VoiceOver use across every view. Save `vo-soak.png`. Pass when no AccessKit panic appears in the log.
- [ ] Lane 10. Use F6 and Shift-F6 from each region. Save `f6.png`. Pass when focus cycles sidebar, detail, and toolbar in both directions.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Hitch time ratio while scrolling 500 recordings, with VoiceOver off and on.
- [ ] Probe. `xctrace` Animation Hitches on a 10-second scroll, VoiceOver off then on, at P6's head and P7's head, interleaved three times.
- [ ] Baseline. Record P6's head numbers first. The comparison point is P6's head, because `main` still runs Tauri while the stack is open.
- [ ] Rule. Fail when the VoiceOver-off ratio rises more than 10% over P6, or when the VoiceOver-on ratio exceeds 10 ms per second.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 2, 4, and 8 screenshots into `docs/design/review/p7-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of a VoiceOver-only session with audio. Save it as `docs/design/review/p7-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P7 to the stack on P6b's branch.

## Package the GPUI app and delete Tauri (P8)

**Depends on.** P6b to build. P7 to verify and merge. Rebase onto P7's branch before any live or perf lane.

**Files.**

- [ ] Create `apps/dory-gpui/Info.plist`, `apps/dory-gpui/Dory.entitlements`, `crates/dory-macos/src/legacy_cleanup.rs`, `crates/dory-macos/tests/legacy_cleanup.rs`, and `scripts/bundle-macos.sh`.
- [ ] Delete `src/`, `src-tauri/`, `index.html`, `vite.config.ts`, `package.json`, `pnpm-lock.yaml`, `tsconfig.json`, and `tsconfig.node.json`.
- [ ] Edit `README.md`, `docs/USER_GUIDE.md`, `AGENTS.md`, `scripts/pre_build.py`, `.github/workflows/ci.yml`, and the root `Cargo.toml`.

**Build.**

- [ ] `scripts/bundle-macos.sh` builds `Dory.app` with `cargo-packager`, links sherpa-onnx statically, signs with the identity in `DORY_SIGNING_IDENTITY`, and notarizes when credentials exist.
- [ ] Info.plist carries `NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`, `LSMinimumSystemVersion` 26.0, and the bundle id `com.dory.app`, so the data folder carries over.
- [ ] TCC matches the code signature as well as the bundle id. Trunk builds are ad-hoc signed, so the new app re-prompts for the microphone, and System Audio Recording is new in any case. P7 builds the permissions-only onboarding for upgrading users; P8's lane 1 checks it in the bundle.
- [ ] `legacy_cleanup.rs` runs once on first launch. It finds aggregate devices by UID (`com.dory.aggregate` and `com.dory.multioutput`), never by name. If the default input or output is one of them, it restores from `audio_recording_session.json`, then `audio_output_restore.json`, then the built-in mic and speakers. Then it destroys the two devices and logs every step.
- [ ] Remove the last Tauri callers of core APIs, then delete the Tauri crate in the same PR.

**You see.**

- [ ] `scripts/bundle-macos.sh` produces `Dory.app`. Installing it over the trunk app in `dorytest` keeps every recording and setting, and asks only for permissions.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-macos/tests/legacy_cleanup.rs` uses a fake HAL to cover cleanup with each restore file present or missing, a user device named "Dory Input" with a different UID that must survive, and a second run that does nothing. Run `cargo test --workspace` and check that CI no longer runs `pnpm`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. In `dorytest`, install the `baseline-tauri` bundle, record two meetings, then install the head bundle over it. Save `upgrade.png`. Pass when both meetings appear, play, and keep their summaries.
- [ ] Lane 2. After lane 1, read the devices with `system_profiler SPAudioDataType` before and after the head's first launch. Save `devices.png`. Pass when the two Dory UIDs are gone and the default devices are the ones recorded before the trunk install.
- [ ] Lane 3. Launch the bundle from Finder in a fresh `dorytest` profile. Save `clean-launch.png`. Pass when onboarding starts and the permission prompts name Dory.
- [ ] Lane 4. Run `codesign --verify --deep --strict Dory.app` and `spctl -a -vv Dory.app`. Save `sign.png`. Pass when both succeed with a Developer ID identity. Without one, the lane is `NOT VERIFIED` and the operator is told that permissions reset on every rebuild.
- [ ] Lane 5. Record and process a meeting in the bundle. Save `bundle-flow.png`. Pass when bundled transcription runs and the summary appears.
- [ ] Lane 6. In a `dorytest` profile where BlackHole was never installed, record with `afplay` playing. Save `no-blackhole.png`. Pass when the system channel RMS is above -30 dBFS.
- [ ] Lane 7. Run `rg -i 'blackhole|tauri|invoke\(' --glob '!docs/plans/**' --glob '!CHANGELOG*' --glob '!Cargo.lock'`. Save `grep.png`. Pass when only the legacy cleanup module and the README upgrade note match.
- [ ] Lane 8. Follow `README.md` from a fresh clone to a running app in `dorytest`. Save `readme.png`. Pass when every command works as written.
- [ ] Lane 9. Operator lane. Quit during processing, reboot `dorytest`, and relaunch. Save `reboot.png`. Pass when the job resumes.
- [ ] Lane 10. Record 2 hours and process it in the bundle. Save `soak.png`. Pass when RSS over the process tree stays under the P4 idle number plus the loaded model size and no error is logged.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Bundle size without sidecars, cold start, and idle RSS over the process tree of the shipped app.
- [ ] Probe. Build the `baseline-tauri` bundle with `pnpm build` and the head bundle with `scripts/bundle-macos.sh`. Measure `du -sh` excluding `Contents/MacOS/sona*` and `ffmpeg`, the `first-frame` log line, and `scripts/rss-tree.sh` three times each, interleaved.
- [ ] Baseline. Record the trunk numbers in `docs/plans/baselines.md` first.
- [ ] Rule. Fail when the head bundle without sidecars exceeds the trunk bundle without sidecars, or when start or RSS exceeds trunk.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 2, and 3 screenshots into `docs/design/review/p8-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of the upgrade from the trunk app. Save it as `docs/design/review/p8-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends P8 to the stack on P7's branch. The operator lands P1 to P8 bottom-up.

## Close the program

- [ ] Every box above is checked with its evidence.
- [ ] `docs/plans/decisions.tsv` has a row for every merge, every go or no-go, and every scope change.
- [ ] Reply to the operator with the report the execution playbook names.

## Appendix A. Prototype evidence

This planning run tried to settle the GPUI accessibility question with a throwaway GPUI Kit window before writing the plan. Claude Code's auto mode denied building and launching third-party crates, so the question is open. P0 answers it.

What research agents found on 2026-11-15. These are external claims that P0 re-checks, not facts this run verified.

- AccessKit support merged into zed main in PR 56065 on 2026-05-27. Follow-up PRs added landmarks and F6 navigation (60397), `accessibility_id` (61926), and a macOS leak fix (64143).
- Zed ships with AccessKit off by default after panics (PR 57954).
- Open macOS issues report the window instead of the focused field as focused (66691), cached views dropping nodes (66690), and clipped elements reported as visible (66730).
- GPUI has no Dynamic Type, no Increase Contrast reading, no automatic Reduce Motion reading, no native controls, and no tray API.
- crates.io `gpui` 0.2.2 is 13 months old and lacks AccessKit. GPUI Kit (`gpui-component` 0.9.1, 2026-11-09) pins the `gpui-pre` 0.3.12 snapshot of zed main.

Unproven until P0 runs:

- Whether VoiceOver reads a GPUI Kit window correctly on macOS 26.
- Whether a global process tap from `cidre` or `coreaudio-rs` records browser meeting audio reliably.
- Whether the System Audio Recording grant re-prompts monthly the way Screen Recording does on macOS 15.

## Appendix B. Alternatives rejected

- **A SwiftUI shell over `dory-core` through UniFFI.** This is the strongest option for "native and accessible" on macOS. AppKit controls give VoiceOver, Dynamic Type, Increase Contrast, Reduce Motion, and the menu-bar extra for free. Dory already depends on macOS-only pieces (Core Audio taps, the `com.dory.app` TCC grants), so losing cross-platform reach costs little. It lost only because the operator chose GPUI. It stays the fallback if P0 is no-go. P1 to P3 keep the core API in owned types and callback traits so UniFFI can wrap it, but `broadcast::Receiver` and boxed-future traits need a thin adapter that nobody has written or tested yet.
- **Staying on Tauri and redesigning the React UI.** WKWebView gives working web accessibility with less effort. It keeps two languages, the webview audio stack, and the MediaRecorder limits that forced BlackHole. The operator asked to leave it.
- **Slint.** It has AccessKit support but has open VoiceOver reports for lists and tables (slint issues 12644 and 14965) and draws its own controls, so it carries the same risk as GPUI with a smaller ecosystem for this kind of app.
- **Redesign in React first, then port.** The redesigned React screens would be deleted in the port, so this doubles the UI work.
- **Supporting macOS before 26 in the first release.** Process taps exist from macOS 14.2, so a second macOS adapter could cover 14.2 to 15 later through the same capture port. ScreenCaptureKit and BlackHole were rejected for older systems, because ScreenCaptureKit needs the Screen Recording permission that macOS 15 re-prompts monthly and BlackHole is the dependency this program removes. Intel Macs on macOS 26 stay supported and built in CI, but no lane runs on Intel.
- **Extracting a core crate from the Tauri backend.** The first version of this plan moved `lib.rs` into a `dory-core` crate behind a `Paths` seam. The operator asked for ports and adapters, so an architect arena designed the core from scratch (`docs/architecture/hexagonal.md`). The old backend code is reused only where it fits the new ports, such as Sona's HTTP protocol.

## Appendix C. Risks

- **GPUI accessibility bugs (P0, P4, P7).** The owner watches zed issues 66690, 66691, and 66730 and any new AccessKit panic. P4 sets the workspace release profile to `panic = "unwind"` so an AccessKit panic can be caught at the view boundary, and P7 restores `abort` only after its VoiceOver soak passes. Cargo cannot set `panic` per package, so the whole workspace changes together.
  - P7 found that GPUI Kit 0.9.1's component `Dialog` leaves its accessibility node unnamed, so VoiceOver hears only "dialog". The minimal repro is `docs/plans/upstream/kit-dialog-has-no-accessible-name.md`. It is not filed, because filing is the operator's call. `tests/a11y_tree.rs` keeps it as its one exception and fails once Kit is fixed.
- **GPUI API churn (P4 to P8).** GPUI Kit made breaking releases from 0.6 to 0.9 between September and November 2026. The owner pins exact versions in `Cargo.lock` and upgrades only in a dedicated PR.
- **Process tap reliability (P0, P2).** Browser audio may come from helper processes, and a global tap also records notification sounds. P0 lane 8 and P2 lane 2 watch this.
- **Clock drift between mic and tap (P2).** The two devices run on different clocks. Trunk's aggregate device corrected this, and P2 must do it in software. P2 lane 9 measures it over 2 hours.
- **TCC attribution and signing (P2, P5, P8).** A binary launched from a terminal gets its permissions attributed to the terminal, and an ad-hoc signed app loses grants on each rebuild. Permission lanes run the bundled app. Real users keep grants across updates only with a stable Developer ID signature.
- **Operator data and settings (all PRs).** Lanes use throwaway data folders and the `dorytest` account. The arm step backs up the real data folder first.
- **No control skill for native macOS windows (all PRs).** Lanes drive the app through the computer-use tools and the accessibility tree, which is slower and flakier than `control-ui`. The owner re-runs a failed lane once before filing a defect.
- **Data migration (P3, P8).** Legacy WebM files are converted and kept until the user moves them to the Trash. P3 lane 8 checks that the Tauri app keeps working mid-stack.
- **Frozen React UI (P1 to P8).** `main` keeps the Tauri app while the stack is open. Bug and accessibility fixes there stay allowed, and each one rebases the stack.
- **Compile time (all PRs).** Reports describe 10 to 20 minute CI runs for GPUI apps. CI caches with `Swatinem/rust-cache` and builds `dory-gpui` only when its files change.
- **Process weight (all PRs).** Ten lanes per PR follow the pstack plan template. For a one-person project the operator may cut lanes that repeat an earlier PR's check, and the root logs each cut.

## Appendix D. Links and reading list

- `docs/plans/ux-redesign.md` defines every UX and A11Y id.
- `docs/plans/claude-design-brief.md` is the design request whose output feeds P4's theme.
- `docs/plans/speech-model-evaluation.md` fills the `ModelCatalog` rows in P6 and adds `Transcriber` implementations behind the trait from P1.
- `docs/plans/decisions.tsv` is the decision trail per pstack `show-me-your-work/SKILL.md`.
- `docs/plans/baselines.md` holds the trunk numbers, written in P1.
- Run pstack `how/SKILL.md` before P1 and P4. Run pstack `interrogate/SKILL.md` on P2 and P7 before review.
- GPUI accessibility is tracked in [zed PR 56065](https://github.com/zed-industries/zed/pull/56065) and [zed issue 66691](https://github.com/zed-industries/zed/issues/66691).
- [GPUI Kit](https://github.com/longbridge/gpui-kit) is the component library P4 builds on.
- Process taps are shown in [AudioCap](https://github.com/insidegui/AudioCap) and documented in [Apple's AudioHardwareTap reference](https://developer.apple.com/documentation/coreaudio/audiohardwaretap).
