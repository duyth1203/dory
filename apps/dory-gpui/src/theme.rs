//! Dory's look, Book (`docs/design/book-theme.md`), as one `Theme` kept as a GPUI global that
//! follows the system appearance, the display accessibility settings (`system_prefs`), and View >
//! Bigger and Smaller.
//!
//! Views read it with `Theme::get(cx)` and never hard-code a color or a text size. Every color
//! pair the views paint is listed in [`Theme::pairs`], and `tests/contrast.rs` holds each one to
//! its WCAG floor in every variant.

use gpui_kit::{App, Global, Hsla, Pixels, Rgba, WindowAppearance, px, rgb};

use crate::library::Tone;

/// Every color a view paints, in one appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub window_bg: Hsla,
    pub sidebar_bg: Hsla,
    pub panel_border: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub divider: Hsla,
    pub field_bg: Hsla,
    pub control_bg: Hsla,
    pub control_border: Hsla,
    pub card_bg: Hsla,
    pub card_border: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    /// The 2 pt focus ring, `accent`. Increase Contrast draws it in `text`, which Kit's 50% halo
    /// around a focused field then keeps strong, where an accent halo washed out to pale blue.
    pub ring: Hsla,
    pub on_accent: Hsla,
    pub selection_inactive: Hsla,
    pub wave_rest: Hsla,
    /// Bars transcription has passed (processing view): `accent`, moved until it reaches 3 to 1
    /// on the cards the bars sit on.
    pub wave_done: Hsla,
    /// The waveform's empty track, under the bars.
    pub wave_track: Hsla,
    pub record_red: Hsla,
    /// Text and the dot on the `record_red` pill.
    pub on_record: Hsla,
    pub record_glyph: Hsla,
    pub attention: Hsla,
    /// The processing card's border when it failed: always next to the triangle and the words,
    /// never instead of them. Increase Contrast draws it in `attention`.
    pub attention_border: Hsla,
    /// Text on the `attention` pill when 5 minutes are left.
    pub on_attention: Hsla,
    /// The meter bars that are not lit, on the pill.
    pub meter_off: Hsla,
    /// Opaque, so its contrast can be measured.
    pub playing_bg: Hsla,
    /// The fill behind a matched word in a search result. `text` is held to 4.5 to 1 on it. A
    /// match is also semibold, so the fill never carries it alone (A11Y-8).
    pub search_match: Hsla,
    pub avatar_bg: Hsla,
    /// The fill of everything that floats over the app: dialogs, popovers, menus, toasts, and
    /// the Processing window (`docs/design/floating-surfaces.md` section 2). Opaque, and lighter
    /// than the window in dark mode.
    pub surface_raised: Hsla,
    /// The 1 pt edge of a floating surface. Increase Contrast takes `text_secondary`, which
    /// reaches 3 to 1 against the surface and the dimmed window.
    pub surface_border: Hsla,
    /// Cards inside a surface: the running job and the setup steps.
    pub surface_inset: Hsla,
    pub inset_border: Hsla,
    /// Behind modal dialogs only, over the whole window.
    pub scrim: Hsla,
    /// The scrim under Reduce Transparency.
    pub scrim_opaque: Hsla,
    /// A dialog's close button at rest. Transparent under Increase Contrast, which rings it.
    pub close_bg: Hsla,
    /// The empty part of a progress bar, and the disabled primary button.
    pub track: Hsla,
    /// A Settings group's fill (settings brief section 1).
    pub group_bg: Hsla,
    pub group_border: Hsla,
    /// The off radio's ring, moved until it reaches 3 to 1 on every fill a radio sits on (WCAG
    /// 1.4.11). Increase Contrast draws it like every other border, in `text_secondary`.
    pub radio_border: Hsla,
    /// A meeting-language chip, `avatar_bg`.
    pub chip_bg: Hsla,
    /// The dotted table-of-contents leaders in the sidebar. Decorative.
    pub leader: Hsla,
}

/// Below this window width the running head and the drop cap hide and the transcript's
/// timestamps move inline.
pub const NARROW_WINDOW: f32 = 900.;

/// What Book's ornaments draw at one window width and text scale, after their reflow rules
/// (`book-theme.md` section 4). The section ornament and the play-script transcript draw at
/// every size, so they are not here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Presentation {
    pub running_head: bool,
    pub toc_leaders: bool,
    pub drop_cap: bool,
    /// The script's timestamps hang in the left margin rather than sitting before the name.
    pub hanging_timestamps: bool,
}

impl Presentation {
    pub fn of(window_width: f32, text_scale: f32) -> Presentation {
        let wide = window_width >= NARROW_WINDOW;
        Presentation {
            running_head: wide,
            // The title wraps from 150% text, and leaders go with it.
            toc_leaders: text_scale < 1.5,
            drop_cap: wide && text_scale <= 1.25,
            hanging_timestamps: wide,
        }
    }
}

