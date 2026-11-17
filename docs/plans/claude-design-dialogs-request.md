# Claude Design request: dialogs and popovers

A follow-up to `docs/plans/claude-design-brief.md`, in the chosen direction C. The owner found that the Processing Queue dialog doesn't stand out in dark mode: its border is too faint and it blends into the window behind it. This request asks for one designed treatment for every surface that floats over the app.

## What is wrong today

- The two modal dialogs, **Window › Processing Queue** (`apps/dory-gpui/src/queue_popover.rs`) and **Set Up Summaries** (`set_up_summaries.rs`), open through GPUI Kit's `open_dialog`. Kit draws them in its own default theme, not direction C's tokens. Dory only passes its colors to Kit under Increase Contrast (`theme.rs`, `sync`).
- Kit's dialog shadow is 10% black (two layers, 20 pt and 8 pt down). On a dark window and a dark scrim it is invisible.
- So in dark mode a dialog is a near-black panel on a near-black window, with a faint 1 pt border as its only edge.
- The other floating surfaces each made their own choice:
  - the model explainer and the language combobox popover use `menu_bg` and `menu_border`
  - toasts use `toast_bg`
  - the About window is a separate window
- Kit's dialog also gives VoiceOver no title (`docs/plans/upstream/kit-dialog-has-no-accessible-name.md`). The design should name each dialog in its title so the fix has a string to use.

## Send the request

1. Open the Dory project in Claude Design, where direction C and the processing and Settings frames live.
2. Attach these files:
   - `docs/design/handoff-direction-c.md` (tokens), `docs/design/processing-view.md` and `docs/design/settings-icons-blur.md` (the newest frames and rules).
   - `docs/plans/ux-redesign.md` (UX-15 for the queue, UX-13 for Set Up Summaries, UX-25 for materials, and A11Y-1 to A11Y-10).
   - Screenshots of the Processing Queue in light and dark, and of the Set Up Summaries sheet in dark. Take them with `screencapture -x -l <window id>` on Dory's window only.
3. Paste the request below as the next message.
4. Save the exports to `docs/design/dialogs/` and add a row to `docs/plans/decisions.tsv`.

## The request

```text
Next for Dory, in direction C: one visual treatment for everything that floats
over the app. The owner's finding: the Processing Queue dialog doesn't stand out
in dark mode. Its border is faint, its shadow can't be seen, and its fill is
almost the window's color, so it blends into the page behind it.

Surfaces to cover, as one family:
1. Modal dialogs: Processing Queue (a list of jobs with Move to Top and Cancel,
   UX-15) and Set Up Summaries (steps to install Ollama, a Download button with
   progress, then a ready state, UX-13).
2. The scrim behind a modal dialog.
3. Popovers anchored to a control: the model explainer (UX-28) and the language
   combobox list (settings-icons-blur.md section 4.3).
4. Menus (Export, the row's ⋯ menu, the Filter menu).
5. Toasts (the Undo toast after Stop and after Delete).

For each, specify: fill, border, shadow, radius, padding, the title row (dialogs),
the close control, the button row, and how it separates from the page behind it
in light and in dark. Dark mode is the case to solve first: on macOS, a floating
surface in dark mode is lighter than the window under it, not darker.

Constraints:
- GPUI draws everything: flat fills, 1 px borders, rounded rectangles, layered
  box shadows (color, offset, blur, spread), system font, Lucide icons. There is
  no per-element backdrop blur, so a dialog cannot blur the content under it
  (settings-icons-blur.md section 5). Do not imitate glass with stacked
  translucent layers.
- Motion only changes opacity, color, position, or size, and every animation has
  a Reduce Motion version.
- Increase Contrast: a solid 1 pt border in text_secondary and no reliance on
  shadow. Reduce Transparency: an opaque scrim.
- Contrast: text 4.5:1, borders and graphics 3:1 against both the surface and the
  window behind it, in light, dark, and Increase Contrast.
- Every dialog has a visible title that VoiceOver can read as its name. Focus
  starts on the first control, Escape closes, and focus returns to the control
  that opened it.
- Windows from 800 x 560 up and text from 85% to 200%. A dialog never runs off
  the window; a long one scrolls inside.
- Use direction C's tokens, and name any new token (for example surface_raised,
  surface_border, scrim, shadow_float) with light, dark, and Increase Contrast
  values.

Please deliver:
- Processing Queue with three jobs and Set Up Summaries in each of its three
  states, in light and dark at 1100 x 720, plus the queue at 800 x 560 and 200%.
- One explainer popover, one menu, and the Undo toast in the same family.
- A spec table per surface with the tokens, sizes, and shadow layers, and the
  motion for open and close with its Reduce Motion stand-in.
- Any proposed edits to UX-25 or the handoff.
```
