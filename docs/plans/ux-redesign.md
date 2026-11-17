# Dory UX redesign

This spec defines what the native Dory app looks like and how it behaves. The GPUI program (`docs/plans/gpui-migration.md`) builds it. The Claude Design brief (`docs/plans/claude-design-brief.md`) asks for visual directions inside these rules. The approved visual design is direction C in `docs/design/handoff-direction-c.md`. It owns tokens, layout, and components. This file owns behavior.

Items marked **Proposed** come from the design handoff and wait for the owner's confirmation. Build lanes verify against the proposed rule until the owner changes it.

Each requirement below has an id (`UX-n`, `A11Y-n`). PR verification lanes cite these ids.

## The job Dory does

A person is about to join a call. They want three things, in this order:

1. Start capturing their voice and the other side's audio in one action, with no setup on the day.
2. Forget about the app while the call runs.
3. Get a readable transcript with speaker labels and a short summary they can copy or export.

Everything in the UI serves that loop or gets out of its way.

## What gets in the way today

These findings come from reading the current React app and backend.

- **Setup is a hard gate.** `SetupScreen.tsx` blocks the app until seven checks pass. The checks include installing BlackHole, building aggregate devices, pulling a 9.6 GB Gemma model, and pasting a GGUF file path into a text field. A user who only wants a transcript still has to install Ollama.
- **Recording changes system audio devices.** `begin_recording_audio_routing` in `lib.rs` switches the default output to the "Dory Output" Multi-Output device. macOS disables the volume keys on Multi-Output devices, which is why the backend has an unused `osascript` volume workaround.
- **Processing waits for a click.** A saved recording sits at `pipeline_status: idle` until the user presses **Summarize**. The toast after saving tells the user to do this, which shows that the default is wrong.
- **The list carries everything.** Each `RecordingCard` holds the player, rename, actions, transcript, and summary in collapsible panels. Long transcripts push other recordings off screen, and there is no place to read a meeting at full width.
- **The live transcript is not tied to a recording.** `useTranscriptStream` listens to a global event, so it only works while exactly one job runs.
- **The keyboard covers little.** The only key handling is Escape to cancel a rename. The scrub bar responds only to the pointer. Radix supplies focus handling in dialogs, and the native port loses that unless it is rebuilt on purpose.
- **The chrome uses engineering words.** The header reads "Sona + Gemma 4 + BlackHole · Local AI". Users see component names instead of what the app does for them.
- **Results stay inside the app.** README "Known Limitations" says there is no export.

## Information architecture

One main window with a sidebar and a detail pane. This is the standard macOS source-list layout used by Mail, Notes, and Voice Memos.

- **UX-1. Sidebar.** Recordings grouped by **Today**, **Yesterday**, **Previous 7 days**, and month. Each row shows the title, the start time, the duration, and a status glyph with a text label. A search field sits at the top of the sidebar and filters by title, transcript, and summary text. A filter menu in the search field offers **All**, **Processing**, **Needs attention**, and **Done**.
- **UX-2. Detail pane.** The selected recording fills the pane. From top to bottom it has an editable title, metadata (date, duration, speakers found), the player, and two tabs, **Summary** and **Transcript**.
- **UX-3. Transcript tab.** Lines group by speaker turn. Each turn shows the speaker label and a timestamp. Recordings transcribed before the port have no stored times, so their turns show no timestamp and cannot seek. Clicking a timestamp or pressing Return on a focused turn seeks the player there. Speakers can be renamed once ("Speaker 1" becomes "Sam") and the name applies to the whole recording.
- **UX-4. Summary tab.** Rendered Markdown with sections for key points, decisions, and action items. A **Copy** button and an **Export** menu sit above it.
- **UX-5. Toolbar.** The window toolbar holds the record control on the leading side. Nothing else lives there. Search stays at the top of the sidebar (UX-1).
- **UX-6. Start screen and empty states.** With no recordings, or with nothing selected, the detail pane shows the start screen. It has one large **Start Recording** button that has focus by default, one line of text, the Command-Shift-R shortcut, and a sources card. The card lists Microphone and System audio with the device name and a status (glyph and word). A source that is not allowed shows **Not allowed** and a **Grant** button, the same component as onboarding (UX-16). With no recordings the line is the first-run line. With recordings it reads "Pick a recording on the left, or start a new one." A selected recording that is still processing shows its live state in place of the tabs (see UX-11).