/// The two colors of the selected row's focus ring, outside in: `edge`, `line`, then the fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowRing {
    pub edge: Hsla,
    pub line: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// A11Y-6: macOS Increase Contrast. Solid borders, no shadows, no translucency.
    pub high_contrast: bool,
    /// macOS Differentiate Without Color: states that a fill color carries also get a word.
    pub differentiate_without_color: bool,
    /// Off under Increase Contrast, which draws borders instead.
    pub shadows: bool,
    pub tokens: Tokens,
    /// View > Bigger and Smaller (A11Y-7), 0.85 to 2.0. Every text size goes through
    /// [`Theme::text`].
    pub text_scale: f32,
}

/// Views read a token as `theme.text_secondary`: the tokens are most of what a theme is.
impl std::ops::Deref for Theme {
    type Target = Tokens;

    fn deref(&self) -> &Tokens {
        &self.tokens
    }
}

impl Tokens {
    /// Every color token by name.
    pub fn named(&self) -> Vec<(&'static str, Hsla)> {
        let Tokens {
            window_bg,
            sidebar_bg,
            panel_border,
            text,
            text_secondary,
            divider,
            field_bg,
            control_bg,
            control_border,
            card_bg,
            card_border,
            accent,
            accent_text,
            ring,
            on_accent,
            selection_inactive,
            wave_rest,
            wave_done,
            wave_track,
            record_red,
            on_record,
            record_glyph,
            attention,
            attention_border,
            on_attention,
            meter_off,
            playing_bg,
            search_match,
            avatar_bg,
            surface_raised,
            surface_border,
            surface_inset,
            inset_border,
            scrim,
            scrim_opaque,
            close_bg,
            track,
            group_bg,
            group_border,
            radio_border,
            chip_bg,
            leader,
        } = *self;
        vec![
            ("window_bg", window_bg),
            ("sidebar_bg", sidebar_bg),
            ("panel_border", panel_border),
            ("text", text),
            ("text_secondary", text_secondary),
            ("divider", divider),
            ("field_bg", field_bg),
            ("control_bg", control_bg),
            ("control_border", control_border),
            ("card_bg", card_bg),
            ("card_border", card_border),
            ("accent", accent),
            ("accent_text", accent_text),
            ("ring", ring),
            ("on_accent", on_accent),
            ("selection_inactive", selection_inactive),
            ("wave_rest", wave_rest),
            ("wave_done", wave_done),
            ("wave_track", wave_track),
            ("record_red", record_red),
            ("on_record", on_record),
            ("record_glyph", record_glyph),
            ("attention", attention),
            ("attention_border", attention_border),
            ("on_attention", on_attention),
            ("meter_off", meter_off),
            ("playing_bg", playing_bg),
            ("search_match", search_match),
            ("avatar_bg", avatar_bg),
            ("surface_raised", surface_raised),
            ("surface_border", surface_border),
            ("surface_inset", surface_inset),
            ("inset_border", inset_border),
            ("scrim", scrim),
            ("scrim_opaque", scrim_opaque),
            ("close_bg", close_bg),
            ("track", track),
            ("group_bg", group_bg),
            ("group_border", group_border),
            ("radio_border", radio_border),
            ("chip_bg", chip_bg),
            ("leader", leader),
        ]
    }
}

impl Global for Theme {}

/// Which shadow a floating surface casts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Dialogs and the Processing window.
    Float,
    /// Popovers, menus, and toasts.
    Pop,
}

/// What the theme is built from besides the appearance: the macOS display settings and the text
/// scale. `system_prefs`, Settings, and the View menu change it, then call [`sync`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub increase_contrast: bool,
    pub differentiate_without_color: bool,
    pub text_scale: f32,
}

impl Default for Display {
    fn default() -> Self {
        Display {
            increase_contrast: false,
            differentiate_without_color: false,
            text_scale: 1.0,
        }
    }
}

impl Global for Display {}

/// WCAG 2.2 AA for body text.
pub const MIN_CONTRAST: f32 = 4.5;
/// WCAG 2.2 AA for focus rings, borders that mark a control, and graphics (1.4.11).
pub const MIN_NON_TEXT_CONTRAST: f32 = 3.0;

/// The failed card's border is extra to its glyph and words, so it only has to be visible.
pub const ATTENTION_BORDER_MIN: f32 = 1.5;

/// A11Y-7's steps, smallest to largest.
pub const TEXT_SCALES: [f32; 7] = [0.85, 1.0, 1.15, 1.3, 1.5, 1.75, 2.0];

/// The next step up from `scale`, or the largest.
pub fn bigger(scale: f32) -> f32 {
    TEXT_SCALES
        .into_iter()
        .find(|s| *s > scale + 0.001)
        .unwrap_or(TEXT_SCALES[TEXT_SCALES.len() - 1])
}

/// The next step down from `scale`, or the smallest.
pub fn smaller(scale: f32) -> f32 {
    TEXT_SCALES
        .into_iter()
        .rev()
        .find(|s| *s < scale - 0.001)
        .unwrap_or(TEXT_SCALES[0])
}

