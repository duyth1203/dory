# Dory processing view: from Stop to summary (direction C)

This spec covers the detail pane and the sidebar row while a recording is processed. It extends `docs/design/handoff-direction-c.md`, which owns the base tokens, and `docs/plans/ux-redesign.md`, which owns behavior. Frames P1 to P8 are on the design canvas: https://claude.ai/artifact/182Pgi1xwq6Kb5KGC618GP. Each frame shows light and dark at 1100 × 720. P4 also exists at 800 × 560 and at 200% text.

Source: Claude Design, from the request in `docs/plans/claude-design-processing-request.md`.

## 1. The idea

The view shows progress on the recording itself. The card shows the meeting's own waveform. As transcription works through the audio, the bars it has finished turn from `wave_rest` to `wave_done`, and a 2 pt reading head marks where it is. Under the waveform, the caption says how far it has got in meeting time ("Transcribed up to 24:48"), not only in percent. Lines of transcript appear under the card as they are written.

At the end, the same card becomes the player. The waveform stays where it is, the step row is replaced by the transport controls, and the tabs below show the summary. Nothing jumps, because the processing view and the finished view are one layout with one card.

## 2. Anatomy and sizes

All sizes are in points at 100% text. Text sizes scale with View › Bigger/Smaller (0.85 to 2.0). Spacing, glyphs, waveform height and radii do not scale.

```text
Title                         30 bold                    ┐
Meta line                     14 secondary               │ column, max 720,
                                                         │ side padding 48
┌ Processing card ─────────────────────────────── r14 ┐  │ (24 below 900 pt
│ Headline 17 semibold              [Move to Top]     │  │ window width)
│ Detail 14 secondary                                 │  │
│ [Light, on battery] Use Fast This Time   (P5 only)  │  │
│ [Primary fix button]                     (P8 only)  │  │
│ ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮|▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯ waveform 48     │  │
│ Transcribed up to 24:48                   62:00  12 │  │
│ ─────────────────────────────────────────────────── │  │
│ ✓ Recording Saved   ◌ Transcript 40%   ◌ Summary …  │  │
└─────────────────────────────────────────────────────┘  │
Summary | Transcript           tabs, 34 tall             │
Transcript so far / placeholder / summary                ┘
```

| Element | Spec |
| --- | --- |
| Card | `card_bg`, 1 pt `card_border` (`attention_border` in P8), radius 14, padding 18 top, 20 sides, 16 bottom. Top margin 20 from the meta line. Height follows content |
| Headline row | Wraps: the text block is flex 1 1 260, and the optional right button drops below it when the column is too narrow |
| Headline | 17 semibold, line height 1.3. P8 only: an 18 pt triangle glyph in `attention` before it |
| Detail | 14, line height 1.45, `text_secondary`. P8 uses `text`, because it is the instruction |
| Waveform | 48 tall, 18 above. 110 bars (fewer on narrow windows: one bar per 6 pt of width), 2 pt gaps, radius 2. Bar heights come from the recording's peak data, computed when it is saved |
| Reading head | 2 × (48 + 8) pt in `accent`, centered on the frontier, 4 pt above and below the bars |
| Caption | 12, tabular figures, 8 above, `text_secondary`. Left: progress in meeting time. Right: total duration |
| Step row | 12 top padding over a 1 pt `card_border` rule, 14 above. Three steps that wrap, with gaps of 8 vertical and 28 horizontal. Each step is a 14 pt glyph, the name (13 semibold) and a word (13 secondary) |
| Fix button (P8) | Primary: `accent` fill, `on_accent` text, min height 30, padding 4/14, 14 above. It has focus when the failure appears |
| Light chip (P5) | Min height 22, 1 pt `control_border`, radius 11, battery glyph plus "Light, on battery" (12). The borderless "Use Fast This Time" (12 medium, `accent_text`) sits beside it |
| Tabs | As in the handoff. While summarizing, the Summary tab shows the dots glyph before its label |
| Transcript so far | List with a 42 pt timestamp column (grows to 54 pt at 200%), 15/1.6 text, 12 between lines, and a 12 pt footnote |