## Recording

- **UX-7. One action to record.** The toolbar record button, the menu item **File > New Recording** (Command-Shift-R), and a menu-bar extra all start recording at once. The app captures the microphone and system audio through Core Audio process taps, through the platform audio adapter described in `docs/architecture/hexagonal.md`. Recording never changes the user's default input or output device.
- **UX-8. Recording state.** While recording, the toolbar control turns into a red elapsed-time pill with a live level meter and **Pause** and **Stop** buttons. A new row appears at the top of the sidebar with a "Recording" label. The menu-bar extra shows a red dot and the elapsed time. The menu-bar item starts and stops recording without opening the window, and its menu lists what is in progress: the recording, the running job, the waiting count, and downloads. Closing the window keeps Dory running in the menu bar; only Quit exits, and it asks first when something is in progress.
- **UX-9. Stop does not ask.** Stopping saves right away and offers **Undo** for 5 seconds. Undo resumes the recording into the same file. Processing starts only after the 5 seconds pass. The current "Stop this recording?" dialog goes away. Delete still asks, because it removes data.
- **UX-10. Limits stay visible.** The 2-hour cap remains. At 5 minutes left, the pill turns amber and the app posts one notification. At the cap, the app saves and posts a notification that says so.

## Processing

- **UX-11. Processing starts on stop.** **Proposed.** **Settings > Transcription > Process recordings automatically** is on by default. When it is on, a stopped recording joins the queue after the 5-second Undo window. When it is off, the detail pane shows the finish card: "Finish this recording?", one sentence with the estimated time, **Transcribe and Summarize** (primary, has focus), **Transcript Only**, and a checkbox **Always do this when I stop** that turns the setting back on.
- **UX-12. Progress is specific.** The selected recording's detail pane shows one processing card from Stop until the summary is ready. The card shows the recording's waveform, which fills in as transcription passes through the audio, with a caption in meeting time ("Transcribed up to 24:48") and a step row for Recording, Transcript, and Summary. The stage words, the sidebar row words, and the VoiceOver sentences are the ones in `docs/design/processing-view.md` section 4.
  - Time left appears once the first minute of audio is transcribed. Before that the card says "estimating time left". The estimate is rounded up to whole minutes, and shows "less than 1 min" below 60 seconds. It updates at most every 10 seconds. It only goes down, unless the new estimate is more than 30% higher, in which case it may go up once. The estimate uses the current pace (UX-30).
  - Queue position shows the number of recordings ahead and, when they have estimates, when this one starts.
  - Transcript lines for this recording stream into the Transcript tab as they are written, without speaker names until the job finishes. This is a committed requirement, separate from UX-27, which is about lines during recording.
  - VoiceOver announces the card's sentence when the stage changes, when processing fails, and when it is done. Percent changes update the sentence without announcing it.
  - One card, no jumps. When processing finishes, the card becomes the player in place (UX-2).
- **UX-13. Failures say what to do.** A failed job shows one sentence and one button. When the audio is safe, the sentence says so first ("Your recording is saved, but …"). The button fixes the cause or retries. After the fix, the same card continues from where it stopped, without a new click. When the summary step fails, the transcript stays readable, and only the Summary tab shows the failure. The row shows "Needs attention" with the triangle glyph, and its VoiceOver sentence is the sentence plus the button's label. Set Up Summaries is a two-step dialog: install Ollama, then download the model with progress. It can close while the download continues, and the waiting recordings are summarized when it finishes.
  - Speech model missing: "Your recording is saved, but the speech model isn't on this Mac." **Download Parakeet (487 MB)**, which then shows "Downloading Parakeet, 120 of 487 MB" in the same card.
  - Summary model missing: "The transcript is ready, but summaries aren't set up." **Set Up Summaries**.
  - Disk full: "Your recording is saved, but there isn't enough space to transcribe it." **Free Up Space…**, which opens Settings > Storage.
  - Transcriber crashed: "Transcription stopped partway. Your recording is saved." **Try Again**.
  - Audio unreadable: "This recording's audio can't be read." **Show in Finder**.
