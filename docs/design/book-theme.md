# Dory: the Book theme

Book is Dory's only theme. It has a light version ("paper") and a dark version ("night") that follow the Mac's appearance. Frames B1–B4 are on the canvas in the section "Experiment · Book theme": https://claude.ai/artifact/182Pgi1xwq6Kb5KGC618GP.

Book first shipped as a second theme next to **Modern** (direction C). On 2026-11-19 Modern was removed, along with the theme choice and the window glass, and Book became the app's look. This file extends `handoff-direction-c.md` (the token names), `floating-surfaces.md` (surface tokens), `settings-icons-blur.md` (Lucide), and `processing-view.md`. Their hex values and glass describe the retired Modern theme. When this file and those files or the canvas disagree, this file wins.

## 1. The rule: tokens plus a few presentation rules, never a second layout

Book uses the same windows, components, sizes, focus order and VoiceOver text the direction C spec defines. It sets:

- **The color tokens** (section 2).
- **The typeface** (section 3).
- **Five presentation rules** (section 4).

Views read every color from `Theme` and never hard-code one. A missing token must fail to compile, not fall back to a default.

## 2. Tokens

Contrast is measured against the surface each token sits on.

| Token | Paper (light) | Night (dark) | Notes |
|---|---|---|---|
| `window_bg` | `#FAF6EC` | `#1C1915` | Paper. Warm, but not yellow enough to look aged |
| `sidebar_bg` | `#EFE6D2` | `#24201B` | The "endpaper". Opaque: the window has no glass |
| `panel_border`, `divider` | `#DDD2BC` | `#3A342C` | |
| `text` | `#2A251F` | `#ECE3D3` | Ink: 14:1 light, 13:1 dark |
| `text_secondary` | `#6A6054` | `#B8AC97` | Light: 5.9:1 on paper, 5.1:1 on endpaper, 4.9:1 on the highlight. Dark: 7.8:1 |
| `accent`, `accent_text` | `#2E4A86` | `#9DB5E6` | An ink blue taken from the app icon. It is not red (red stays for recording) and not gold. This is Dory's own accent, shown under Multicolor (the macOS default). An accent picked in System Settings › Appearance replaces it, moved until it reads at 4.5:1 as text on every surface and under `on_accent`, and it follows a change at once |
| `on_accent` | `#FFFFFF` | `#1C1915` | Flips in dark mode, because the dark accent is light. Check every place that hard-codes white on `accent` |
| `selection_inactive`, `playing_bg` | `#F2E2A0` | `rgba(222,186,84,0.20)` | A highlighter yellow, from the icon's yellow. It marks the selected row and the playing turn |
| `card_bg` / `card_border` | `#F3ECDD` / `#E2D7C1` | `#24201B` / `#3A342C` | |
| `control_bg` / `control_border` | `#FFFDF7` / `#CDBFA4` | `#2A251F` / `#4A4238` | |
| `wave_rest` | `#8E8370` | `#7D7262` | 3.1:1 and 3.3:1 on the card. `wave_done` = `accent` |
| `record_red` | `#C62D23` | `#E5483D` | Still used only for recording |
| `attention` | `#8F5200` | `#F0B24A` | |
| `surface_raised` / `surface_border` | `#FFFDF7` / `#7A6F5E` | `#2E2924` / `#847A6A` | The border is at least 3:1 against both the surface and the dimmed window |
| `scrim` | `rgba(40,28,10,0.16)` | `rgba(0,0,0,0.5)` | |
| Increase Contrast | borders = `text_secondary`, no shadows, highlight fill plus a 1 pt `text_secondary` outline | same | |

## 3. Type

