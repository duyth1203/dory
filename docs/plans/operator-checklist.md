# Operator checklist for P4 to P8

Everything an agent could not check. Work top to bottom. Each step says what passes and what to save. Send back the pass or fail per step and the saved files. Paths assume the repo root of the `feat/gpui-rewrite` worktree.

## 0. Set up once

```bash
scripts/bundle-macos.sh
```

```bash
mkdir -p /tmp/dory-checks && rm -rf /tmp/dory-check-data && cp -cR "$HOME/Library/Application Support/dory-dev/live-data" /tmp/dory-check-data && rm -f /tmp/dory-check-data/.lock
```

Launch through `open`, so macOS attributes the permission prompts to Dory and not to Terminal. `DORY_DATA_DIR` keeps your real data folder untouched and makes the old-audio-device cleanup skip itself.

```bash
open -n target/bundle/Dory.app --env DORY_DATA_DIR=/tmp/dory-check-data --stdout /tmp/dory-checks/app.log --stderr /tmp/dory-checks/app.log
```

Save screenshots with `screencapture -x /tmp/dory-checks/<name>.png`.

## 1. Hands-on pass (P4 to P6b), about 15 minutes

Nothing here has been looked at by a person. Note anything that looks wrong, not only failures.

- [ ] The window opens on "Allow recording again". Grant Microphone. Grant System Audio Recording in System Settings, return, and see the status update without a relaunch. Save `permissions.png`.
- [ ] The library: rows grouped by date, search finds a word from a transcript, the filter works, arrow keys move the selection. Save `library.png`.
- [ ] Open a recording: title, metadata line with language, Summary and Transcript tabs (Command-1 and Command-2), scroll until the player docks at the bottom. Click a transcript timestamp and hear playback from there. Save `detail.png` and `docked.png`.
- [ ] Record: Command-Shift-R, speak for 20 seconds with something playing through `afplay`, Command-Period. The Undo toast shows for 5 seconds. Click Undo once, then stop again and let it go. Save `recording.png` and `undo.png`.
- [ ] The new recording processes on its own: stage and percent on the row, then a summary if Ollama runs. Save `processed.png`.
- [ ] The menu-bar item shows a red dot and the time while recording, and its menu stops the recording.
- [ ] Settings (Command-Comma): every tab. Transcription shows the Processing choice and the model list with the info popovers. Save `settings-<tab>.png`.
- [ ] Delete a recording, then Command-Z. It comes back from the Trash.
- [ ] Export the summary as Markdown through the Export menu and open the file.
- [ ] Light mode: set Processing to Light, process a recording, and see "· Light, chosen in Settings" on the row.

## 2. VoiceOver and display (P7 lanes 2, 3, 4, 6, 7, 9; also P0 spike lanes 3, 8, 9, 10)

- [ ] **Lane 2.** Turn VoiceOver on (Command-F5). Continue on "Allow recording again". Press Command-Shift-R, wait 10 seconds, press Command-Period. Pass when VoiceOver says "Recording started" and then "Recording saved". Save `vo-record.png` with the caption panel showing.
- [ ] **Lane 3.** Press F6 until the list has focus, press Down, then Tab to "Playback position" and press Right twice. Pass when VoiceOver reads the position as time, for example "10 seconds of 48 minutes". Save `vo-play.png`.
- [ ] **Lane 4.** Press Command-2 and move into the transcript. Use the rotor (VO-U) on Items or Lists. Pass when each turn reads "speaker, N minutes N seconds, text". Save `vo-rotor.png`.
- [ ] **Lane 6.** While recording, turn on System Settings > Accessibility > Display > Reduce motion. Pass when the level meter stops moving without a relaunch and `app.log` shows `system-prefs: reduce motion true`. Save `reduce-motion.png`.
- [ ] **Lane 7.** With the app open, turn on Increase contrast, then Differentiate without color. Pass when borders darken and shadows go without a relaunch, the recording pill reads "Recording, 0:12", and every row status has a glyph and a word. Save `contrast.png`.
- [ ] **Lane 9, 30 minutes.** Use VoiceOver across every view: both tabs, Settings tabs, Window > Processing Queue, Settings > Storage. Pass when `pgrep -f dory-gpui` still prints a pid and `grep -i panic /tmp/dory-checks/app.log` prints nothing. Save `vo-soak.png`. On a pass I switch the release build to `panic = "abort"`.
- [ ] **Spike lane 7, on the real app.** Click the menu-bar item, then Start Recording. Pass when the window's control turns into the red pill.

Known and expected: the Processing Queue and Set up summaries dialogs read only "dialog" (a GPUI Kit bug, repro in `docs/plans/upstream/`).

## 3. Install and upgrade, in the `dorytest` account (P8 lanes)

Run these in the second macOS account, never in yours. The first launch of `Dory.app` without `DORY_DATA_DIR` removes the "Dory Input" and "Dory Output" devices if they exist and restores the default devices. BlackHole stays installed.

- [ ] **Lane 1, upgrade.** There is no `baseline-tauri` tag, and the old app can no longer transcribe (Sona is gone). Instead, copy your backup of the old data folder (`~/dory-backup-*` from the plan's setup, or a fresh `ditto` of `~/Library/Application Support/com.dory.app`) into `dorytest`'s `~/Library/Application Support/com.dory.app`, then open `Dory.app`. Pass when the old meetings appear, play, and keep their summaries. Save `upgrade.png`.
- [ ] **Lane 2, old devices.** Only if `dorytest` has the old devices. Run `system_profiler SPAudioDataType > /tmp/before.txt`, open `Dory.app` once, then `system_profiler SPAudioDataType > /tmp/after.txt`. Pass when "Dory Input" and "Dory Output" are gone and the defaults are sane. Save `devices.png` and both files.
- [ ] **Lane 3, clean launch.** In a fresh `dorytest` profile, open `Dory.app` from Finder. Pass when onboarding starts at Welcome and the permission prompts name Dory. Save `clean-launch.png`.
- [ ] **Lane 4, signing.** Needs a Developer ID identity. Without one it stays not verified, and permissions reset on every rebuild. With one, run `DORY_SIGNING_IDENTITY="Developer ID Application: …" scripts/bundle-macos.sh`, then `codesign --verify --deep --strict` and `spctl -a -vv` on the app. Save `sign.png`.
- [ ] **Lane 5.** Record and process a meeting in the bundle. Pass when transcription runs and the summary appears. Save `bundle-flow.png`.
- [ ] **Lane 6.** In a profile that never had BlackHole, record with `afplay` playing, then run `sox <take> -n stat` on the system channel. Pass when RMS is above -30 dBFS. Save `no-blackhole.png`.
- [ ] **Lane 8.** Follow `README.md` from a fresh clone to a running app. Pass when every command works as written. Save `readme.png`.
- [ ] **Lane 9.** Quit during processing, reboot `dorytest`, relaunch. Pass when the job resumes. Save `reboot.png`.
- [ ] **Lane 10, 2 hours.** Record 2 hours and process it in the bundle. Pass when no error is logged and memory stays reasonable. Save `soak.png`.

## 4. Decisions only you can make

- [ ] File the GPUI Kit dialog bug upstream? The repro is in `docs/plans/upstream/kit-dialog-has-no-accessible-name.md`.
- [ ] Fix ffmpeg on this Mac (`libx265` is missing), so **Move Original WebM Files to Trash** can convert the old files. Until then it fails and moves nothing.