## 3. New tokens

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `wave_done` | `#0A63D1` (= accent) | `#4D8BF0` | Transcribed bars. In dark mode it is lighter than accent so it reaches 3:1 on `card_bg` `#212637` (≈4.6:1) |
| `attention_border` | `#E2B98A` | `#8A6220` | Card border in the failed state, next to the glyph and the words, never instead of them |
| `motion_fast` | 150 ms | | Fades of single elements |
| `motion_base` | 240 ms | | Color changes, line arrival |
| `motion_slow` | 400 ms | | Card height changes, hand-over |
| `ease_standard` | cubic-bezier(0.2, 0, 0, 1) | | Default easing |
| `pulse_period` | 1600 ms | | Breathing loops |

With Differentiate Without Color on, untranscribed bars draw as a 1 pt `wave_rest` outline with no fill, and transcribed bars stay filled. The reading head and the caption also carry position.

## 4. States, copy, and the sidebar row

One `role=status` element in the card holds the VoiceOver sentence. It is announced when the state changes, not on every percent (see section 7). Its text is always readable on demand. The visible headline and detail say the same thing.

| # | State | Headline | Detail | Steps (Recording · Transcript · Summary) | Below the card | VoiceOver sentence |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | Stopped, Undo window | Recording stopped | Transcription starts in 5 seconds. | ■ Stopped · ◌ Starts in 5 s · ◌ Waiting | Placeholder: "The transcript appears here, line by line, as it is written." The Undo toast is at the bottom of the window | Recording stopped. Transcription starts in 5 seconds. |
| P2 | Saving | Saving the recording | 62 minutes of audio. This takes a moment. | ••• Saving · ◌ Waiting · ◌ Waiting | Placeholder | Saving the recording. |
| P3a | Queued | Waiting to transcribe | 2 recordings are ahead of this one. It starts in about 9 min. | ✓ Saved · 🕒 2 ahead · ◌ Waiting | Placeholder. Headline row has Move to Top (UX-15) | Waiting to transcribe. 2 recordings ahead, starts in about 9 minutes. |
| P3b | Loading model | Getting ready to transcribe | Loading the speech model. This takes a few seconds. | ✓ Saved · ••• Preparing · ◌ Waiting | Placeholder | Getting ready to transcribe. Loading the speech model. |
| P4 | Transcribing | Transcribing | 40% · about 4 min left | ✓ Saved · ••• 40% · ◌ Waiting | Lines so far, newest last, plus the footnote "Speaker names are added when transcription finishes." | Transcribing, 40%, about 4 minutes left. |
| P4, first minute | Transcribing, no estimate yet | Transcribing | 3% · estimating time left | same | same | Transcribing, 3%. Estimating time left. |
| P5 | Transcribing light | Transcribing | 40% · about 9 min left, then the chip "Light, on battery" and Use Fast This Time | ✓ Saved · ••• 40%, light · ◌ Waiting | Lines so far | Transcribing, 40%, about 9 minutes left. Running light because the Mac is on battery. |
| P6 | Summarizing | Writing the summary | The transcript is ready to read. Summary in about 1 min. | ✓ Saved · ✓ Ready · ••• Writing | Transcript tab selected, showing the finished transcript with speaker names | Transcript ready. Writing the summary, about 1 minute left. |
| P7 | Done | none: the card becomes the player | none | none | Summary tab, unless the user is reading the Transcript tab, in which case it stays there | Summary ready for Weekly sync. Also a system notification if the window is not key (UX-14). |
| P8 | Failed (missing model) | Transcription didn't start | Your recording is saved, but the speech model isn't on this Mac. | ✓ Saved · ⚠ Needs the speech model · ◌ Waiting | Placeholder: "The transcript appears here once the speech model is installed." | Transcription didn't start. Your recording is saved, but the speech model isn't on this Mac. Download Parakeet, 487 megabytes. |

Light reasons for P5 follow UX-30: "on battery", "Low Power Mode", "Mac is warm", "while you record".

