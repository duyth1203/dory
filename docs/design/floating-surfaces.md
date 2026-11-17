# Dory: floating surfaces and the Processing Queue (direction C)

This spec covers dialogs, the scrim, popovers, menus and toasts as one family, and the redesigned Processing Queue and Set Up Summaries dialogs. It extends `docs/design/handoff-direction-c.md` (tokens), `docs/design/processing-view.md` (stage words and the waveform language) and `docs/design/settings-icons-blur.md` (glass and Lucide). Frames are on the canvas in the section "Floating surfaces": https://claude.ai/artifact/182Pgi1xwq6Kb5KGC618GP.

Source: Claude Design, from the request in `docs/plans/claude-design-dialogs-request.md`.

| Frame | Shows |
| --- | --- |
| Q1 | Queue: one running job and two queued. Focus starts on Close |
| Q2 | Queue: the running job is in light pace |
| Q3 | Queue: a blocked job with its fix |
| Q4 | Queue: jobs finished this session |
| Q5 | Queue: empty |
| Q6 | Queue, busiest state, at 800 × 560 |
| Q7 | Queue, busiest state, at 200% text |
| U1–U3 | Set Up Summaries: install, downloading, ready |
| F1 | Explainer popover, Export menu and Undo toast. Shown together for comparison only; they never appear at once in the app |

Each frame shows light and dark side by side. Q7 is the source component, and its Tweaks panel switches state, appearance, text size, window size and Increase Contrast.

The same pass also put direction C's glass sidebar (`settings-icons-blur.md` section 5) on the C, flow and processing frames, so every frame now matches the shipping window.

## 1. How a floating surface separates from the page

Dark mode first. On macOS a floating surface in dark mode is lighter than the window under it. Shadows barely show on dark backgrounds, so the edge has to come from three things, in this order:

1. A lighter fill. `surface_raised` dark is `#2B2C32`, against a window of `#1A1B1F` that drops to about `#0E0E10` under the scrim.
2. A visible border. `surface_border` dark is `#777A84`. It measures at least 3:1 against both the surface and the dimmed window.
3. The scrim (dialogs only) darkens everything behind the dialog.

The shadow adds depth over lighter regions. Nothing depends on it, and Increase Contrast removes it.

Light mode works the same way, except the fill is white and the scrim is lighter. The border is a mid-gray, `#737882`, darker than you might expect. That is what the 3:1 rule against the dimmed window requires (section 7, open question 1).

No surface uses blur or stacked translucent layers. GPUI can't blur behind a single element, so every floating surface is opaque.

## 2. Tokens

| Token | Light | Dark | Increase Contrast | Notes |
| --- | --- | --- | --- | --- |
| `surface_raised` | `#FFFFFF` | `#2B2C32` | same | Fill of dialogs, popovers, menus and toasts. Replaces `menu_bg` and `toast_bg` |
| `surface_border` | `#737882` | `#777A84` | `text_secondary` (`#56585F` / `#AEB0B8`) | 1 pt. Light: 4.4:1 on the surface, 3.1:1 on the dimmed window. Dark: 3.2:1 on the surface, 4.4:1 on the dimmed window. Replaces `menu_border` |
| `surface_inset` | `#F3F5FA` | `#33343B` | same | Cards inside a surface: the running job and the setup steps |
| `inset_border` | `#E0E4EE` | `#45464E` | `text_secondary` | |
| `scrim` | `rgba(15,17,23,0.16)` | `rgba(0,0,0,0.48)` | same | Behind modal dialogs only. It also covers the glass sidebar |
| `scrim_opaque` | `#C4C7CE` | `#0B0B0D` | same | Used with Reduce Transparency |
| `shadow_float` | `0 1 2 rgba(0,0,0,.08)`, `0 12 32 rgba(0,0,0,.18)` | `0 1 2 rgba(0,0,0,.5)`, `0 16 40 rgba(0,0,0,.55)` | none | Dialogs. Two layers: color, offset, blur, no spread |
| `shadow_pop` | `0 1 2 rgba(0,0,0,.08)`, `0 8 24 rgba(0,0,0,.16)` | `0 1 2 rgba(0,0,0,.5)`, `0 10 28 rgba(0,0,0,.5)` | none | Popovers, menus, toasts |
| `close_bg` | `rgba(0,0,0,.05)` | `rgba(255,255,255,.08)` | transparent plus a 1 pt `text_secondary` ring | Resting fill of the dialog's close button |
| `track` | `#E1E4EA` | `#45464E` | `text_secondary` outline | Empty part of a progress bar, and the disabled primary button |
| `wave_rest` (dark, updated) | — | `#7C8193` (was `#6E7385`) | — | Now 3.2:1 on `surface_inset`; the old value was about 2.6:1 |