/// A saved scale, kept within A11Y-7's range.
pub fn clamp_text_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(TEXT_SCALES[0], TEXT_SCALES[TEXT_SCALES.len() - 1])
    } else {
        1.0
    }
}

/// One foreground painted on one background, and the ratio it must reach.
#[derive(Clone, Debug)]
pub struct Pair {
    pub name: String,
    pub fg: Hsla,
    pub bg: Hsla,
    pub min: f32,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Increase Contrast multiplies the search mark's fill alpha by this. The mark is also underlined
/// then, because `HighlightStyle` has no border (A11Y-6, `docs/plans/search.md` S4 lane 9).
const HIGH_CONTRAST_MATCH_BOOST: f32 = 1.8;

/// How much of `radio_border` a disabled radio or checkbox keeps, over its own fill. A dimmed
/// control has to stay a visible control; the old `divider` outline fell to 1.2 to 1.
const DISABLED_OUTLINE_KEEP: f32 = 0.6;

/// The floor for a disabled outline under Increase Contrast.
pub const DISABLED_OUTLINE_HIGH_MIN: f32 = 2.0;

fn tint((hex, alpha): (u32, f32)) -> Hsla {
    Hsla { a: alpha, ..c(hex) }
}

/// The dark text on a light fill, which `readable_on` tries after white.
const DARK_TEXT: u32 = 0x1D1D1F;

/// The colors of one appearance, one field per token it sets, so leaving one out does not
/// compile. Values from `docs/design/book-theme.md` section 2; the ones the spec leaves open are
/// chosen in the same family and held to the same floors by `tests/contrast.rs`.
struct BookPalette {
    window_bg: u32,
    sidebar_bg: u32,
    panel_border: u32,
    divider: u32,
    text: u32,
    text_secondary: u32,
    accent: u32,
    on_accent: u32,
    /// `selection_inactive` and `playing_bg`, as color and alpha over the paper.
    highlight: (u32, f32),
    /// `search_match`, the ink blue as color and alpha over the sidebar's paper.
    search_match: (u32, f32),
    card_bg: u32,
    card_border: u32,
    control_bg: u32,
    control_border: u32,
    /// The canvas's 7% ink tint lowered to 4% over the endpaper, so secondary text in the search
    /// field still reaches 4.5 to 1.
    field_bg: u32,
    wave_rest: u32,
    record_red: u32,
    attention: u32,
    attention_border: u32,
    avatar_bg: u32,
    surface_raised: u32,
    surface_border: u32,
    surface_inset: u32,
    inset_border: u32,
    scrim: (u32, f32),
    scrim_opaque: u32,
    close_bg: (u32, f32),
    track: u32,
    group_bg: u32,
    group_border: u32,
    radio_border: u32,
    leader: u32,
}

/// Paper, then night.
const BOOK: [BookPalette; 2] = [
    BookPalette {
        window_bg: 0xFAF6EC,
        sidebar_bg: 0xEFE6D2,
        panel_border: 0xDDD2BC,
        divider: 0xDDD2BC,
        text: 0x2A251F,
        text_secondary: 0x6A6054,
        accent: 0x2E4A86,
        on_accent: 0xFFFFFF,
        highlight: (0xF2E2A0, 1.0),
        search_match: (0x2E4A86, 0.18),
        card_bg: 0xF3ECDD,
        card_border: 0xE2D7C1,
        control_bg: 0xFFFDF7,
        control_border: 0xCDBFA4,
        field_bg: 0xE8DECA,
        wave_rest: 0x8E8370,
        record_red: 0xC62D23,
        attention: 0x8F5200,
        attention_border: 0xD9B98A,
        avatar_bg: 0xE9DFC9,
        surface_raised: 0xFFFDF7,
        surface_border: 0x7A6F5E,
        surface_inset: 0xF3ECDD,
        inset_border: 0xE2D7C1,
        scrim: (0x281C0A, 0.16),
        scrim_opaque: 0xCFC6B4,
        close_bg: (0x3C280A, 0.06),
        track: 0xE6DCC6,
        group_bg: 0xF3ECDD,
        group_border: 0xE2D7C1,
        radio_border: 0x9A8C72,
        leader: 0xB9AE98,
    },
    BookPalette {
        window_bg: 0x1C1915,
        sidebar_bg: 0x24201B,
        panel_border: 0x3A342C,
        divider: 0x3A342C,
        text: 0xECE3D3,
        text_secondary: 0xB8AC97,
        accent: 0x9DB5E6,
        on_accent: 0x1C1915,
        highlight: (0xDEBA54, 0.20),
        search_match: (0x9DB5E6, 0.25),
        card_bg: 0x24201B,
        card_border: 0x3A342C,
        control_bg: 0x2A251F,
        control_border: 0x4A4238,
        field_bg: 0x312D29,
        wave_rest: 0x7D7262,
        record_red: 0xE5483D,
        attention: 0xF0B24A,
        attention_border: 0x7A5A28,
        avatar_bg: 0x3A342C,
        surface_raised: 0x2E2924,
        surface_border: 0x847A6A,
        surface_inset: 0x36302A,
        inset_border: 0x4A4238,
        scrim: (0x000000, 0.5),
        scrim_opaque: 0x0E0C0A,
        close_bg: (0xFFF1DA, 0.08),
        track: 0x4A4238,
        group_bg: 0x24201B,
        group_border: 0x3A342C,
        radio_border: 0x6E6456,
        leader: 0x5A5145,
    },
];

/// The search mark's fill: `color` at `alpha` over `under`. Increase Contrast raises the alpha by
/// [`HIGH_CONTRAST_MATCH_BOOST`], backing off in 2% steps until `text` keeps 4.5 to 1 on it, which
/// a bright accent in dark reaches before the full boost.
fn match_fill(color: Hsla, under: Hsla, text: Hsla, alpha: f32, high: bool) -> Hsla {
    if !high {
        return blend(color, under, alpha);
    }
    let mut boosted = (alpha * HIGH_CONTRAST_MATCH_BOOST).min(1.0);
    while boosted > alpha && contrast(text, blend(color, under, boosted)) < MIN_CONTRAST {
        boosted -= 0.02;
    }
    blend(color, under, boosted.max(alpha))
}

/// `color` at `alpha` over `under`, as one opaque color.
fn over((hex, alpha): (u32, f32), under: Hsla) -> Hsla {
    if alpha >= 1.0 {
        c(hex)
    } else {
        blend(c(hex), under, alpha)
    }
}

impl Theme {
    /// Book (`docs/design/book-theme.md`): paper and ink, an opaque window, and Literata for
    /// reading. Each token comes from [`BOOK`].
    pub fn new(dark: bool, display: Display) -> Theme {
        let p = &BOOK[dark as usize];
        let high = display.increase_contrast;
        let window_bg = c(p.window_bg);
        let sidebar_bg = c(p.sidebar_bg);
        let card_bg = c(p.card_bg);
        let control_bg = c(p.control_bg);
        let field_bg = c(p.field_bg);
        let highlight = over(p.highlight, window_bg);
        let group_bg = c(p.group_bg);
        let surface_raised = c(p.surface_raised);
        let surface_inset = c(p.surface_inset);
        let text = c(p.text);
        let text_secondary = c(p.text_secondary);
        let border = |normal: u32| if high { text_secondary } else { c(normal) };
        let reading = [
            window_bg,
            sidebar_bg,
            card_bg,
            control_bg,
            field_bg,
            highlight,
            group_bg,
            surface_raised,
            surface_inset,
        ];
        let accent = c(p.accent);
        let record_red = c(p.record_red);
        let attention = with_contrast_all(c(p.attention), &reading, MIN_CONTRAST);
        let wave_floor = if high {
            MIN_CONTRAST
        } else {
            MIN_NON_TEXT_CONTRAST
        };
        let waves = [card_bg, surface_inset];
        let wave_rest = with_contrast_all(c(p.wave_rest), &waves, wave_floor);
        let tokens = Tokens {
            window_bg,
            sidebar_bg,
            panel_border: border(p.panel_border),
            text,
            text_secondary,
            divider: border(p.divider),
            field_bg,
            control_bg,
            control_border: border(p.control_border),
            card_bg,
            card_border: border(p.card_border),
            accent,
            accent_text: accent,
            ring: if high { text } else { accent },
            on_accent: c(p.on_accent),
            selection_inactive: highlight,
            wave_rest,
            wave_done: with_contrast_all(accent, &waves, wave_floor),
            wave_track: if high {
                card_bg
            } else {
                blend(wave_rest, card_bg, 0.35)
            },
            record_red,
            on_record: readable_on(record_red),
            record_glyph: with_contrast_all(record_red, &reading, MIN_CONTRAST),
            attention,
            attention_border: if high {
                attention
            } else {
                c(p.attention_border)
            },
            on_attention: readable_on(attention),
            meter_off: blend(readable_on(record_red), record_red, 0.4),
            playing_bg: highlight,
            search_match: match_fill(
                c(p.search_match.0),
                sidebar_bg,
                text,
                p.search_match.1,
                high,
            ),
            avatar_bg: c(p.avatar_bg),
            surface_raised,
            surface_border: border(p.surface_border),
            surface_inset,
            inset_border: border(p.inset_border),
            scrim: tint(p.scrim),
            scrim_opaque: c(p.scrim_opaque),
            close_bg: if high {
                gpui_kit::transparent_black()
            } else {
                tint(p.close_bg)
            },
            track: c(p.track),
            group_bg,
            group_border: border(p.group_border),
            radio_border: if high {
                text_secondary
            } else {
                with_contrast_all(
                    c(p.radio_border),
                    &[control_bg, group_bg, window_bg],
                    MIN_NON_TEXT_CONTRAST,
                )
            },
            chip_bg: c(p.avatar_bg),
            leader: c(p.leader),
        };
        Theme {
            dark,
            high_contrast: high,
            differentiate_without_color: display.differentiate_without_color,
            shadows: !high,
            tokens,
            text_scale: clamp_text_scale(display.text_scale),
        }
    }