Language: the meta line shows the language once it is detected, normally within the first 30 seconds of audio ("Today at 14:00 · 62 min · English"). Before that it shows no language rather than a placeholder. The speaker count appears in P6, when the speakers are known.

### Sidebar row

The row is at least 52 tall, and it grows when text is larger. It has two lines:

- Line 1: the title (14 semibold, line height 1.25). It truncates at 125% text and below, and wraps above that.
- Line 2: the date and duration on the left ("14:00 · 62 min", 12 secondary), and the status glyph plus word on the right. The line wraps when they don't fit.

Padding is 7 top/bottom and 12 left/right, radius 8.

| State | Glyph | Word | Glyph color |
| --- | --- | --- | --- |
| P1 | stop square in circle | Stopped | secondary |
| P2 | dots in circle | Saving | `accent_text` |
| P3a | clock | Queued, 2 ahead | secondary |
| P3b | dots | Preparing | `accent_text` |
| P4 | dots | Transcribing 40% | `accent_text` |
| P5 | dots | 40%, light | `accent_text` |
| P6 | dots | Summarizing | `accent_text` |
| P7 | check in circle | Done | secondary |
| P8 | triangle | Needs attention | `attention` |

The VoiceOver label for a row is "Weekly sync, 14:00, 62 min, Transcribing 40%".

This replaces the handoff's spinning progress ring. Rotation is outside the allowed motion (position, size, opacity, color), so the dots glyph is used instead.

## 5. Motion

Every animation runs at 60 fps and changes only opacity, color, position or size. The Reduce Motion column describes what the same moment looks like with Reduce Motion on.

| Element | Property | Duration and easing | Loop | Reduce Motion stand-in |
| --- | --- | --- | --- | --- |
| Waveform arrival (P1) | Bar opacity 0 → 1, staggered left to right, 4 ms per bar | 150 ms per bar, `ease_standard`, about 600 ms total | Once | All bars appear at once |
| Bar becomes transcribed (P4, P5) | Color `wave_rest` → `wave_done` | `motion_base`, `ease_standard` | On each progress update | Instant color change |
| Reading head position | x position, interpolated between progress updates (about 1 per second) | Linear, over the measured update interval | Continuous while transcribing | Jumps to each new position with no tween |
| Reading head breathing | Opacity 1 → 0.45 → 1 | `pulse_period`, ease-in-out | Infinite. Paused while the window is hidden | Solid, no breathing. The dots glyph and the percent still show activity |
| Dots glyph (row, step, Summary tab) | Opacity of the three dots, each 0.3 → 1 → 0.3, offset by a third of the period | `pulse_period`, ease-in-out | Infinite | All three dots at full opacity, static. The word ("Saving", "40%") carries the state |
| Headline and detail change | Old text opacity 1 → 0 (`motion_fast`), then new text 0 → 1 (`motion_fast`) | `ease_standard` | Once per state change | Instant swap |
| Card height change (button or chip appears or goes) | Height | `motion_slow`, `ease_standard` | Once | Instant |
| New transcript line | Opacity 0 → 1 and y +4 → 0 | `motion_base`, `ease_standard` | Once per line | Appears instantly. The list is still a polite live region, so lines are not announced one by one |
| Step completes | Glyph cross-fade (dots → check), color to secondary | `motion_fast` | Once | Instant |
| Hand-over (P6 → P7) | Step row fades out (`motion_fast`). Transport fades in (`motion_base`) in the same slot. Bars fade from `wave_done` to `wave_rest` except the played part (`motion_slow`). Card height animates to the player's height (`motion_slow`) | `ease_standard` | Once | Instant swap to the player. Focus does not move |
| Failure (→ P8) | Border color to `attention_border`, then the button fades in, with a height change | `motion_base`, then `motion_slow` | Once | Instant. Focus moves to the fix button in both cases |

Pause all loops when the window is occluded or the app is hidden. Nobody is watching them, and they wake the GPU.

## 6. Reflow (800 × 560 up, text 85% to 200%)