- **UX-14. Notifications.** Dory posts a system notification when its window can't tell you itself, and every kind can be turned off in **Settings > General > Notifications** (all on by default, each with its effect in one line):
  - **Recording started and saved:** when a recording starts or stops while Dory's window is closed, for example from the menu-bar item.
  - **Recording time limit:** the 5-minute warning and the save at the 2-hour cap (UX-10).
  - **Transcript ready** and **Summary ready:** when a job finishes a stage and the window is not key. Each is announced once.
  - **Needs attention:** when a job stops and needs a fix (UX-13), with the one-sentence cause.
  - **Model downloaded:** when a speech or summary model finishes downloading and the window is not key.
  - Clicking a notification opens that recording, reopening the window if it is closed. If macOS has notifications turned off for Dory, the Notifications section says so and offers **Open System Settings**.
- **UX-15. The queue stays out of the way.** The queue appears as status on sidebar rows. **Window > Processing Queue** opens a small non-modal Processing window, which floats and remembers its position, with sections in this order:
  1. Now: the running job, with its stage, percent, time left, and mini waveform.
  2. Up next: the queued jobs in order, with position, length, and start time.
  3. Needs attention: each blocked job with its cause and its UX-13 fix.
  4. Finished this session.
  - The header shows the total ("3 recordings · about 24 min left"). Jobs can be reordered with Move to Top, Move Up, and Move Down (menu, keyboard, and VoiceOver actions) or by dragging. Cancel acts at once and offers Undo for 5 seconds. Choosing a row selects its recording in the main window and leaves the Processing window open. Layout in `docs/design/floating-surfaces.md` section 4.
- **UX-30. Processing stays gentle.** **Settings > Transcription > Processing** has three choices, each with its effect in one line.
  - **Automatic** (default): "Runs fast when the Mac is plugged in and cool. Runs light on battery, in Low Power Mode, when the Mac is hot, and while you record."
  - **Fast**: "Uses up to 4 cores. Finishes soonest, but the fans may spin up and the battery drains faster."
  - **Light**: "Uses 2 cores. Takes longer, stays quieter, and is easier on the battery."
  - Light limits transcription only. Summaries run at their normal pace: the summary model works mostly on the GPU, and Ollama ignores a thread limit.
  - While transcription runs light, the detail pane says why, for example "Light, on battery", and offers **Use Fast This Time**, which runs this one job fast without changing the setting. The sidebar row uses the short form "40%, light", because the full reason does not fit at 52 pt. UX-12's time-left estimate uses the current pace.
  - Automatic switches to light at once, and back to fast only after the condition has been clear for 2 minutes, so plugging in and out does not make the work stutter.
- **UX-27. Live transcript while recording.** **Proposed.** While recording, the detail pane shows draft lines from a streaming on-device recognizer: a timestamp column and the text, with the line still being heard in secondary color. The list is a polite live region. A header reads "Live transcript" with a **Draft** badge, and a footnote says names and punctuation are finished after stop. A card above it says the window can be closed and offers **Hide Live Transcript**, which is remembered. The post-stop pass still runs and replaces the draft, because speaker labels need the whole file. The recognizer runs at low priority behind **Settings > Transcription > Live transcript**, which may default to off on battery. It ships only after a 60-minute call with a video app shows acceptable CPU and energy use.

## Onboarding and setup