    /// Increase Contrast outlines the highlighter fill (the selected row, the playing turn) in
    /// 1 pt `text_secondary`. `None` otherwise.
    pub fn highlight_outline(&self) -> Option<Hsla> {
        self.high_contrast.then_some(self.text_secondary)
    }

    /// Book's ornaments as drawn at this window width and the current text scale.
    pub fn presentation(&self, window_width: f32) -> Presentation {
        Presentation::of(window_width, self.text_scale)
    }

    pub fn for_appearance(appearance: WindowAppearance, display: Display) -> Theme {
        let dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        Theme::new(dark, display)
    }

    pub fn get(cx: &App) -> Theme {
        *cx.global::<Theme>()
    }

    /// A text size in points, scaled by View > Bigger and Smaller.
    /// The selected Settings pane's icon tile, lighter than the accent row it sits on.
    pub fn selected_tile(&self) -> Hsla {
        blend(gpui_kit::white(), self.accent, 0.22)
    }

    /// The window under a modal dialog's scrim, as one opaque color.
    pub fn dimmed_window(&self) -> Hsla {
        blend(
            Hsla {
                a: 1.0,
                ..self.scrim
            },
            self.window_bg,
            self.scrim.a,
        )
    }

    /// The close button's resting fill over the surface, as one opaque color.
    pub fn close_fill(&self) -> Hsla {
        blend(
            Hsla {
                a: 1.0,
                ..self.close_bg
            },
            self.surface_raised,
            self.close_bg.a,
        )
    }

