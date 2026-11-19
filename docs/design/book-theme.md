# Dory: Book theme (experimental), a second theme next to Modern

Dory gets two themes: **Modern** (direction C, the default) and **Book**. Each has a light and a dark version that follow the Mac's appearance. Frames B1–B4 are on the canvas in the section "Experiment · Book theme": https://claude.ai/artifact/182Pgi1xwq6Kb5KGC618GP.

This file extends `handoff-direction-c.md` (Modern's tokens), `floating-surfaces.md` (surface tokens), `settings-icons-blur.md` (glass and Lucide), and `processing-view.md`. When this file and the canvas disagree, this file wins.

## 1. The rule: a theme is tokens plus a few flags, never a second layout

Book uses the same windows, components, sizes, focus order and VoiceOver text as Modern. It changes:

- **The color tokens** (section 2).
- **The reading typeface** (section 3).
- **Six presentation flags** (section 4).

Anything that needs a different layout is out of scope. Each flag must also be testable on its own, because every screen is now 2 themes × 2 appearances × Increase Contrast, which is four times the QA of one theme. Only the flags keep that affordable.

Implement it as `Theme { tokens, reading_font, flags }`. `Theme::modern(appearance)` and `Theme::book(appearance)` are its two constructors. Book fills in every token the handoff defines, including the floating-surface tokens. A missing token must fail a test, not silently fall back to Modern's value.

## 2. Tokens

Contrast is measured against the surface each token sits on.

| Modern token | Book light ("paper") | Book dark ("night") | Notes |
|---|---|---|---|
| `window_bg` | `#FAF6EC` | `#1C1915` | Paper. Warm, but not yellow enough to look aged |
| `sidebar_bg` | `#EFE6D2` | `#24201B` | The "endpaper". Opaque: Book turns the glass off (section 4) |
| `panel_border`, `divider` | `#DDD2BC` | `#3A342C` | |
| `text` | `#2A251F` | `#ECE3D3` | Ink: 14:1 light, 13:1 dark |
| `text_secondary` | `#6A6054` | `#B8AC97` | Light: 5.9:1 on paper, 5.1:1 on endpaper, 4.9:1 on the highlight. Dark: 7.8:1 |
| `accent`, `accent_text` | `#2E4A86` | `#9DB5E6` | An ink blue taken from the app icon. It is not red (red stays for recording) and not gold |
| `on_accent` | `#FFFFFF` | `#1C1915` | Flips in dark mode, because the dark accent is light. Check every place that hard-codes white on `accent` |
| `selection_inactive`, `playing_bg` | `#F2E2A0` | `rgba(222,186,84,0.20)` | A highlighter yellow, from the icon's yellow. It marks the selected row and the playing turn |
| `card_bg` / `card_border` | `#F3ECDD` / `#E2D7C1` | `#24201B` / `#3A342C` | |
| `control_bg` / `control_border` | `#FFFDF7` / `#CDBFA4` | `#2A251F` / `#4A4238` | |
| `wave_rest` | `#8E8370` | `#7D7262` | 3.1:1 and 3.3:1 on the card. `wave_done` = `accent` |
| `record_red` | `#C62D23` | `#E5483D` | Still used only for recording |
| `attention` | `#8F5200` | `#F0B24A` | |
| `surface_raised` / `surface_border` | `#FFFDF7` / `#7A6F5E` | `#2E2924` / `#847A6A` | Same rule as Modern: the border is at least 3:1 against both the surface and the dimmed window |
| `scrim` | `rgba(40,28,10,0.16)` | `rgba(0,0,0,0.5)` | |
| Increase Contrast | borders = `text_secondary`, no shadows, highlight fill plus a 1 pt `text_secondary` outline | same | |

## 3. Type

- **The reading typeface is serif.** Bundle **Literata** (SIL Open Font License; it was designed for reading on screens). Load it with GPUI's font loading so it renders the same on every Mac. Apple's New York is the alternative, but it's a system font reached through the system's serif design rather than by family name. Check that GPUI can address it before relying on it.
- **Everything you operate stays in SF Pro:** the toolbar, buttons, search, menus, status words, timestamps, Settings, and dialog body text. Serif at 11–13 pt in controls reads worse, and it would stop Dory feeling like a Mac app.

| Role | Book |
|---|---|
| Recording title | Serif 38, weight 500 (not bold: a chapter title) |
| Byline (metadata) | Serif italic 16, secondary: "Thursday, 12 November 2026, at 10:30 · 48 minutes · in English, with Sam, Priya and Alex" |
| Body (summary, transcript) | Serif 17, line height 1.7, measure at most 640 (about 70 characters) |
| Section headings, tabs, running head, "Contents" | Serif small caps 13–14, tracking +12%, secondary |
| Sidebar entry title | Serif 15 |
| Recording lengths in Contents | Oldstyle numerals. Times everywhere stay tabular |

At 200% text the column grows to 720 and the measure becomes the limit, not a fixed width.

## 4. Book's presentation flags

| Flag | What it does | Reflow / accessibility |
|---|---|---|
| `glass` off | The window is opaque paper; no blur | Reduce Transparency changes nothing in Book |
| `running_head` | Above the title: the recording name on the left, the date on the right, in small caps over a hairline | Decorative (`aria-hidden`). Hidden below 900 pt window width |
| `toc_leaders` | Sidebar entries as "Title ...... 48 min", with dotted leaders | Leaders keep a 12 pt minimum and disappear when the title wraps (at 150% text and above). The VoiceOver label is unchanged |
| `drop_cap` | A 3-line initial on the summary's first paragraph, in `accent_text` | Drawn once and hidden from VoiceOver (the text keeps its letter). Off above 125% text and below 900 pt width |
| `section_ornament` | An asterism (⁂) between the first and second summary sections | Decorative, hidden from VoiceOver |
| `script_transcript` | Each turn reads like a play script: the speaker name in small caps, inline ("SAM. Okay, …"), with the timestamp hanging in the left margin | Below 900 pt window width, the timestamp moves inline before the name. The VoiceOver sentence is unchanged (A11Y-10). The playing turn keeps the word "Playing", not only the highlight |

**Content does not change between themes.** Book renders the same summary Markdown: lists are lists, with hanging en dashes for bullets. Frame B1 shows the key points as one paragraph to explore the reading feel. A theme must not reshape the model's output, so don't build it that way.

## 5. Choosing a theme

- **Settings › General › Theme** (frame B4): two picture cards, **Modern** and **Book**. Each has a preview with "Aa" in its own typeface and a one-line description. Below them, a note says light and dark follow the Mac, and there's the Text size control (A11Y-7).
- It is also in **View › Theme › Modern / Book** (UX-21).
- Switching cross-fades the window in 200 ms; with Reduce Motion it switches at once. The choice is remembered.
- The default is Modern. Book first shipped behind an off-by-default Try the Book theme setting. Since 2026-11-19 Settings shows Book next to Modern from the start, so a user can see and pick it without first finding a hidden step.

## 6. Proposed spec edits

- **UX-20:** add "Two themes, **Modern** (default) and **Book**. A theme changes colors, the reading typeface, and the presentation flags in `book-theme.md`; it never changes layout, controls, or what any text says. Light and dark follow the system in both."
- **UX-25:** add "Book uses an opaque window: no glass."
- **Handoff, section 2:** every token gets a Book light and Book dark value, and a test fails if any is missing.

## 7. Risks to watch

- **QA cost:** the number of combinations to test quadruples. Snapshot every screen in both themes; flags keep Book cheap only if they stay few.
- **"A native Mac app":** the first brief asked for a well-made Apple app. Book stays native only because its controls stay SF and system-shaped. Don't let serif spread into controls.
- **Warm paper and contrast:** cream lowers contrast compared with white. Every secondary color here passes 4.5:1. Check any new token the same way before adding it.

## 8. As built (GPUI, November 2026)

Decisions and their reasons are rows in `docs/plans/decisions.tsv`. In short:

- **Shape.** `theme::Theme { name, tokens, reading_font, flags, … }`, built by `Theme::modern` and `Theme::book`. Views read tokens as `theme.text_secondary` through a `Deref` to `Tokens`. Book's colors are one `BookPalette` literal per appearance, so a missing token does not compile, and `theme::tests::book_sets_every_token_itself` fails when a Book token equals Modern's. Modern is pinned to its pre-theme values by `tests/golden/modern-tokens.txt`.
- **Tokens the spec leaves open.** Book also sets `field_bg`, `avatar_bg`, `surface_inset`, `inset_border`, `scrim_opaque`, `close_bg`, `track`, `group_bg`, `group_border`, `radio_border`, `attention_border`, and `leader` (the canvas's dotted leaders). `tests/contrast.rs` holds each pair to its floor in light, dark, and Increase Contrast. Two values move from the canvas: light `field_bg` is `#E8DECA`, because `text_secondary` measured 4.38:1 on the canvas's tint; night `on_record` is black, because white (3.94:1) and ink (4.44:1) both miss 4.5:1 on `#E5483D`. Light `text_secondary` measures 4.96:1 on the endpaper and 4.73:1 on the highlight, not the 5.1 and 4.9 written above; both pass.
- **Type.** Literata 3.103 Regular, Italic, Medium, and SemiBold are embedded (`scripts/vendor-literata.sh`, `src/fonts.rs`) and credited in About. Small caps use the font's `c2sc` and `smcp`, and lengths use `onum`. GPUI has no letter-spacing, so the +12% tracking is not drawn. The byline keeps Modern's words in serif italic 16; the example sentence in section 3 is not used, because a theme never changes what text says.
- **Flags.** `Presentation::of(flags, window width, text scale)` applies the reflow rules, and each flag has its own unit test. The drop cap is a 3-line initial in a column beside its paragraph (GPUI cannot float text around a box), and it appears only when the summary's first body block is a paragraph of at least 120 characters. Real summaries open with lists, so they show the asterism and no drop cap. The asterism sits before the first heading that follows body text. The script's speaker name is a small-caps run in one serif paragraph, with the same Rename button laid over it; "Playing" sits under the hanging time. Below 900 pt the time moves to a row above the turn, not inline before the name. No "Contents" label is added to the sidebar.
- **Choosing.** Settings › General › Theme has the Modern and Book cards (a radio group, the current one selected), the system note, and Text size. View › Theme lists both. The choice lives in `ui-prefs.json` (`theme`); an older file's `book_offered` key is ignored. The switch fades a veil of the old window color over 200 ms rather than blending two frames, and it switches at once under Reduce Motion.