Text keeps its handoff colors. On `surface_raised`, `text_secondary` measures 7.2:1 light and 6.4:1 dark. On `surface_inset` it is 6.6:1 light and 5.8:1 dark. `accent_text` dark measures 5.7:1 on the surface.

## 3. The family, surface by surface

| | Modal dialog | Popover (explainer, combobox list) | Menu (Export, ⋯, Filter) | Toast (Undo) |
| --- | --- | --- | --- | --- |
| Fill | `surface_raised` | `surface_raised` | `surface_raised` | `surface_raised`. The old dark `toast_bg` is retired, so in light mode the toast is now white |
| Border | 1 pt `surface_border` | 1 pt `surface_border` | 1 pt `surface_border` | 1 pt `surface_border` |
| Radius | 14 | 10 | 8 | 12 |
| Shadow | `shadow_float` | `shadow_pop` | `shadow_pop` | `shadow_pop` |
| Behind it | scrim over the whole window | nothing | nothing | nothing |
| Width | Queue 560, Set Up Summaries 480. Never more than the window width minus 48 | 300 (explainer). The combobox list matches its field | 200–260, fitted to the longest item | Fits its content. At most the detail pane width minus 48 |
| Height | At most the window height minus 80 (minus 48 below 640 pt tall). The body scrolls; the header and footer stay put | The explainer fits its content. The combobox list is at most 8 rows | Fits its content | 40 |
| Padding | Header 18 / 16 / 14 / 20. Body 0 20 16. Footer 12 20 | 14 16 | 5, items padded 0 9 | 0 8 0 16 |
| Title row | Title 15 semibold. An optional subtitle (13, secondary) carries the live summary. Close sits top right | Title 13 semibold, no close button | none | none |
| Close | A 28 × 28 button with a Lucide `x` (14) and `close_bg`, named "Close". Esc does the same | Esc, a click outside, or moving focus away | Esc or a click outside | It closes itself after 5 s. Esc closes it when focused |
| Buttons | Footer with a 1 pt divider above. A text link on the left, if any. Buttons on the right: secondary (bordered), then primary (accent) at the far right. All 28 tall | Inline links only | Items 24 tall. The keyboard highlight is an accent fill with `on_accent` text | One bordered Undo, plus a countdown in secondary text |
| Placement | Centered in the window | 6 pt from its anchor. Flips above the anchor if it doesn't fit below. No arrow | Under its button, left edges aligned. Flips like a popover | Bottom center of the detail pane, 28 above the bottom edge |
| Focus | The first control: Close in the Queue; the first action in Set Up Summaries. On close, focus returns to whatever opened the dialog | The first control in it, or none for a read-only explainer | The first item | Not focused. ⌘Z also undoes |

Increase Contrast: borders use `text_secondary`, shadows are off, the glass becomes solid `sidebar_bg`, and dividers inside surfaces use `text_secondary`. Reduce Transparency: the scrim becomes `scrim_opaque`, and the window is opaque (`settings-icons-blur.md` section 5).

### Motion

| Surface | Open | Close | Reduce Motion |
| --- | --- | --- | --- |
| Scrim | Opacity 0 → 1, 150 ms, `ease_standard` | 1 → 0, 120 ms | Same fade. Fades aren't movement, and macOS keeps them under Reduce Motion |
| Dialog | Opacity 0 → 1, and size from 97% → 100% around its center, 180 ms, `ease_standard` | Opacity → 0, size → 98%, 120 ms | Opacity only, 120 ms |
| Popover | Opacity 0 → 1, and position 4 pt toward the anchor → 0, 120 ms | Opacity → 0, 100 ms | Opacity only |
| Menu | Appears at once, like macOS menus | Opacity → 0, 150 ms, after the chosen item flashes once (two 60 ms color changes) | No flash. It closes at once |
| Toast | Opacity 0 → 1, and position 8 pt up → 0, 200 ms | Opacity → 0, 200 ms | Opacity only |
| Content inside a dialog (rows added or removed) | Height and opacity, 240 ms | Same | Instant |

## 4. Processing Queue

### Layout (Q1)

- **Header:** "Processing Queue" and the live summary, "3 recordings · about 24 min left". Then " · 1 needs attention" when that applies, or " · running light" in light pace. With nothing to do there is no subtitle.
- **Now:** the running job in a `surface_inset` card. It shows:
  - the title and length, then the stage line with the activity dots: "Transcribing 40% · about 4 min left";
  - a 24 pt mini waveform, using the processing card's colors and reading head;
  - the caption "Transcribed up to 24:48 / 62:00";
  - Cancel on the right.
  - In light pace it adds the chip "Light, on battery" and Use Fast This Time (UX-30).