    /// The filled part of a progress bar: the accent where it reaches 3 to 1 on the track, else
    /// the accent text color, which is lightened (dark) or darkened (light) until it does.
    pub fn progress_fill(&self) -> Hsla {
        if contrast(self.accent, self.track) >= MIN_NON_TEXT_CONTRAST {
            self.accent
        } else {
            with_contrast_all(self.accent_text, &[self.track], MIN_NON_TEXT_CONTRAST)
        }
    }

    /// `shadow_float` for dialogs and `shadow_pop` for popovers, menus, and toasts, as the
    /// spec's CSS layers. GPUI takes a blur radius as the gaussian's deviation, half of CSS's
    /// blur, so each blur is halved. None under Increase Contrast.
    pub fn shadow(&self, elevation: Elevation) -> Vec<gpui_kit::BoxShadow> {
        if !self.shadows {
            return Vec::new();
        }
        let layers: [(f32, f32, f32); 2] = match (elevation, self.dark) {
            (Elevation::Float, false) => [(1., 2., 0.08), (12., 32., 0.18)],
            (Elevation::Float, true) => [(1., 2., 0.5), (16., 40., 0.55)],
            (Elevation::Pop, false) => [(1., 2., 0.08), (8., 24., 0.16)],
            (Elevation::Pop, true) => [(1., 2., 0.5), (10., 28., 0.5)],
        };
        layers
            .into_iter()
            .map(|(y, blur, alpha)| gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0., 0., 0., alpha),
                offset: gpui_kit::point(px(0.), px(y)),
                blur_radius: px(blur / 2.),
                spread_radius: px(0.),
                inset: false,
            })
            .collect()
    }

    pub fn text(&self, points: f32) -> Pixels {
        px(points * self.text_scale)
    }

    /// The detail pane's reading column, in points: a 640 pt measure for 17 pt serif, growing with
    /// the text to 720 at 200%.
    pub fn column(&self) -> f32 {
        (640. + (self.text_scale - 1.) * 80.).clamp(640., 720.)
    }

    /// The focus ring (2 pt [`Theme::ring`]). On an `accent` fill it switches to `on_accent`, or
    /// the ring would vanish into the fill (spike finding, lane 4).
    pub fn focus_ring(&self, on_accent_fill: bool) -> Hsla {
        if on_accent_fill {
            self.on_accent
        } else {
            self.ring
        }
    }

    /// The focus ring of the accent-filled selected sidebar row. `on_accent` alone touches the
    /// sidebar's paper on the outside, so in light a white line vanished into the near-white
    /// column. The line sits between two accent bands, `edge` outside and the row's fill inside,
    /// so both of its sides are told apart from it whatever the sidebar is.
    pub fn row_ring(&self) -> RowRing {
        RowRing {
            edge: self.accent,
            line: self.on_accent,
        }
    }

    /// The outline of a disabled radio or checkbox: `radio_border` dimmed toward the control's
    /// own fill, so it reads as a dimmed control rather than as no control.
    pub fn disabled_outline(&self) -> Hsla {
        blend(self.radio_border, self.control_bg, DISABLED_OUTLINE_KEEP)
    }

    pub fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Secondary => self.text_secondary,
            Tone::Accent => self.accent_text,
            Tone::Attention => self.attention,
            Tone::Record => self.record_glyph,
        }
    }

    /// Every color pair the views paint, with its floor: 4.5 to 1 for text (A11Y-8), 3 to 1 for
    /// focus rings and graphics. A view that paints a new pair adds it here first.
    pub fn pairs(&self) -> Vec<Pair> {
        let text = |name: &str, fg, bg| Pair {
            name: name.to_owned(),
            fg,
            bg,
            min: MIN_CONTRAST,
        };
        let graphic = |name: &str, fg, bg| Pair {
            name: name.to_owned(),
            fg,
            bg,
            min: MIN_NON_TEXT_CONTRAST,
        };
        let backgrounds = [
            ("window_bg", self.window_bg),
            ("sidebar_bg", self.sidebar_bg),
            ("card_bg", self.card_bg),
            ("control_bg", self.control_bg),
            ("field_bg", self.field_bg),
            ("selection_inactive", self.selection_inactive),
            ("playing_bg", self.playing_bg),
            ("group_bg", self.group_bg),
            ("surface_raised", self.surface_raised),
            ("surface_inset", self.surface_inset),
        ];
        let mut pairs = Vec::new();
        for (fg_name, fg) in [
            ("text", self.text),
            ("text_secondary", self.text_secondary),
            ("accent_text", self.accent_text),
            ("attention", self.attention),
            ("record_glyph", self.record_glyph),
        ] {
            for (bg_name, bg) in backgrounds {
                pairs.push(text(&format!("{fg_name} on {bg_name}"), fg, bg));
            }
        }
        for (bg_name, bg) in [
            ("window_bg", self.window_bg),
            ("sidebar_bg", self.sidebar_bg),
            ("card_bg", self.card_bg),
            ("control_bg", self.control_bg),
            ("group_bg", self.group_bg),
            ("surface_raised", self.surface_raised),
            ("surface_inset", self.surface_inset),
        ] {
            pairs.push(graphic(&format!("focus ring on {bg_name}"), self.ring, bg));
        }
        // The operator chose a light border that reads with the white fill and the shadow rather
        // than alone (floating-surfaces.md, the operator's decisions), so standard light only
        // has to show it on the surface. Dark and Increase Contrast hold it to 3 to 1 on the
        // surface and on the window the scrim dims, translucent and opaque.
        let edge_floor = if self.dark || self.high_contrast {
            MIN_NON_TEXT_CONTRAST
        } else {
            ATTENTION_BORDER_MIN
        };
        let mut edges = vec![("surface_raised", self.surface_raised)];
        if self.dark || self.high_contrast {
            edges.extend([
                ("the dimmed window", self.dimmed_window()),
                ("scrim_opaque", self.scrim_opaque),
            ]);
        }
        for (bg_name, bg) in edges {
            pairs.push(Pair {
                name: format!("surface_border on {bg_name}"),
                fg: self.surface_border,
                bg,
                min: edge_floor,
            });
        }
        for (bg_name, bg) in [
            ("control_bg", self.control_bg),
            ("group_bg", self.group_bg),
            ("window_bg", self.window_bg),
        ] {
            pairs.push(graphic(
                &format!("radio_border on {bg_name}"),
                self.radio_border,
                bg,
            ));
        }
        let row_ring = self.row_ring();
        pairs.extend([
            Pair {
                name: "row ring edge on sidebar_bg".to_owned(),
                fg: row_ring.edge,
                bg: self.sidebar_bg,
                min: ATTENTION_BORDER_MIN,
            },
            graphic("row ring line on its edge", row_ring.line, row_ring.edge),
        ]);
        for (bg_name, bg) in [
            ("control_bg", self.control_bg),
            ("group_bg", self.group_bg),
            ("window_bg", self.window_bg),
        ] {
            pairs.push(Pair {
                name: format!("disabled outline on {bg_name}"),
                fg: self.disabled_outline(),
                bg,
                // Disabled controls are exempt from WCAG 1.4.11. They only have to be seen, and
                // under Increase Contrast clearly so.
                min: if self.high_contrast {
                    DISABLED_OUTLINE_HIGH_MIN
                } else {
                    ATTENTION_BORDER_MIN
                },
            });
        }
        pairs.extend([
            text("on_accent on accent", self.on_accent, self.accent),
            graphic("focus ring on accent", self.on_accent, self.accent),
            text("on_record on record_red", self.on_record, self.record_red),
            text(
                "on_attention on attention",
                self.on_attention,
                self.attention,
            ),
            text(
                "text_secondary on close_bg",
                self.text_secondary,
                self.close_fill(),
            ),
            graphic("progress on track", self.progress_fill(), self.track),
            graphic(
                "wave_rest on surface_inset",
                self.wave_rest,
                self.surface_inset,
            ),
            graphic(
                "wave_done on surface_inset",
                self.wave_done,
                self.surface_inset,
            ),
            graphic("playhead on surface_inset", self.text, self.surface_inset),
            text("text on search_match", self.text, self.search_match),
            text("text on avatar_bg", self.text, self.avatar_bg),
            text("text on chip_bg", self.text, self.chip_bg),
            graphic("wave_rest on card_bg", self.wave_rest, self.card_bg),
            graphic("accent wave on card_bg", self.accent, self.card_bg),
            graphic("wave_done on card_bg", self.wave_done, self.card_bg),
            graphic("playhead on card_bg", self.text, self.card_bg),
            graphic("record dot on control_bg", self.record_red, self.control_bg),
            graphic(
                "text_secondary icon on chip_bg",
                self.text_secondary,
                self.chip_bg,
            ),
            graphic(
                "on_accent icon on the selected pane's tile",
                self.on_accent,
                self.selected_tile(),
            ),
        ]);
        for (bg_name, bg) in [
            ("card_bg", self.card_bg),
            ("surface_inset", self.surface_inset),
        ] {
            pairs.push(Pair {
                name: format!("attention_border on {bg_name}"),
                fg: self.attention_border,
                bg,
                // It only adds to the triangle and the words, so it needs to be seen, not read.
                // Increase Contrast holds it to the full graphics floor.
                min: if self.high_contrast {
                    MIN_NON_TEXT_CONTRAST
                } else {
                    ATTENTION_BORDER_MIN
                },
            });
        }
        if self.high_contrast {
            for (bg_name, bg) in [
                ("window_bg", self.window_bg),
                ("sidebar_bg", self.sidebar_bg),
                ("card_bg", self.card_bg),
                ("control_bg", self.control_bg),
                ("group_bg", self.group_bg),
            ] {
                pairs.push(graphic(
                    &format!("border on {bg_name}"),
                    self.control_border,
                    bg,
                ));
            }
            for (fg_name, fg) in [
                ("inset_border", self.inset_border),
                ("close ring", self.text_secondary),
                ("track outline", self.text_secondary),
            ] {
                pairs.push(graphic(
                    &format!("{fg_name} on surface_raised"),
                    fg,
                    self.surface_raised,
                ));
            }
        }
        pairs
    }
}

