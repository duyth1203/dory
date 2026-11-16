# Claude Design request: the processing states

A follow-up to `docs/plans/claude-design-brief.md`, in the chosen direction C. The detail pane's processing view, from Stop to a finished summary, is the screen people watch most after the recording itself, and today it is a text line over a thin bar. This request asks for its designed states.

## Send the request

1. Open the Dory project in Claude Design, where direction C lives.
2. Attach these files:
   - `docs/design/handoff-direction-c.md` and `docs/design/canvas-direction-c.html` (the chosen direction).
   - `docs/plans/ux-redesign.md` (UX-9, UX-11, UX-12, UX-13, UX-27, UX-30, and A11Y-1 to A11Y-10 apply).
   - Screenshots of the current interim view: one just after Stop ("Saving the recording"), and one mid-transcription ("Transcribing, 40%"). Take them with `screencapture -x` in the app.
3. Paste the request below as the next message.
4. Save the exports to `docs/design/processing-states/` and add a row to `docs/plans/decisions.tsv`.

## The request

```text
Next screen for Dory, in direction C: the detail pane while a recording is
processed, from the moment the user presses Stop until its summary is ready.

Why it matters: after a call, people watch this view. Today it reads as a plain
text line ("Saving the recording", "Starting transcription", "Transcribing,
40%") over a 4 pt bar, with a three-step strip above it (Recorded ✓, Transcript
●, Summary ○). It works, but it looks like a loading spinner, not like the app
is doing careful work on their meeting. Make it feel calm, specific, and alive,
and make it say what is happening and how long is left.

The sequence to design, as one continuous view (no jump between states):
1. Stopped, Undo window (5 s): the take is saved, processing starts when the
   window ends. An Undo toast already shows at the bottom of the window; this
   view should not repeat it, but may show that transcription starts shortly.
2. Saving the take (about a second).
3. Waiting: queued behind other recordings ("2 ahead of this one"), or loading
   the speech model (a few seconds).
4. Transcribing: a percent, and after the first minute of audio an estimate
   ("about 4 min left"). Optional later: the transcript lines appear as they are
   written (UX-27 is proposed, not built), so leave room for them.
5. Running light: the same as 4, with the reason ("Light, on battery"), UX-30.
6. Summarizing: the transcript is ready and readable; the summary is being
   written. This state may live in the Summary tab instead of this view.
7. Done: the view hands over to the Summary and Transcript tabs.
8. Failed: one sentence and one fix button (UX-13), for example "The speech
   model isn't on this Mac. Download Parakeet (487 MB)".

Also design the matching sidebar row for each state (the row is 52 pt tall and
shows title, date, and one status: glyph plus word).

Constraints:
- GPUI draws everything: flat fills, 1 px borders, rounded rectangles, system
  font, SF Symbols-style glyphs. Motion can change position, size, opacity, and
  color only, at 60 fps. No Lottie, video, blur, or particle effects.
- Every animation needs a Reduce Motion version that conveys the same state
  without movement.
- Increase Contrast and Differentiate Without Color: no state may rely on color
  alone; each needs a glyph or word.
- VoiceOver reads one status line per change, so the view needs one sentence
  that describes the state in full ("Transcribing, 40%, about 4 minutes left").
- Light and dark, text from 85% to 200%, windows from 800 x 560 up. Nothing may
  be clipped or spill out of its container at any of those sizes.
- Use direction C's tokens (handoff section 2); name any new token you need.

Please deliver:
- Each state above, light and dark, at 1100 x 720, plus the Transcribing state
  at 800 x 560 and at 200% text.
- Specs: sizes, spacing, tokens, and the motion for each animated element
  (property, duration, easing, loop), with its Reduce Motion stand-in.
- The copy for every state, including the VoiceOver sentence.
- Anything in UX-12 or UX-13 you would change, as proposed spec edits.
```