- **Up next · N:** the queued jobs in the order they will run. Each row has:
  - a drag handle (Lucide `grip-vertical`) and the position (1, 2, …) in a 22 pt circle;
  - the title, then "48 min · starts in about 4 min";
  - Move to Top on every row except position 1, and a ⋯ menu.
- **Needs attention · N:** below the waiting line, in its own card with an `attention_border` and the triangle glyph. It shows the title, the one-sentence cause, and the single fix button from UX-13 (for example Set Up Summaries). These jobs are not in the waiting line and have no position.
- **Finished this session · N:** a quiet list. Each row has a check (or a crossed circle for cancelled), the title, and "Done at 14:32" or "Cancelled at 14:05". Done rows show Open; cancelled rows show Process Again. Clear empties the list. The list resets when the app quits.
- **Footer:** Processing Settings… on the left (opens Settings › Transcription › Processing), and Done on the right.
- **Size:** 560 wide. The height fits the content up to the window height minus 80. At 1100 × 720 that is about 640, so about six queued rows show next to the running card before the body scrolls. The header and footer never scroll.

### Actions

- Choosing a row (click, or Return on a focused row) closes the dialog and selects that recording in the main window. Focus moves to its detail pane.
- Cancel acts at once, with no confirmation:
  - The job moves to Finished as "Cancelled".
  - A toast says "Cancelled Design review" with Undo for 5 s. Undo puts the job back in its old position.
  - A cancelled running job loses its progress unless the worker can resume. Say "restarts" in the toast if it can't.
  - The recording's sidebar row returns to "Ready to transcribe".
- Reordering, all three ways, because each serves a different person:
  - Move to Top as a visible button, for the common case;
  - Move Up, Move Down, Move to Top and Cancel in the ⋯ menu, and as VoiceOver custom actions on each row;
  - drag by the handle, for mouse users.
- Keyboard: ⌥↑ and ⌥↓ move the focused row, ⌥⌘↑ moves it to the top, Delete cancels (with Undo), Return opens.
- The running job can't be reordered. Move to Top makes a job next, it doesn't interrupt the running one.
- After any move, VoiceOver announces "Design review moved to position 1".

### Words: the sidebar and the queue agree

| State | Sidebar row | Queue | Change |
| --- | --- | --- | --- |
| Transcribing | Transcribing 40% | Transcribing 40% · about 4 min left | Queue drops its comma ("Transcribing, 40%" → "Transcribing 40%") |
| Light pace | 40%, light | Transcribing 40%, light · about 9 min left, and the chip | New in the queue |
| Summarizing | Summarizing | Summarizing · about 1 min left | Unchanged |
| Queued | Queued, 2 ahead | Position number, then "starts in about 15 min" | Queue adds the position and start time |
| Blocked or failed | Needs attention | Needs attention section, with the cause and the fix | "Failed" is retired. Both are "Needs attention", and the sentence says which |
| Finished | Done | Done at 14:32 | The time is added |
| Cancelled | Ready to transcribe | Cancelled at 14:05 | The sidebar no longer says "Cancelled": the recording is back to idle |

### VoiceOver

The dialog's name is its title, "Processing Queue". Each row is one button with a full sentence:

- Running: "Weekly sync, transcribing, 40%, about 4 minutes left." In light pace, add ", running light because the Mac is on battery."
- Queued: "Design review, queued, 1 of 2, 48 minutes, starts in about 4 minutes."
- Blocked: "Hiring debrief, needs attention. The transcript is ready, but summaries aren't set up." The fix button follows it.
- Finished: "Kundengespräch Müller GmbH, done at 14:32."

The running job's text updates with each percent but is only announced when the stage changes, when the job finishes or fails, and when it is cancelled. That is the same rule as the processing card.

### Empty state (Q5)

The title is "Nothing is waiting". The text is "When you stop a recording, it waits here while Dory transcribes and summarizes it. You can reorder or cancel jobs, and open the ones that just finished." The footer still has Processing Settings… and Done, and focus is on Done.

## 5. Set Up Summaries (U1–U3)

The dialog is 480 wide. Its title is "Set Up Summaries", which is also its VoiceOver name.

