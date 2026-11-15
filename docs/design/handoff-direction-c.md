# Dory design handoff: main window, direction C

This file hands the approved visual design to the GPUI build. It sits next to:

- `docs/plans/ux-redesign.md`: behavior and accessibility. It is the source of truth. Requirement ids (`UX-n`, `A11Y-n`) below point into it.
- `docs/plans/gpui-migration.md`: the build program.
- Design canvas (mockups, light and dark): https://claude.ai/artifact/182Pgi1xwq6Kb5KGC618GP. It is private to the owner. The frames are HTML mockups, so take values from this file and use the canvas only to look. An offline copy is at `docs/design/canvas-direction-c.html`.

When this file and the mockups disagree, this file wins. When this file and `ux-redesign.md` disagree on behavior, `ux-redesign.md` wins, except for the spec changes listed in section 7. Apply those to `ux-redesign.md` before building the affected screens.

## 1. Direction C in one paragraph

Spacious and quiet, close to Voice Memos. The sidebar is a rounded panel inset 8 pt from the window edges, so it reads as its own surface. The detail pane is one centered reading column at most 720 pt wide. When the page is scrolled to the top, the player is a large card under the title. Once the card scrolls out of view, it collapses into a slim bar docked at the bottom of the column, so a transcript timestamp can always seek something you can see (UX-3). Body text is 15 pt. Color is used sparingly: the accent marks selection and the main action, and red appears only while recording.

## 2. Tokens

Implement these as one `Theme` struct with `light()` and `dark()` constructors. A third high-contrast variant (A11Y-6) is still to come; see section 8.

### Color

| Token | Light | Dark | Used for |
|---|---|---|---|
| `window_bg` | `#FFFFFF` | `#1A1B1F` | Window and detail pane |
| `sidebar_bg` | `#F1F3F7` | `#25262C` | Sidebar panel. The real build uses the system blurred material (UX-25); this is its fallback and its Increase Contrast fill |
| `panel_border` | `#DDE0E6` | `#3A3B42` | 1 pt border around the sidebar panel |
| `text` | `#1D1D1F` | `#F2F2F4` | Primary text |
| `text_secondary` | `#56585F` | `#AEB0B8` | Metadata, group headers, status labels. At least 5.2:1 on every background it sits on, including the inactive selection |
| `divider` | `#E1E3E8` | `#36373D` | Hairlines, tab underline track |
| `field_bg` | `#E3E6EB` | `#32333A` | Search field |
| `control_bg` | `#FFFFFF` | `#2C2D33` | Bordered buttons, keycaps |
| `control_border` | `#C9CCD3` | `#4A4B52` | Bordered buttons |
| `card_bg` | `#F3F5FA` | `#212637` | Player card, docked player, finish card, sources card. The faint navy tint comes from the app icon |
| `card_border` | `#E0E4EE` | `#323A52` | Same cards |
| `accent` | `#0A63D1` | `#2667D9` | Selection fill, primary button, played waveform, focus ring, active tab underline |
| `accent_text` | `#0A5BC0` | `#6AA6FF` | Accent text on the window background: timestamps, borderless Copy and Export buttons, the processing glyph |
| `on_accent` | `#FFFFFF` | `#FFFFFF` | Text on `accent` |
| `selection_inactive` | `#DADDE4` | `#393A41` | Selected sidebar row while focus is elsewhere |
| `wave_rest` | `#8A90A0` | `#6E7385` | Unplayed waveform bars. At least 3:1 against the card |
| `record_red` | `#D70015` | `#E0303A` | Record dot, recording pill. Never used for anything else |
| `record_glyph` | `#D70015` | `#FF6961` | The "Recording" glyph in a sidebar row |
| `attention` | `#A35400` | `#FFB340` | "Needs attention" glyph, and the pill when 5 minutes are left (UX-10) |
| `playing_bg` | `#0A63D1` at 7% | `#6AA6FF` at 12% | The transcript turn under the playhead |
| `avatar_bg` | `#E3E7F0` | `#323A52` | Speaker initial circle |
| `toast_bg` | `#2C2D31` | `#3A3B41` | Undo toast. Text is white |
| `menu_bg` / `menu_border` | `#FFFFFF` / `#D5D8DE` | `#2C2D33` / `#45464D` | Context menu |

