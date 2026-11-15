# Claude Design request for Dory

Use this request to get visual directions for the native Dory redesign from Claude Design. The requirements live in `docs/plans/ux-redesign.md`. This file holds the request and the steps to send it.

## Send the request

1. Open Claude Design and start a new project named "Dory".
2. Attach these files:
   - `src-tauri/icon-source.png` (the app icon).
   - `docs/plans/ux-redesign.md` (the requirements).
   - Two screenshots of the current app, the setup screen and the main window, so Claude Design can see what to move away from.
3. Paste the request below as the first message.
4. Pick one direction. Then ask for the remaining screens in that direction, one at a time.
5. Save the chosen direction's exports to `docs/design/` and note the choice in `docs/plans/decisions.tsv`.

## The request

```text
Project: Dory, a local-first meeting recorder for macOS.

What it does: Dory records your microphone and your Mac's audio during a call,
transcribes it on-device with speaker labels, and writes a short summary with
decisions and action items. Nothing leaves the Mac.

Who it is for: people who take many calls a day (founders, engineers, managers,
researchers) and want notes without a bot joining the meeting. Some of them use
VoiceOver, keyboard-only navigation, larger text, Reduce Motion, or Increase
Contrast. Design for them from the start.

The core loop, which every screen serves:
1. Start recording in one action, with no setup on the day.
2. Forget the app while the call runs.
3. Read a transcript with speaker labels and a summary, then copy or export it.

Platform and constraints:
- A native macOS app on macOS 26 and later. It must feel like a well-made Apple
  app (Voice Memos, Notes, Mail), not a website in a window.
- The UI is built with GPUI, a Rust GPU-rendered framework. It draws every
  control itself, so use plain shapes, system font (SF Pro), SF Symbols-style
  icons, flat fills, 1px borders, rounded rectangles, and simple shadows. Avoid
  effects that need a browser (CSS filters, backdrop blur on arbitrary
  elements, complex gradients on text). One blurred material is allowed, on
  the sidebar.
- Light and dark appearance, both first-class. Use the system accent color for
  selection and the primary action. Red only for recording.
- Minimum pointer target 24x24 pt, primary actions 28 pt tall. Body text
  contrast at least 4.5:1 in both appearances. Status never shown by color
  alone: always a glyph plus a word.
- Show a visible keyboard focus ring in every mock where something has focus.

Screens to design (read the attached ux-redesign.md for the exact rules; ids
in brackets):
1. Main window, populated: sidebar with recordings grouped by Today /
   Yesterday / Previous 7 days, search at top, status glyph + label per row
   [UX-1]; toolbar with only the record control [UX-5]; detail pane with editable title, metadata, a player with a
   waveform scrubber, and Summary / Transcript tabs [UX-2, UX-4].
2. Transcript tab: turns grouped by speaker with name and timestamp, one turn
   focused with a focus ring, a speaker being renamed inline [UX-3].
3. Recording in progress: the toolbar control as a red elapsed-time pill with
   a live level meter, Pause and Stop; the new "Recording" row in the sidebar;
   the menu-bar extra with a red dot and time [UX-7, UX-8].
4. Processing: a row and the detail pane showing "Transcribing, 40%, about
   3 min left" with transcript lines streaming in; one failed row with a
   one-sentence cause and a fix button [UX-11 to UX-13].
5. First run, three steps: Welcome, Permissions (Microphone and System Audio
   Recording, each explained before the macOS prompt, with live status),
   Models (default speech model download with size and progress, optional
   "Set up summaries") [UX-16 to UX-18].
6. Settings window with tabs General, Recording, Transcription, Summaries,
   Storage [UX-19, UX-20]. Transcription includes Language, set to
   Automatic by default [UX-26].
7. Empty state for a brand-new user [UX-6].
8. The same main window at 200% text size, and with Increase Contrast on,
   to prove the layout reflows and still reads well [A11Y-6, A11Y-7].

What I want first: three distinct visual directions for screen 1 only, side by
side, each with a one-paragraph rationale. Vary density, how strongly the
sidebar is separated from the detail pane, and how the player sits in the
layout. Keep all three native to macOS. After I pick one, do the remaining
screens in that direction, in the order above.

Tone: calm, quiet, trustworthy. The app should disappear while a call is
running and feel fast when you come back to it. No marketing copy, no
gradients for decoration, no emoji. Use realistic sample data: meeting titles
like "Weekly sync", "Design review", "1:1 with Sam", durations from 12 to
94 minutes, transcripts with 2 to 4 speakers. Include one meeting that is not
in English and one where speakers switch language, with the detected language
shown next to the duration.

Use the attached icon for the app's color mood. Treat ux-redesign.md as the
source of truth for behavior and accessibility.
```

## Take the result into GPUI

Claude Design exports HTML prototypes. GPUI does not render HTML, so treat the export as the visual spec, not as code.

- Copy colors, spacing, radii, and type sizes into the GPUI theme as named tokens in PR P4 of the migration plan.
- Check each token pair against the contrast rule in A11Y-8 before it lands.
- Keep the screenshots of the chosen direction in `docs/design/` as the review baseline for the visual lanes in the migration plan.
