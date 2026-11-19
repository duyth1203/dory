# Dory: Settings fixes, icon library, window blur

A brief for the Claude Code session working in the Dory repo. It answers the owner's review of the Settings window from 17 November 2026. It extends `docs/design/handoff-direction-c.md` (tokens) and `docs/plans/ux-redesign.md` (behavior). Mockups S1 to S4 are on the design canvas: https://claude.ai/artifact/182Pgi1xwq6Kb5KGC618GP.

Owner's review, item by item:

- Settings should use the macOS System Settings layout, with the panes listed in a left sidebar. See section 1.
- Use one FOSS, lightweight icon library across the whole app. See section 2.
- The Microphone radio buttons don't line up with the radio buttons in Transcription. See section 3.
- Transcription › Language must reflect Meeting languages (section 4.1). Scrolling the language list then the page gets stuck (section 4.2). The long list should become a searchable combobox with chips (section 4.3).
- Blur or glass, if possible. See section 5.

## 1. Settings window layout (S1, S3, S4)

Use the System Settings layout, built from the direction C parts. The window is one window with a sidebar of panes. Do not use a tab bar.

| Part | Spec |
| --- | --- |
| Window | 860 × 720 by default. Resizable in height only, minimum 560. Width is fixed, like System Settings. Opens with ⌘, and restores the last pane |
| Sidebar | 220 wide, a panel inset 8 pt, radius 12. It shows the traffic lights, a search field (28 tall), then the panes General, Recording, Transcription, Summaries, Storage. Each pane row is at least 32 tall: a 22 pt icon tile (radius 6), then the label (13). The selected row has an accent fill and white text, and its icon tile is white at 22% opacity |
| Sidebar search | Filters panes and setting labels. Each result jumps to its pane and briefly highlights the row: a 1 s accent 12% fill, or a 2 pt outline with Reduce Motion |
| Content toolbar | 52 tall, the pane title (15 semibold) and a 1 pt divider underneath. Nothing else |
| Content | Scrolls as one surface (see section 4.2), with padding 20 top and 28 sides |
| Section header | 13 semibold, `text_secondary`, 4 pt indent, 8 above the group |
| Group | `group_bg`, 1 pt `group_border`, radius 10. Rows are separated by 1 pt divider lines that span the full row |
| Row | Padding 11 top/bottom, 14 sides. The label is 13 medium, and the description is 12 secondary, line height 1.4, below it. Controls sit on the right, aligned to the label's first line. Each description names the effect in one line (UX-20) |

New tokens: `group_bg` is `#F7F8FA` light and `#212226` dark. `group_border` is `#E3E6EB` light and `#34353B` dark.

## 2. Icon library: Lucide