- **UX-16. First run is three short steps.** Welcome, permissions, and models. The app explains each permission before macOS shows its prompt.
  - Permissions covers **Microphone** and **System Audio Recording**. Each has a **Grant** button and a status that updates when the user returns from System Settings.
  - Models downloads the default speech model with a progress bar and a size label. It detects Ollama and offers **Set up summaries** as an optional step.
- **UX-17. Setup never blocks recording.** A missing summary model turns off summaries only. A missing speech model still lets the user record, and those recordings queue until the model is ready. The hard-gate `SetupScreen` goes away.
- **UX-18. No file paths in the main flow.** The app manages models in its own data folder. A model is a choice from a list with sizes and speed notes. **Settings > Transcription > Add model file...** opens a native file panel for advanced users.

## Settings

- **UX-19. A native Settings window** opens with Command-Comma. It uses the System Settings layout: a sidebar with the panes **General**, **Recording**, **Transcription**, **Summaries**, and **Storage**, and a search field that finds settings by name. Layout per `docs/design/settings-icons-blur.md` section 1.
- **UX-20. Fewer options.** Appearance follows the system. The custom palette setting goes away, and the system accent color tints the app. Each remaining option names its effect in one line. Two themes, **Modern** (default) and **Book**. A theme changes colors, the reading typeface, and the presentation flags in `docs/design/book-theme.md`; it never changes layout, controls, or what any text says. Light and dark follow the system in both. Book is experimental: it shows only once **Settings › General › Theme › Try the Book theme** is on.
- **UX-28. Model explainer.** Each model row in **Settings > Transcription** and **Settings > Summaries** has an info button that opens a popover. The popover gives one sentence on what the model is, then **Good at** and **Watch out for** lists, then the download size and the languages it covers. The text comes from the model's catalog row, so a new model brings its own explanation. A row that cannot handle one of the meeting languages says which one, for example "Doesn't support Tagalog", in the row itself, not only in the popover. The popover opens on click and with Space on the focused info button, and VoiceOver reads it as a group labelled with the model name.
- **UX-29. Meeting languages.** **Settings > Transcription > Meeting languages** lists the languages meetings are held in. Onboarding asks once and preselects the system language. With language on **Automatic**, Dory only picks a speech model that covers every listed language, so a meeting that mixes Tagalog and English never goes to an English-only model. Meeting languages is a combobox with chips and type-ahead search. The list keeps at least one language.
- **UX-26. Language.** **Settings > Transcription > Language** defaults to **Automatic**, which detects the language and follows switches mid-meeting. The detail pane shows the detected language next to the duration. The user can pick a language for one recording and re-transcribe it from the detail pane. Summaries use the meeting's main language. The Language options are Automatic plus the meeting languages (UX-29), and nothing else. When the selected language is removed from the meeting languages, Language returns to Automatic and says so.
- **UX-31. Scrolling.** Settings content has one scroll area. Any inner scroll area passes scrolling to its parent when it reaches its edge; a trackpad gesture that starts inside it stays with it until the gesture ends.

## Platform conventions on macOS

- **UX-21. Menus.** The menu bar has **Dory**, **File**, **Edit**, **View**, **Recording**, **Window**, and **Help**. Every command in the UI also exists as a menu item with its shortcut.
- **UX-22. Delete is recoverable.** Delete moves the recording to the macOS Trash and offers Undo with Command-Z.
- **UX-23. Export.** **File > Export** writes Markdown, plain text, or SRT subtitles through a native save panel.
- **UX-24. Window state.** The app restores the window size and the sidebar width (240 to 380 pt) on relaunch. **Proposed.** It restores the selected recording only if the app quit less than 4 hours ago. Otherwise it opens on the start screen (UX-6).
- **UX-25. Materials and type.** The main and Settings windows use a blurred window background. The sidebar column is translucent; the content pane is opaque. Everything that floats over the app (dialogs, popovers, menus, toasts) is opaque `surface_raised` with a 1 pt `surface_border`. In dark mode it is lighter than the window under it. Modal dialogs dim the window with `scrim`. Nothing uses per-element blur. Increase Contrast draws borders in `text_secondary` and removes shadows; Reduce Transparency makes the window and the scrim opaque. Text uses the system font. Light and dark follow the system appearance. Book uses an opaque window: no glass. Icons come from Lucide, vendored in `assets/icons/`. Tokens and sizes are in `docs/design/floating-surfaces.md`.