- 800 × 560: the sidebar drops to 240 (its minimum), the column padding to 24, and the content width to about 500. The headline row's right button moves under the text. The step row wraps to two lines when needed. The waveform loses bars (one per 6 pt) instead of thinning them.
- 200% text: nothing has a fixed height except the waveform and the 2 pt rules. Rows grow past 52. Titles wrap. The detail and the step row wrap. The detail pane scrolls as one surface (handoff section 3), and the docked player rule applies once the card scrolls away.
- Nothing truncates the status word. The status is the information, so the title gives up space first.
- Check every state at 800 × 560 × 200% in snapshot tests. That is the worst case, and it is not on the canvas.

## 7. Proposed spec edits

### UX-12. Progress is specific (replace the text)

UX-12. Progress is specific. The selected recording's detail pane shows one processing card from Stop until the summary is ready. The card shows the recording's waveform, which fills in as transcription passes through the audio, with a caption in meeting time ("Transcribed up to 24:48") and a step row for Recording, Transcript, and Summary. The stage words, the sidebar row words, and the VoiceOver sentences are the ones in `docs/design/processing-view.md` section 4.

- Time left appears once the first minute of audio is transcribed. Before that the card says "estimating time left". The estimate is rounded up to whole minutes, and shows "less than 1 min" below 60 seconds. It updates at most every 10 seconds. It only goes down, unless the new estimate is more than 30% higher, in which case it may go up once. The estimate uses the current pace (UX-30).
- Queue position shows the number of recordings ahead and, when they have estimates, when this one starts.
- Transcript lines for this recording stream into the Transcript tab as they are written, without speaker names until the job finishes.
- Announcements. VoiceOver announces the card's sentence when the stage changes, when processing fails, and when it is done. Percent changes update the sentence without announcing it.
- One card, no jumps. When processing finishes, the card becomes the player in place (UX-2).

The streaming lines are already in UX-12 today ("The transcript tab streams lines for that recording only"). They are a committed requirement, not the optional UX-27 feature. UX-27 is about lines during recording.

### UX-13. Failures say what to do (replace the text)

UX-13. Failures say what to do. A failed job shows one sentence and one button. When the audio is safe, the sentence says so first ("Your recording is saved, but …"). The button fixes the cause or retries. After the fix, the same card continues from where it stopped, without a new click. When the summary step fails, the transcript stays readable, and only the Summary tab shows the failure.

| Cause | Sentence | Button |
| --- | --- | --- |
| Speech model missing | Your recording is saved, but the speech model isn't on this Mac. | Download Parakeet (487 MB). It then shows "Downloading Parakeet, 120 of 487 MB" in the same card |
| Summary model missing | The transcript is ready, but summaries aren't set up. | Set Up Summaries |
| Disk full | Your recording is saved, but there isn't enough space to transcribe it. | Free Up Space… (opens Settings › Storage) |
| Transcriber crashed | Transcription stopped partway. Your recording is saved. | Try Again |
| Audio unreadable | This recording's audio can't be read. | Show in Finder |

The row shows "Needs attention" with the triangle glyph. Its VoiceOver sentence is the sentence plus the button's label.

### UX-30. Short form in the row (add)

The sidebar row uses a short form, "40%, light", because "Transcribing, 40% · Light, on battery" does not fit at 52 pt. The full reason shows in the detail pane, which also offers Use Fast This Time to run this one job fast without changing the setting.

### Handoff section 4 (amend)

Status glyphs: replace the indeterminate progress ring with the dots glyph. Rotation is not part of the allowed motion. The sidebar row is two lines and at least 52 tall: the title, then the date and duration with the status. It replaces the earlier three-line row.

## 8. Open questions

- **Playback while processing.** The card has no transport until P7, so someone can't replay a moment while it transcribes. Adding a Play button would mean the waveform shows two things at once (transcription progress and playback position). Left out. If people ask for it, add Play and show the playback position as a text-colored line, separate from the accent reading head.
- **Use Fast This Time is new.** Confirm that you want a per-job override next to the UX-30 setting.
- **Time-left accuracy.** The rules in UX-12 smooth the estimate, but test them on real 12–94-minute recordings before shipping. An estimate that goes up again is worse than "estimating".