Choice: Lucide (https://lucide.dev).

- License: ISC, plus MIT for the icons that come from Feather. Both are fine to use and ship in an app. Ship the license text in About › Acknowledgements.
- Style: a consistent 24 × 24 outline set with 2 pt strokes. It sits close to SF Symbols' regular weight at 14–16 pt.
- Already used with GPUI: gpui-component's Icon is built on Lucide. Its icon files live in a separate gpui-component-assets crate, so they don't add weight unless you choose to include them.
- Why not SF Symbols: GPUI draws everything itself, so we'd have to bundle the symbols as image files. Apple's license restricts how SF Symbols may be redistributed, so don't bundle them.
- Phosphor and Tabler (both MIT) were the alternatives. Lucide wins because the GPUI ecosystem already uses it.

Keep it light: vendor only what we use.

- Copy each SVG Dory uses into `assets/icons/` (about 40 files, each well under 1 KB).
- Add an `Icon` enum with one variant per file, and render each with GPUI's `svg()` element, tinted by `text_color`.
- Pin the Lucide version in `assets/icons/VERSION`. Lucide renames icons over time: `alert-triangle` became `triangle-alert`, for example.
- Don't take a dependency on the whole set.

Sizes: 12 in rows and status, 14 in buttons and tabs, 16 default, 18–22 in transport controls. Use stroke 2 at 16 and below. Lucide's SVGs set the stroke width in the file, so ship 1.5-stroke copies only for the 20–22 pt transport icons.

| Use | Lucide name |
| --- | --- |
| Search | `search` |
| Filter | `list-filter` |
| Record dot | none: a drawn circle in `record_red`, not an icon |
| Pause, stop, play | `pause`, `square`, `play` |
| Skip back 5 s, forward 5 s | `rotate-ccw` and `rotate-cw`, with the "5" drawn in text on top |
| Microphone | `mic` |
| System audio | `volume-2` |
| Done, saved | `circle-check` |
| Needs attention | `triangle-alert` |
| Queued | `clock` |
| Activity (processing) | none: three drawn dots, see the processing spec |
| Language | `languages` |
| Speakers | `users` |
| Date, duration | `calendar`, `clock` |
| Copy | `copy` |
| Export | `share` |
| More | `ellipsis` |
| Info | `info` |
| Remove chip | `x` |
| Add | `plus` |
| Popup button | `chevrons-up-down` |
| Menu check | `check` |
| Download | `download` |
| Show in Finder | `folder-open` |
| Move to Trash | `trash-2` |
| Rename | `pencil` |
| General pane | `settings-2` |
| Recording pane | `mic` |
| Transcription pane | `captions` |
| Summaries pane | `file-text` |
| Storage pane | `hard-drive` |
| Assistants pane | `bot` |
| Advanced pane | `sliders-horizontal` |
| Battery (Light) | `battery-medium` |

Replace every hand-drawn glyph in the app with this set in one pass, so the two styles never mix in a build. The canvas mockups still use the older hand-drawn glyphs except in Settings. They will be updated, but this table is the reference until then.

## 3. One radio component (S1 and S3)

The mismatch comes from two separately built radio lists. Build one `RadioGroup` component and use it in Recording (Microphone, System audio), Transcription (Speech model, Processing) and anywhere else a radio appears.

| Metric | Value |
| --- | --- |
| Row | Padding 11 top/bottom, 14 sides. Full width inside the group, separated by divider lines |
| Radio | 16 × 16 circle. 1 pt `radio_border` (`#A9ADB6` light, `#5A5B63` dark) on `control_bg`. Selected: accent fill and border, with a 6 pt white dot |
| Radio position | 14 from the row's left edge. Vertically aligned to the label's first line, not the row's center: the label line is 18 tall, so the circle sits 1 pt below the row's top padding. Centering on the row is the likely cause of the mismatch, because rows with a description are taller |
| Label | Starts 10 pt after the radio. 13 medium, line height 18 |
| Description | 12 secondary, 1 below the label, aligned to the label's left edge, never under the radio |
| Trailing control | Optional (the speech model's info button). 24 × 24, aligned to the label's first line |
| Hit area | The whole row selects the option, not only the circle |
| Keyboard | The group is one Tab stop. Arrow keys move the selection. The focus ring goes on the radio (2 pt accent, offset 2). Space selects |
| VoiceOver | "Microphone, radio group" and then "Automatic, selected, 1 of 4" |

Check it with a snapshot test that renders both panes and asserts that every radio's x position, and its y offset from its label, are equal.

## 4. Transcription › Languages

### 4.1 Language follows Meeting languages (S1)

There is one source of truth: the meeting languages list. The Language popup is computed from it, never stored separately.

- Options: Automatic, a separator, then each meeting language in the list's order. Nothing else.
- Automatic's description names the languages: "Automatic detects which of English, German or French is spoken, and follows switches mid-meeting." With one meeting language it reads "Uses English."
- Removing the selected language: if the user removes a meeting language that Language is set to, Language goes back to Automatic. A one-line note appears under the row for this session: "German was removed from meeting languages, so Language is back to Automatic."
- Removing the last language: the list must keep at least one. The last chip's remove button is disabled, with the help text "Keep at least one meeting language."
- Per-recording override (UX-26) offers the same options, computed when it opens.
- Model rows recompute their coverage text and their "Doesn't support …" warnings (UX-28) whenever the list changes.

Unit-test this as a pure function: (meeting_languages, stored_choice) → (options, effective_choice, note).

### 4.2 Scrolling gets stuck

The page doesn't scroll back up because the long language list is a scroll area inside the page's scroll area. The inner list keeps taking wheel events after it reaches its top, so they never reach the page.

1. Remove the nested scroll. The combobox in 4.3 replaces the inline list. After that, Settings content has exactly one scroll area: the page. Make "no nested scroll areas in Settings" a rule.
2. Make scroll areas pass leftover scrolling to their parent. This is for scroll areas that can't be removed, such as the combobox's own list or a long transcript in the main window. When an area is at its top and the wheel moves up, or at its bottom and the wheel moves down, it must leave the event unhandled (not stop propagation) so the parent scrolls. Trackpad momentum that begins inside the inner area should stay with it until the gesture ends, as in AppKit. Only the next gesture moves to the parent.
3. Regression test: put the pointer over an inner scroll area, scroll it to the bottom, then send upward wheel events past its top. The inner area should reach its top, and then the page should scroll.

### 4.3 Meeting languages combobox (S1, S2)

Closed (S1):

- A field at least 36 tall, with a 1 pt `control_border`, radius 8 and 4/6 padding.
- It shows chips in the order they were added. Each chip is 26 tall, radius 13, `chip_bg` (= `avatar_bg`), with a 13 pt label and a 20 × 20 remove button ("Remove German").
- After the chips comes a borderless **+ Add Language** button. The field wraps to more lines as chips are added.
- Clicking anywhere in the field that isn't a chip opens the popover.

Open (S2):

- The popover is anchored 6 pt under the field and is as wide as the field. It has `menu_bg`, a 1 pt `menu_border`, radius 10, padding 6 and the menu shadow.
- It does not use blur (see section 5).
- At the top is a search input, 30 tall, which has focus with the ring. A match count sits on its right ("4 matches").
- Below it is a list with rows 28 tall: a checkmark column (16), the English name, then the language's own name in secondary text on the right.
- The list shows at most 8 rows, then scrolls inside the popover. That inner scroll area follows rule 4.2.2.
- At the bottom is a hint line in 11 pt: "Return adds or removes · Esc closes · Delete removes the last chip".

Behavior:

| Input | Effect |
| --- | --- |
| Typing | Filters as you type, ignoring case and accents. It matches the English name, the language's own name and the ISO code: "ge" finds German, Georgian, Swiss German and Low German, and "de" finds German. The best match is highlighted |
| ↑ / ↓ | Move the highlight. It wraps |
| Return or click | Adds the highlighted language as a chip, or removes it if it's already chosen. The popover stays open so several languages can be picked, and the search text clears |
| Delete in an empty search | Removes the last chip, and VoiceOver announces "German removed" |
| Esc, click outside, or Tab | Closes the popover. Focus returns to the field |
| No matches | "No language matches “xyz”." |

Selected languages stay in their alphabetical place with a check. They are not pinned to the top, so the list doesn't jump while you pick.

Accessibility: the field is a combobox, the list is a multi-select listbox, and each row is an option with its selected state. Chips are buttons named "Remove German". At 200% text, the chips and rows grow taller and wrap. They never truncate a language name.

## 5. Blur and glass: what GPUI can do

It can blur the whole window. GPUI's `WindowBackgroundAppearance::Blurred` makes the window background transparent and blurs the desktop behind it, through the system's own blur view on macOS. Use it for the main window and Settings:

- Set the window to Blurred.
- Draw the sidebar column, including its 8 pt margins, in a translucent tint, so the blurred desktop shows through: `glass_tint` is `rgba(236,239,245,0.55)` light and `rgba(30,31,36,0.55)` dark. On top of that, the sidebar panel adds `sidebar_tint`: `rgba(255,255,255,0.28)` light, 0.05 dark.
- Draw the content pane opaque in `window_bg`. Reading text stays on solid ground.

It can't blur one element over another. There is no per-element backdrop blur in GPUI, so popovers, menus, toasts and the docked player can't blur the app content under them. Draw them with solid fills. Do not imitate glass with stacked translucent layers. It looks muddy and fails contrast.

Liquid Glass (macOS 26): GPUI doesn't use Apple's glass views. It may be possible to place one behind the GPUI view, but that means custom AppKit code in the platform adapter. Treat it as a later experiment, not part of this work.

Rules:

- Contrast on glass: text on the sidebar must pass 4.5:1 against the worst case: a plain white or plain black wallpaper, and a saturated photo. Test all three. If secondary text fails, raise `sidebar_tint` before touching text colors. Behind sidebar text, the layers should add up to at least about 0.7 opacity.
- Reduce Transparency, a separate macOS setting from Increase Contrast, switches the window to Opaque and the sidebar to solid `sidebar_bg`. Increase Contrast does the same and also adds borders (A11Y-6). Read both at launch and when they change.
- Inactive window: when the window is not key, macOS dims the blur. Keep the tints the same and let the system blur change.

## 6. Proposed spec edits

Applied to `docs/plans/ux-redesign.md` on 17 November 2026.

- UX-19: "A native Settings window opens with Command-Comma. It uses the System Settings layout: a sidebar with the panes General, Recording, Transcription, Summaries, and Storage, and a search field that finds settings by name."
- UX-25: "The main and Settings windows use a blurred window background. The sidebar column is translucent, and the content pane is opaque. Popovers and menus are opaque."
- UX-26: add "The Language options are Automatic plus the meeting languages (UX-29), and nothing else. When the selected language is removed from the meeting languages, Language returns to Automatic and says so."
- UX-29: add "Meeting languages is a combobox with chips and type-ahead search. The list keeps at least one language."
- A11Y-6: add "Reduce Transparency switches the windows to an opaque background and a solid sidebar."
- New UX-31, Scrolling: "Settings content has one scroll area. Any inner scroll area passes scrolling to its parent when it reaches its edge."

## 7. Done when

- [ ] Settings shows the five panes in the sidebar. ⌘, restores the last pane.
- [ ] Every icon comes from `assets/icons/` (Lucide), and the Lucide license is in Acknowledgements.
- [ ] The radio snapshot test passes for Recording and Transcription.
- [ ] The language function's unit tests pass, including removing the selected language and the last language.
- [ ] The scrolling regression test passes in Settings and in the transcript.
- [ ] The combobox works with the keyboard alone, and VoiceOver reads the chips and options.
- [ ] Window blur works. Reduce Transparency and Increase Contrast each switch to opaque. Sidebar text passes contrast on white, black and photo wallpapers.