**Accent rule (add to A11Y-8).** The brief says "use the system accent color", but macOS's default blue `#007AFF` measures about 4.0:1 against white, and white text on it is also about 4.0:1. Both fail the 4.5:1 body-text rule. Read the user's accent from AppKit (`NSColor.controlAccentColor`), then derive `accent`, `accent_text` and `on_accent` from it, darkening the lightness (in OKLCH or HSL) until each pair passes 4.5:1. The hex values above are what that rule produces for the default blue. Unit-test it against all eight macOS accent colors in both appearances.

### Type

System font, SF Pro. Tabular figures for every time and duration.

| Role | Size / weight | Line height |
|---|---|---|
| Recording title (editable) | 30 / bold, tracking −0.3 | 1.2 |
| Start screen label | 22 / semibold | 1.2 |
| Section heading (Key points, Live transcript) | 17 / semibold | 1.3 |
| Body: summary, transcript text | 15 / regular | 1.6 |
| Sidebar row title | 14 / semibold | 1.25 |
| Tabs | 14; active is semibold | — |
| Metadata line under the title | 14 / regular, secondary | 1.4 |
| Controls, menu items | 13 / regular or medium | — |
| Row meta, status, timestamps, group headers | 12; group headers are semibold | — |

Every size is multiplied by the View > Bigger/Smaller scale, 0.85 to 2.0 (A11Y-7). Nothing has a fixed height that clips text. Rows, cards and buttons grow with their content.

### Spacing, radii, shadows

- Grid: 4 pt. Common steps: 8, 12, 16, 20, 24, 28.
- Radii: window 12 (system), sidebar panel 12, cards 14, sidebar row 8, buttons 6–7, pill 14 (fully round), menu 10, menu item 5.
- Sidebar panel shadow: `0 1 3 rgba(0,0,0,.08)` plus `0 6 18 rgba(0,0,0,.06)`. Docked player: `0 4 16 rgba(0,0,0,.08)`. Menu and toast: `0 8 28 rgba(0,0,0,.2)`. Increase Contrast turns all shadows off and draws borders instead (A11Y-6).
- Focus ring: 2 pt solid `accent`, offset 2 pt from the element, following its corner radius. Draw it on every focusable element (A11Y-2). GPUI does not provide one.

## 3. Layout

```
┌─ window ─────────────────────────────────────────────────────────────┐
│┌ sidebar panel 300 ┐  toolbar 52: [● Record]  ← nothing else (UX-5)  │
││ traffic lights 44  │                                                │
││ [⌕ Search    ⊜]    │      ┌ column, max 720, centered, side pad 48 ┐│
││ Today              │      │ Title 30                                ││
││  row               │      │ meta 14                                 ││
││  row (selected)    │      │ ┌ player card (at top of scroll) ─────┐ ││
││ Yesterday          │      │ └─────────────────────────────────────┘ ││
││  …                 │      │ Summary | Transcript         Copy Export⋯││
│└────────────────────┘      │ content…                                ││
│ inset 8 on top, left,      │ ┌ docked player (after card scrolls) ─┐ ││
│ bottom                     └─┴─────────────────────────────────────┴─┘│
└──────────────────────────────────────────────────────────────────────┘
```

- The sidebar is resizable from 240 to 380, and its width is restored on relaunch (UX-24).
- The column keeps 48 pt of side padding. Below about 900 pt of window width, drop the padding to 24 pt.
- The detail pane scrolls as one surface. The tab bar sticks to the top once the title scrolls off. In the scrolled state, the tab bar row shows the recording title at 14 semibold to the left of the tabs (frame 5).

## 4. Components

Each lists its states and the ids it satisfies.

**Sidebar row** (UX-1, A11Y-1, A11Y-8). Three lines: title (14 semibold, one line, ellipsis), meta (`start · duration · language`, 12 secondary), status (12 pt glyph and a 12 pt label). Padding 10/12, radius 8, 2 pt gap between rows. Selected with keyboard focus in the list: `accent` fill, all text `on_accent`. Selected with focus elsewhere: `selection_inactive` fill, normal text colors. Context-menu target: 2 pt `accent` outline, no fill change. The VoiceOver label reads all three lines: "Design review, 10:30, 48 minutes, English, Done".