| State | Subtitle | Body | Footer | Focus |
| --- | --- | --- | --- | --- |
| U1, install | 2 short steps. Everything stays on this Mac. | One sentence on what Ollama is. Step 1, "Install Ollama", is active: "Not found on this Mac. Dory checks again when you come back.", with Get Ollama…, which opens the website. Step 2, "Download the summary model · Gemma 3 4B · 3.3 GB", has a dashed border and "Available after step 1" | Not Now, then Download Model, disabled until Ollama is found | Get Ollama… |
| U2, downloading | Step 2 of 2 | Step 1 shows as done ("Ollama is installed"). Step 2 shows a 6 pt progress bar (accent on track), "1.2 of 3.3 GB · about 3 min left" and Cancel Download. A note says the download continues if the dialog closes | Continue in Background | Continue in Background |
| U3, ready | none | A large check, "Summaries are ready", and "Hiring debrief is being summarized now. New recordings get a summary automatically." | Done | Done |

The progress bar is a progress indicator with the value text "1.2 of 3.3 gigabytes, about 3 minutes left". The move from U1 to U2 happens when Ollama is detected after the user comes back to the app. VoiceOver announces "Ollama found".

The model name and size are sample values. Use whatever is in the catalog.

## 6. Building it with GPUI Kit

- Always pass Dory's tokens to Kit, in every appearance, not only under Increase Contrast. `theme.rs`'s `sync` should map `surface_raised`, `surface_border`, `scrim`, `shadow_float`, the radius and the padding into Kit's dialog theme. That is the root cause of the dark-mode problem.
- Replace Kit's dialog shadow (two 10% layers) with `shadow_float`.
- The accessible name: use the dialog's visible title (Processing Queue, Set Up Summaries) as the string in the fix for `kit-dialog-has-no-accessible-name.md`.
- Move every floating surface to the same tokens: the model explainer, the combobox list, the Export, ⋯ and Filter menus, and the toasts. Delete `menu_bg`, `menu_border` and `toast_bg` so nothing can drift back.

## 7. Proposed edits

**UX-25, Materials (replace):** The main and Settings windows use a blurred window background. The sidebar column is translucent; the content pane is opaque. Everything that floats over the app (dialogs, popovers, menus, toasts) is opaque `surface_raised` with a 1 pt `surface_border`. In dark mode it is lighter than the window under it. Modal dialogs dim the window with `scrim`. Nothing uses per-element blur. Increase Contrast draws borders in `text_secondary` and removes shadows; Reduce Transparency makes the window and the scrim opaque.

**UX-15, The queue (replace):** The queue appears as status on sidebar rows. Window › Processing Queue opens a dialog with sections in this order:

1. Now: the running job, with its stage, percent, time left and mini waveform.
2. Up next: the queued jobs in order, with position, length and start time.
3. Needs attention: each blocked job with its cause and its UX-13 fix.
4. Finished this session.

The header shows the total ("3 recordings · about 24 min left"). Jobs can be reordered with Move to Top, Move Up and Move Down (menu, keyboard and VoiceOver actions) or by dragging. Cancel acts at once and offers Undo for 5 seconds. Choosing a row opens its recording.

**UX-13:** add "Set Up Summaries is a two-step dialog: install Ollama, then download the model with progress. It can close while the download continues, and the waiting recordings are summarized when it finishes."

**Handoff, section 2:** add the tokens in section 2 above, retire `menu_bg`, `menu_border` and `toast_bg`, and update dark `wave_rest` to `#7C8193`.

## Open questions

- **The light-mode border.** Meeting 3:1 against the dimmed window in light mode needs a mid-gray (`#737882`). That is heavier than macOS's own sheets, which rely on the shadow. WCAG's non-text rule asks for 3:1 on what you need to see the component. The white fill against the dimmed window, plus the shadow, already does that. The design followed the request as written. If it looks heavy in the app, keep the 3:1 border for Increase Contrast and use `#C3C7CF` in standard light mode. Dark mode still needs its border either way.
- **Should the queue be modal at all?** A modal dialog blocks the window you'd want to keep using while jobs run, and opening a recording has to close the dialog. A small non-modal Processing window (like Activity Monitor's, floating, remembered position) would let people watch the queue and browse at once. Its contents would be the same as this design. The designer would make it a window, and designed it as a dialog because the request asked for one.
- **Is Finished this session enough?** Should finished jobs survive a relaunch? The design keeps them per session because the sidebar already shows every recording's state for good.

## Operator's decisions (17 November 2026)

- **Light-mode border:** `surface_border` light is `#C3C7CF`. The 3:1 border stays for Increase Contrast, where it is `text_secondary`. Dark keeps `#777A84`.
- **The queue is a non-modal Processing window,** not a dialog: small, floating, with its position remembered, like Activity Monitor's. Its contents are section 4's, and it uses the dialog family's fill, border, radius, header and footer, without the scrim. Choosing a row selects the recording in the main window and leaves the Processing window open. **Done** becomes **Close**, and ⌘W closes the window. Window › Processing Queue opens it or brings it forward.
- **Finished this session** stays per session, as designed.