- **Every window is set in one serif, Literata** (SIL Open Font License; it was designed for reading on screens). Dory bundles it and loads it with GPUI's font loading, so it renders the same on every Mac. Since 2026-11-19 that includes everything you operate: the toolbar, buttons, search, Dory's own menus and popovers, status words, timestamps, Settings, and dialog body text. Native macOS chrome (the menu bar, window titles, the status item's menu) stays in the system font, because Dory does not draw it.
- Code-like text (model ids, paths) stays monospaced in Menlo.
- The table below sets the reading roles' sizes. Controls keep the sizes the direction C spec gives them.

| Role | Book |
|---|---|
| Recording title | Serif 38, weight 500 (not bold: a chapter title) |
| Byline (metadata) | Serif italic 16, secondary |
| Body (summary, transcript) | Serif 17, line height 1.7, measure at most 640 (about 70 characters) |
| Section headings, tabs, running head, "Contents" | Serif small caps 13–14, tracking +12%, secondary |
| Sidebar entry title | Serif 15 |
| Recording lengths in Contents | Oldstyle numerals. Times everywhere stay tabular |

At 200% text the column grows to 720 and the measure becomes the limit, not a fixed width.

## 4. Presentation rules

The window is opaque paper with no blur, so Reduce Transparency only changes the scrim.

| Rule | What it does | Reflow / accessibility |
|---|---|---|
| Running head | Above the title: the recording name on the left, the date on the right, in small caps over a hairline | Decorative (`aria-hidden`). Hidden below 900 pt window width |
| Contents leaders | Sidebar entries as "Title ...... 48 min", with dotted leaders | Leaders keep a 12 pt minimum and disappear when the title wraps (at 150% text and above). The VoiceOver label is unchanged |
| Drop cap | A 3-line initial on the summary's first paragraph, in `accent_text` | Drawn once and hidden from VoiceOver (the text keeps its letter). Off above 125% text and below 900 pt width |
| Section ornament | An asterism (⁂) between the first and second summary sections | Decorative, hidden from VoiceOver |
| Script transcript | Each turn reads like a play script: the speaker name in small caps, inline ("SAM. Okay, …"), with the timestamp hanging in the left margin | Below 900 pt window width, the timestamp moves inline before the name. The VoiceOver sentence is unchanged (A11Y-10). The playing turn keeps the word "Playing", not only the highlight |

**Content is never reshaped.** Book renders the summary Markdown as written: lists are lists, with hanging en dashes for bullets. Frame B1 shows the key points as one paragraph to explore the reading feel. Don't build it that way.

## 5. Appearance settings

- Light and dark follow the Mac. There is no theme setting and no View › Theme menu.
- **Settings › General** keeps the Text size control (A11Y-7).
- An older `ui-prefs.json` may still hold a `theme` key (and an older `book_offered` key). Both are ignored.

## 6. Risks to watch

- **"A native Mac app":** the first brief asked for a well-made Apple app. With serif in the controls too, Book stays native only through system-shaped controls and native menus. Serif at 11–13 pt reads worse than SF, so watch small labels and keep them at the sizes the spec gives.
- **Warm paper and contrast:** cream lowers contrast compared with white. Every secondary color here passes 4.5:1. Check any new token the same way before adding it.

## 7. As built (GPUI, November 2026)

Decisions and their reasons are rows in `docs/plans/decisions.tsv`. In short:

- **Shape.** `theme::Theme { tokens, … }` plus display state (appearance, Increase Contrast, Differentiate Without Color, text scale), built by `Theme::new`. Views read tokens as `theme.text_secondary` through a `Deref` to `Tokens`. The colors are one `BookPalette` literal per appearance, so a missing token does not compile.
- **Tokens the spec leaves open.** Book also sets `field_bg`, `avatar_bg`, `surface_inset`, `inset_border`, `scrim_opaque`, `close_bg`, `track`, `group_bg`, `group_border`, `radio_border`, `attention_border`, and `leader` (the canvas's dotted leaders). `tests/contrast.rs` holds each pair to its floor in light, dark, and Increase Contrast. Two values move from the canvas: light `field_bg` is `#E8DECA`, because `text_secondary` measured 4.38:1 on the canvas's tint; night `on_record` is black, because white (3.94:1) and ink (4.44:1) both miss 4.5:1 on `#E5483D`. Light `text_secondary` measures 4.96:1 on the endpaper and 4.73:1 on the highlight, not the 5.1 and 4.9 written above; both pass.
- **Type.** Literata 3.103 Regular, Italic, Medium, and SemiBold are embedded (`scripts/vendor-literata.sh`, `src/fonts.rs`) and credited in About. `theme::sync` sets `fonts::FAMILY` as GPUI Kit's font family, and Kit's window root applies it to every window, so no view sets a font itself. Bold text uses SemiBold, the heaviest face bundled. Small caps use the font's `c2sc` and `smcp`, and lengths use `onum`. GPUI has no letter-spacing, so the +12% tracking is not drawn.
- **Presentation.** `Presentation::of(window width, text scale)` applies the reflow rules. The drop cap is a 3-line initial in a column beside its paragraph (GPUI cannot float text around a box), and it appears only when the summary's first body block is a paragraph of at least 120 characters. Real summaries open with lists, so they show the asterism and no drop cap. The asterism sits before the first heading that follows body text. The script's speaker name is a small-caps run in one serif paragraph, with the same Rename button laid over it; "Playing" sits under the hanging time. Below 900 pt the time moves to a row above the turn, not inline before the name. No "Contents" label is added to the sidebar.
- **GPUI Kit.** `theme::sync` puts Kit's buttons, fields, and menus on paper and ink in both appearances.