## Keyboard shortcuts

| Command | Shortcut |
| --- | --- |
| New recording | Command-Shift-R |
| Pause or resume recording | Command-Shift-P |
| Stop recording | Command-Period |
| Play or pause playback | Space (when no text field has focus) |
| Skip back or forward 5 seconds | Left Arrow, Right Arrow in the player |
| Find | Command-F |
| Show Summary or Transcript | Command-1, Command-2 |
| Copy summary | Command-Shift-C |
| Export | Command-E |
| Rename | Return on a selected sidebar row |
| Move to Trash | Command-Delete |
| Settings | Command-Comma |
| Move between sidebar, detail, and toolbar | F6 and Shift-F6 |

## Accessibility requirements

GPUI draws its own controls, so the app gets none of these from AppKit. Each one is a build task and a check.

- **A11Y-1. Every control has a role and a label.** VoiceOver reads each button, field, tab, row, slider, and toggle by name. Icon-only buttons have text labels.
- **A11Y-2. Everything works from the keyboard.** Tab order follows the visual order. Each focused element shows a visible focus ring. No action requires a pointer.
- **A11Y-3. The player is a slider.** It exposes its value as elapsed time ("12 minutes 30 seconds of 48 minutes"). Arrow keys move it 5 seconds, and Page Up and Page Down move it 1 minute.
- **A11Y-4. State changes are spoken.** Recording started, recording stopped, processing finished, and processing failed each produce a VoiceOver announcement. If GPUI lacks live regions, the app posts `NSAccessibilityAnnouncementRequestedNotification` through AppKit.
- **A11Y-5. Reduce Motion is honored.** The app reads the macOS **Reduce motion** setting at launch and on change, and passes it to GPUI's `set_reduce_motion`. With it on, the level meter shows a static bar and no view animates.
- **A11Y-6. Increase Contrast is honored.** The app reads the macOS **Increase contrast** setting and switches to a high-contrast theme with solid borders and no translucent materials. Reduce Transparency switches the windows to an opaque background and a solid sidebar.
- **A11Y-7. Text size is adjustable.** GPUI does not follow a system text size. **View > Bigger** and **View > Smaller** (Command-Plus, Command-Minus) scale all text from 85% to 200%, and the layout reflows without clipping.
- **A11Y-8. Color never carries meaning alone.** Every status has a glyph and a text label. Text contrast meets WCAG 2.2 AA (4.5 to 1 for body text) in both appearances. The app reads the user's accent from `NSColor.controlAccentColor` and derives `accent`, `accent_text`, and `on_accent` by darkening its lightness until each pair passes 4.5 to 1. macOS's default blue fails as is (about 4.0 to 1 on white). A unit test covers all eight macOS accent colors in both appearances.
- **A11Y-9. Targets are large enough.** Pointer targets are at least 24 by 24 points. Primary actions are at least 28 points tall.
- **A11Y-10. Transcripts read in order.** VoiceOver reads a transcript turn as "Sam, 12 minutes 30 seconds, then the text". Turns are list items, so rotor navigation works.

## Cut from the current app

- The `SetupScreen` gate and the Introduction dialog. First-run onboarding replaces both.
- The stop confirmation dialog. Undo replaces it.
- The palette setting and the engineering subtitle in the header.
- BlackHole, the "Dory Input" and "Dory Output" devices, and default-device switching. Process taps replace them. The first release targets macOS 26.
- Batch selection checkboxes on every card. Multi-select in the sidebar with Command-click and Shift-click replaces them.
- **Duplicate** in the context menu. It has no stated need, and **Transcribe Again...** covers trying another language (UX-26).