**Status glyphs.** Always a glyph followed by a word. Map them from `pipeline_status` in `src-tauri/src/lib.rs`:

| State | Glyph | Label | Glyph color |
|---|---|---|---|
| recording (new, not a `pipeline_status` today) | filled dot | Recording | `record_glyph` |
| `idle` | doc with lines | Ready to transcribe | secondary |
| `queued` | clock | Queued | secondary |
| `transcribing` | ring at N% | Transcribing, N% (plus "about M min left" once known, UX-12) | `accent_text` |
| `summarizing` | ring, indeterminate | Summarizing | `accent_text` |
| `done` | check in circle | Done | secondary |
| `failed` | triangle | Needs attention | `attention` |

With Reduce Motion on, the progress ring shows its fill but does not spin (A11Y-5).

**Record control** (UX-5, UX-7, UX-8, UX-10). Idle: bordered pill 28 tall, red 10 pt dot, label "Record". Recording: a red pill (`record_red` fill, white text) containing a white dot, the elapsed time, and a 5-bar level meter, followed by separate round Pause (⇧⌘P) and Stop (⌘.) buttons, each 28×28. With 5 minutes left, the pill fill changes to `attention` and the label becomes "5 min left". With Reduce Motion, the level meter is a static bar. VoiceOver label: "Recording, 12 minutes 4 seconds".

**Start screen** (UX-6, new). One large button: an 80 pt circle holding a 30 pt red dot, with the label "Start Recording" (22 semibold) below, as a single focusable target that has focus by default. Then one line of secondary text, the ⇧⌘R keycaps, and a sources card listing Microphone and System audio, each with its device name and a "Ready" status (glyph and word). A source that isn't ready shows "Not allowed" and a **Grant** button, using the same component as onboarding (UX-16).

**Player card** (A11Y-3). Waveform 48–64 pt tall made of 2 pt bars with 2 pt gaps. Played bars are `accent`, the rest `wave_rest`, and a 2 pt playhead line is in `text`. Below it, three columns: `elapsed / total` on the left, then Back 5 s (32), Play (40 circle, `accent`) and Forward 5 s (32) in the middle, then the speed button on the right. The waveform is the slider: value text "12 minutes 30 seconds of 48 minutes", arrow keys move 5 s, Page Up/Down move 1 min.

**Docked player.** The same controls on one line, 52 tall, in a `card_bg` rounded bar inside the column, 14 pt above the window's bottom edge. It appears when the player card scrolls out of view. With Reduce Motion it switches instantly; otherwise it cross-fades in 150 ms. Keyboard focus moves with it, and both versions are the same slider for VoiceOver.

**Tabs** (UX-2). Underline style: 34 tall, 14 pt, active is semibold with a 2 pt `accent` underline, over a 1 pt `divider` track. ⌘1 and ⌘2 switch tabs. Actions on the right of the same row are borderless `accent_text` buttons. On Summary: Copy, Export ▾, ⋯. On Transcript: Copy Transcript, Export ▾, ⋯.

**Transcript turn** (UX-3, A11Y-10). A list item with a 30 pt initial circle, the speaker name (14 semibold), the timestamp as a button (12, `accent_text`, label "Play from 12:24"), and the text (15/1.6). The turn under the playhead gets the `playing_bg` fill plus a speaker glyph and the word "Playing". Return on a focused turn seeks there. Turns from recordings made before the port have no timestamps and no seek.

**Speaker rename** (UX-3). The name becomes a text field 150 wide with the focus ring, and a 12 pt hint below it: "Renames Speaker 3 in all 14 turns. Return to save, Escape to cancel." Saving updates every turn and the avatar initial.

**Live transcript** (new). Lines are a 42 pt timestamp column plus 15 pt text. The line still being heard is in secondary color. The list is a polite live region. A header shows "Live transcript" and a "Draft" badge, and a footnote says names and punctuation are finished after stop. A card above it says the window can be closed and has a **Hide Live Transcript** button. Hiding it is remembered, and the menu-bar extra keeps working either way.

**Finish card** (new, see section 7.1). Heading "Finish this recording?", one sentence that includes the estimated time, buttons **Transcribe and Summarize** (primary, has focus) and **Transcript Only**, and a checkbox "Always do this when I stop" that sets Settings › Transcription › Process recordings automatically.