/// Installs the theme for the current appearance and [`Display`], and keeps GPUI Kit's
/// own components in step in every appearance: the same mode, the direction C focus ring, the
/// floating-surface fill, text, scrim, and dialog radius (`docs/design/floating-surfaces.md`
/// section 6), Increase Contrast's borders and missing shadows, and the text scale as Kit's rem
/// size. Call it at launch and on every change of any input.
pub fn sync(cx: &mut App) {
    crate::fonts::ensure(cx);
    let display = cx.try_global::<Display>().copied().unwrap_or_default();
    let appearance = cx.window_appearance();
    let theme = Theme::for_appearance(appearance, display);
    cx.set_global(theme);
    gpui_kit::component::Theme::change(appearance, None, cx);
    let scrim = if crate::system_prefs::reduces_transparency(cx) {
        theme.scrim_opaque
    } else {
        theme.scrim
    };
    gpui_kit::component::Theme::update(cx, |kit| {
        kit.ring = theme.ring;
        // Kit sizes its controls in rems, so below 100% its buttons would shrink under 24 pt
        // (A11Y-9). They grow with the text but never shrink.
        kit.font_size = px(16. * theme.text_scale.max(1.0));
        kit.shadow = theme.shadows;
        kit.foreground = theme.text;
        kit.popover = theme.surface_raised;
        kit.popover_foreground = theme.text;
        kit.overlay = scrim;
        kit.radius_lg = px(crate::surface::Kind::Dialog.radius());
        kit.font_family = crate::fonts::FAMILY.into();
        // Kit's buttons, fields, and menus stay on paper and ink.
        kit.border = theme.control_border;
        kit.input = theme.control_border;
        kit.muted_foreground = theme.text_secondary;
        // In light, Kit paints a text field and an outline button (the pickers) in
        // `background`, which is its pure white, through `input_background()`. Dark mixes
        // `input` instead and never reads it. The other readers of `background` (title bar,
        // badge border, alert, tab, toggle, sheet, kbd) are components Dory does not draw.
        if !theme.dark {
            kit.background = theme.control_bg;
        }
        kit.primary = theme.accent;
        kit.primary_foreground = theme.on_accent;
        kit.secondary = theme.control_bg;
        kit.secondary_foreground = theme.text;
        kit.accent = theme.selection_inactive;
        kit.accent_foreground = theme.text;
    });
    cx.refresh_windows();
}

/// Tabular figures, so times and percents keep their width as they count (handoff section 2).
pub fn tabular() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// Changes one input of the theme and rebuilds it.
pub fn set_display(edit: impl FnOnce(&mut Display), cx: &mut App) {
    let mut display = cx.try_global::<Display>().copied().unwrap_or_default();
    edit(&mut display);
    display.text_scale = clamp_text_scale(display.text_scale);
    cx.set_global(display);
    sync(cx);
}

/// WCAG relative-luminance contrast ratio, 1 to 21.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a.into()), luminance(b.into()));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn luminance(color: Rgba) -> f32 {
    let channel = |v: f32| {
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// `top` at `alpha` over `under`, as one opaque color.
pub fn blend(top: Hsla, under: Hsla, alpha: f32) -> Hsla {
    let (t, u): (Rgba, Rgba) = (top.into(), under.into());
    let mix = |a: f32, b: f32| a * alpha + b * (1.0 - alpha);
    Rgba {
        r: mix(t.r, u.r),
        g: mix(t.g, u.g),
        b: mix(t.b, u.b),
        a: 1.0,
    }
    .into()
}

/// White text on `fill` when it passes, else the dark text color, else black (Book's night red,
/// which sits between the two).
pub fn readable_on(fill: Hsla) -> Hsla {
    [c(0xFFFFFF), c(DARK_TEXT)]
        .into_iter()
        .find(|text| contrast(*text, fill) >= MIN_CONTRAST)
        .unwrap_or(c(0x000000))
}

/// A11Y-8: moves `color`'s HSL lightness away from the backgrounds it is painted on, all of which
/// share one lightness side, in 1% steps until the worst pair reaches `min`. Against light
/// backgrounds it darkens, against dark ones it lightens.
pub fn with_contrast_all(color: Hsla, against: &[Hsla], min: f32) -> Hsla {
    let light = against.iter().all(|bg| luminance((*bg).into()) > 0.18);
    let step = if light { -0.01 } else { 0.01 };
    let worst = |c: Hsla| {
        against
            .iter()
            .map(|bg| contrast(c, *bg))
            .fold(f32::INFINITY, f32::min)
    };
    let mut out = Hsla { a: 1.0, ..color };
    while worst(out) < min && (0.0..=1.0).contains(&(out.l + step)) {
        out.l += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(increase_contrast: bool) -> Display {
        Display {
            increase_contrast,
            ..Display::default()
        }
    }

    #[test]
    fn book_tokens_are_the_specs_values() {
        let paper = Theme::new(false, Display::default());
        let night = Theme::new(true, Display::default());
        assert_eq!(paper.window_bg, c(0xFAF6EC));
        assert_eq!(night.window_bg, c(0x1C1915));
        assert_eq!(paper.accent, c(0x2E4A86));
        assert_eq!(night.accent, c(0x9DB5E6));
        assert_eq!(paper.on_accent, c(0xFFFFFF));
        assert_eq!(night.on_accent, c(0x1C1915), "on_accent flips in night");
        assert_eq!(paper.selection_inactive, c(0xF2E2A0));
        assert_eq!(paper.playing_bg, paper.selection_inactive);
        assert_eq!(night.wave_done, night.accent);
        assert_eq!(paper.surface_border, c(0x7A6F5E));
    }

    #[test]
    fn increase_contrast_draws_borders_in_secondary_and_outlines_the_highlight() {
        let book = Theme::new(false, display(true));
        assert!(!book.shadows);
        for border in [
            book.panel_border,
            book.divider,
            book.control_border,
            book.card_border,
            book.surface_border,
            book.group_border,
        ] {
            assert_eq!(border, book.text_secondary);
        }
        assert_eq!(book.highlight_outline(), Some(book.text_secondary));
        assert_eq!(
            Theme::new(false, Display::default()).highlight_outline(),
            None
        );
        assert!(contrast(book.wave_rest, book.card_bg) >= MIN_CONTRAST);
    }

    #[test]
    fn the_running_head_hides_below_900_points() {
        assert!(Presentation::of(900., 1.0).running_head);
        assert!(!Presentation::of(899., 1.0).running_head);
        assert!(Presentation::of(1280., 2.0).running_head);
    }

    #[test]
    fn leaders_go_when_the_title_wraps_at_150_percent() {
        for scale in [0.85, 1.0, 1.15, 1.3] {
            assert!(Presentation::of(800., scale).toc_leaders, "{scale}");
        }
        for scale in [1.5, 1.75, 2.0] {
            assert!(!Presentation::of(1280., scale).toc_leaders, "{scale}");
        }
    }

    #[test]
    fn the_drop_cap_needs_a_wide_window_and_at_most_125_percent_text() {
        assert!(Presentation::of(1280., 1.15).drop_cap);
        assert!(!Presentation::of(1280., 1.3).drop_cap);
        assert!(!Presentation::of(899., 1.0).drop_cap);
    }

    #[test]
    fn script_timestamps_hang_in_the_margin_only_in_a_wide_window() {
        assert!(Presentation::of(1280., 1.0).hanging_timestamps);
        assert!(!Presentation::of(899., 1.0).hanging_timestamps);
    }

    #[test]
    fn text_scale_steps_stay_within_85_and_200_percent() {
        assert_eq!(bigger(1.0), 1.15);
        assert_eq!(smaller(1.0), 0.85);
        assert_eq!(bigger(2.0), 2.0);
        assert_eq!(smaller(0.85), 0.85);
        assert_eq!(
            bigger(1.07),
            1.15,
            "an odd saved value snaps to the next step"
        );
        assert_eq!(clamp_text_scale(9.0), 2.0);
        assert_eq!(clamp_text_scale(f32::NAN), 1.0);
    }
}