**Undo toast** (UX-9). Centered in the detail pane, 28 pt above the bottom. Text "Recording saved", an **Undo** button and a 5-second countdown. Announced to VoiceOver as a status (A11Y-4).

**Context menu and ⋯ menu** (UX-21, UX-22, UX-23). Rename ↩ · — · Copy Summary ⇧⌘C · Copy Transcript · Export… ⌘E · — · Transcribe Again… · Duplicate (see 7.3) · Show in Finder · — · Move to Trash ⌘⌫. Items are 24 tall. Every item also exists in the menu bar with the same shortcut.

## 5. Screens and states

Frame numbers refer to the canvas.

| Frame | State | Must show |
|---|---|---|
| C (top row) | Recording selected, Summary tab, top of scroll | Player card, rendered summary, focus on the Summary tab |
| 1 | No recordings | Sidebar text "No recordings yet". Start screen with the first-run line. Focus on Start Recording |
| 2 | Launch with recordings, nothing selected | Same start screen with the line "Pick a recording on the left, or start a new one." |
| 3 | Recording | Red pill with meter, Pause and Stop. "Untitled recording · Recording" row at the top, selected. Live transcript |
| 4 | Just stopped | Row says "Ready to transcribe". Finish card with focus on the primary button. Player card at 0:00. Transcript · Draft tab. Undo toast |
| 5 | Transcript tab, scrolled | Sticky tab row with the title, turns, playing turn highlighted, rename field focused, docked player |
| 6 | Context menu on a row | Row outlined, menu open, keyboard highlight on one item |

Still to design, in this style and in this order: processing and failure (UX-11 to UX-13), first run (UX-16 to UX-18), Settings (UX-19, UX-20, UX-26), 200% text and Increase Contrast (A11Y-6, A11Y-7).

## 6. Sample data

Use these fixtures in previews and snapshot tests so screens match the canvas: Weekly sync (14:00, 62 min, transcribing 40%), Design review (10:30, 48 min, done, speakers Sam, Priya and Speaker 3 renamed to Alex), 1:1 with Sam (27 min), Kundengespräch Müller GmbH (35 min, German), Partner call, Lyon office (41 min, English and French), Hiring debrief (19 min, needs attention: summary model missing), Customer interview: Ortega (94 min).

## 7. Spec changes these designs need

These decisions differ from `ux-redesign.md`. Update the spec first so build lanes don't verify against two different rules.

1. **UX-9 and UX-11, prompt after stop.** The owner wants a prompt after stopping, but the spec says processing starts by itself. Proposed rule: *Process recordings automatically* stays **on** by default, and stopping starts processing after the 5-second Undo window. The finish card (frame 4) appears only when that setting is off, and its checkbox turns the setting back on. Owner to confirm.
2. **UX-24, what opens at launch.** The spec restores the last selected recording, but the owner wants nothing selected at launch. Proposed rule: restore the selection if the app was last quit less than 4 hours ago; otherwise show the start screen (frame 2). Owner to confirm the threshold.
3. **Duplicate.** It has no stated user need. "Try another language" is already covered by Transcribe Again… (UX-26). Recommendation: leave it out of v1 unless a concrete need comes up.
4. **New: live transcript while recording.** Add it as UX-27.
   - Use a streaming on-device recognizer. Two options are a whisper.cpp streaming setup, or Apple's on-device speech APIs where available. Prototype both for accuracy and CPU load before choosing.
   - Lines are drafts. Speaker labels need the whole file, so the existing post-stop pass still runs and replaces the draft.
   - It runs at low priority. Make it a setting, and consider turning it off by default on battery: it competes with the video-call app for CPU.
   - Measure CPU and energy during a 60-minute call with both on before shipping.
5. **New: start screen** as the UX-6 empty state and the no-selection state (frames 1 and 2).
6. **A11Y-8, accent darkening rule** from section 2.

## 8. Not decided yet

- The Increase Contrast palette. Expected: solid `sidebar_bg`, no shadows, borders at `text_secondary`, `wave_rest` darkened to 4.5:1.
- What a "Needs attention" row shows in the detail pane (frame planned with processing and failure).
- App icon. The current artwork resembles a well-known film character, and the notebook on it reads "MAYA". Replace it with original artwork before release.
