//! Direction C's tokens (section 2 of `docs/design/handoff-direction-c.md`) as one `Theme`, kept
//! as a GPUI global that follows the system appearance, the accent color, the display
//! accessibility settings (`system_prefs`), and View > Bigger and Smaller.
//!
//! Views read it with `Theme::get(cx)` and never hard-code a color or a text size. Every color
//! pair the views paint is listed in [`Theme::pairs`], and `tests/contrast.rs` holds each one to
//! its WCAG floor in every variant.

use gpui_kit::{App, Global, Hsla, Pixels, Rgba, WindowAppearance, px, rgb};

use crate::library::Tone;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// A11Y-6: macOS Increase Contrast. Solid borders, no shadows, no translucency.
    pub high_contrast: bool,
    /// macOS Differentiate Without Color: states that a fill color carries also get a word.
    pub differentiate_without_color: bool,
    /// Off under Increase Contrast, which draws borders instead.
    pub shadows: bool,
    pub window_bg: Hsla,
    pub sidebar_bg: Hsla,
    /// The sidebar column over the blurred desktop, its 8 pt margins included (UX-25).
    pub glass_tint: Hsla,
    /// The sidebar panel, layered on [`Theme::glass_tint`]. `tests/contrast.rs` holds sidebar
    /// text over the pair on white, black, and photo wallpapers.
    pub sidebar_tint: Hsla,
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
    /// The 2 pt focus ring. `accent` in light; in dark, `accent` is darkened for white text and
    /// falls under 3 to 1 on the dark controls, so the ring uses the lightened `accent_text`.
    pub ring: Hsla,
    pub on_accent: Hsla,
    pub selection_inactive: Hsla,
    pub wave_rest: Hsla,
    /// Bars transcription has passed (processing view). `accent` in light; in dark, the accent
    /// lightened until it reaches 3 to 1 on `card_bg`.
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
    /// The 1 pt edge of a floating surface. Light is the operator's `#C3C7CF`, which reads
    /// against white with the shadow; Increase Contrast takes `text_secondary`, which reaches
    /// 3 to 1 against the surface and the dimmed window.
    pub surface_border: Hsla,
    /// Cards inside a surface: the running job and the setup steps.
    pub surface_inset: Hsla,
    pub inset_border: Hsla,
    /// Behind modal dialogs only, over the whole window, glass included.
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
    /// The off radio's ring. The brief's `#A9ADB6` and `#5A5B63`, moved until they reach 3 to 1
    /// on every fill a radio sits on (WCAG 1.4.11).
    pub radio_border: Hsla,
    /// A meeting-language chip, `avatar_bg`.
    pub chip_bg: Hsla,
    /// View > Bigger and Smaller (A11Y-7), 0.85 to 2.0. Every text size goes through
    /// [`Theme::text`].
    pub text_scale: f32,
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

/// What the theme is built from besides the appearance and accent: the macOS display settings
/// and the text scale. `system_prefs` and the View menu change it, then call [`sync`].
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

/// macOS's default blue, used when AppKit has no accent to offer (tests, other platforms).
pub const DEFAULT_ACCENT_LIGHT: u32 = 0x007AFF;
pub const DEFAULT_ACCENT_DARK: u32 = 0x0A84FF;

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

/// The fixed tokens of one appearance before anything is derived from the accent.
struct Base {
    window_bg: u32,
    sidebar_bg: u32,
    panel_border: u32,
    text: u32,
    text_secondary: u32,
    divider: u32,
    field_bg: u32,
    control_bg: u32,
    control_border: u32,
    card_bg: u32,
    card_border: u32,
    selection_inactive: u32,
    wave_rest: u32,
    record_red: u32,
    record_glyph: u32,
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
    playing_alpha: f32,
    match_alpha: f32,
}

const LIGHT: Base = Base {
    window_bg: 0xFFFFFF,
    sidebar_bg: 0xF1F3F7,
    panel_border: 0xDDE0E6,
    text: 0x1D1D1F,
    text_secondary: 0x56585F,
    divider: 0xE1E3E8,
    field_bg: 0xE3E6EB,
    control_bg: 0xFFFFFF,
    control_border: 0xC9CCD3,
    card_bg: 0xF3F5FA,
    card_border: 0xE0E4EE,
    selection_inactive: 0xDADDE4,
    wave_rest: 0x8A90A0,
    record_red: 0xD70015,
    record_glyph: 0xD70015,
    attention: 0xA35400,
    attention_border: 0xE2B98A,
    avatar_bg: 0xE3E7F0,
    surface_raised: 0xFFFFFF,
    surface_border: 0xC3C7CF,
    surface_inset: 0xF3F5FA,
    inset_border: 0xE0E4EE,
    scrim: (0x0F1117, 0.16),
    scrim_opaque: 0xC4C7CE,
    close_bg: (0x000000, 0.05),
    track: 0xE1E4EA,
    group_bg: 0xF7F8FA,
    group_border: 0xE3E6EB,
    radio_border: 0xA9ADB6,
    playing_alpha: 0.07,
    match_alpha: 0.22,
};

const DARK: Base = Base {
    window_bg: 0x1A1B1F,
    sidebar_bg: 0x25262C,
    panel_border: 0x3A3B42,
    text: 0xF2F2F4,
    text_secondary: 0xAEB0B8,
    divider: 0x36373D,
    field_bg: 0x32333A,
    control_bg: 0x2C2D33,
    control_border: 0x4A4B52,
    card_bg: 0x212637,
    card_border: 0x323A52,
    selection_inactive: 0x393A41,
    wave_rest: 0x7C8193,
    record_red: 0xE0303A,
    record_glyph: 0xFF6961,
    attention: 0xFFB340,
    attention_border: 0x8A6220,
    avatar_bg: 0x323A52,
    surface_raised: 0x2B2C32,
    surface_border: 0x777A84,
    surface_inset: 0x33343B,
    inset_border: 0x45464E,
    scrim: (0x000000, 0.48),
    scrim_opaque: 0x0B0B0D,
    close_bg: (0xFFFFFF, 0.08),
    track: 0x45464E,
    group_bg: 0x212226,
    group_border: 0x34353B,
    radio_border: 0x5A5B63,
    playing_alpha: 0.12,
    match_alpha: 0.35,
};

/// The glass tints of `docs/design/settings-icons-blur.md` section 5, as color and alpha, light
/// then dark. The brief's sidebar tint, white at 0.28 and 0.05, fails 4.5 to 1 for secondary text
/// on a black wallpaper in light and a white one in dark. The sidebar tint is raised to the
/// lowest alpha at which every sidebar text color passes for all eight accents, which the pink
/// and blue accent text set, and in dark it takes the solid sidebar's color: white could only
/// lighten it.
const GLASS_TINT: [(u32, f32); 2] = [(0xECEFF5, 0.55), (0x1E1F24, 0.55)];
const SIDEBAR_TINT: [(u32, f32); 2] = [(0xFFFFFF, 0.73), (0x25262C, 0.79)];

fn tint((hex, alpha): (u32, f32)) -> Hsla {
    Hsla { a: alpha, ..c(hex) }
}

/// Increase Contrast's secondary text, which also draws every border (handoff section 8).
const HIGH_CONTRAST_SECONDARY: (u32, u32) = (0x3C3E44, 0xC8CAD0);

impl Theme {
    /// `system_accent` is `NSColor.controlAccentColor` in sRGB.
    pub fn new(dark: bool, system_accent: Hsla, display: Display) -> Theme {
        let base = if dark { &DARK } else { &LIGHT };
        let high = display.increase_contrast;
        let window_bg = c(base.window_bg);
        let sidebar_bg = c(base.sidebar_bg);
        let card_bg = c(base.card_bg);
        let control_bg = c(base.control_bg);
        let field_bg = c(base.field_bg);
        let selection_inactive = c(base.selection_inactive);
        let group_bg = c(base.group_bg);
        let surface_raised = c(base.surface_raised);
        let surface_inset = c(base.surface_inset);
        let on_accent = c(0xFFFFFF);
        let text_secondary = if high {
            c(if dark {
                HIGH_CONTRAST_SECONDARY.1
            } else {
                HIGH_CONTRAST_SECONDARY.0
            })
        } else {
            c(base.text_secondary)
        };
        let border = |normal: u32| if high { text_secondary } else { c(normal) };
        let playing_bg = blend(system_accent, window_bg, base.playing_alpha);
        let search_match = blend(system_accent, sidebar_bg, base.match_alpha);
        let reading = [
            window_bg,
            sidebar_bg,
            card_bg,
            control_bg,
            field_bg,
            selection_inactive,
            playing_bg,
            group_bg,
            surface_raised,
            surface_inset,
        ];
        let accent = with_contrast(system_accent, on_accent);
        let record_red = c(base.record_red);
        let attention = with_contrast_all(c(base.attention), &reading, MIN_CONTRAST);
        let wave_floor = if high {
            MIN_CONTRAST
        } else {
            MIN_NON_TEXT_CONTRAST
        };
        let waves = [card_bg, surface_inset];
        let wave_rest = with_contrast_all(c(base.wave_rest), &waves, wave_floor);
        let wave_done = with_contrast_all(
            if dark { system_accent } else { accent },
            &waves,
            wave_floor,
        );
        Theme {
            dark,
            high_contrast: high,
            differentiate_without_color: display.differentiate_without_color,
            shadows: !high,
            window_bg,
            sidebar_bg,
            glass_tint: tint(GLASS_TINT[dark as usize]),
            sidebar_tint: tint(SIDEBAR_TINT[dark as usize]),
            panel_border: border(base.panel_border),
            text: c(base.text),
            text_secondary,
            divider: border(base.divider),
            field_bg,
            control_bg,
            control_border: border(base.control_border),
            card_bg,
            card_border: border(base.card_border),
            accent,
            accent_text: with_contrast_all(system_accent, &reading, MIN_CONTRAST),
            ring: if dark {
                with_contrast_all(system_accent, &reading, MIN_CONTRAST)
            } else {
                accent
            },
            on_accent,
            selection_inactive,
            wave_rest,
            wave_done,
            wave_track: if high {
                card_bg
            } else {
                blend(wave_rest, card_bg, 0.35)
            },
            record_red,
            on_record: readable_on(record_red),
            record_glyph: with_contrast_all(c(base.record_glyph), &reading, MIN_CONTRAST),
            attention,
            attention_border: if high {
                attention
            } else {
                c(base.attention_border)
            },
            on_attention: readable_on(attention),
            meter_off: blend(readable_on(record_red), record_red, 0.4),
            playing_bg,
            search_match,
            avatar_bg: c(base.avatar_bg),
            surface_raised,
            surface_border: border(base.surface_border),
            surface_inset,
            inset_border: border(base.inset_border),
            scrim: tint(base.scrim),
            scrim_opaque: c(base.scrim_opaque),
            close_bg: if high {
                gpui_kit::transparent_black()
            } else {
                tint(base.close_bg)
            },
            track: c(base.track),
            group_bg,
            group_border: border(base.group_border),
            radio_border: with_contrast_all(
                c(base.radio_border),
                &[control_bg, group_bg, window_bg],
                MIN_NON_TEXT_CONTRAST,
            ),
            chip_bg: c(base.avatar_bg),
            text_scale: clamp_text_scale(display.text_scale),
        }
    }

    pub fn light(system_accent: Hsla) -> Theme {
        Theme::new(false, system_accent, Display::default())
    }

    pub fn dark(system_accent: Hsla) -> Theme {
        Theme::new(true, system_accent, Display::default())
    }

    pub fn for_appearance(
        appearance: WindowAppearance,
        system_accent: Option<Hsla>,
        display: Display,
    ) -> Theme {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme::new(
                true,
                system_accent.unwrap_or_else(|| c(DEFAULT_ACCENT_DARK)),
                display,
            ),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::new(
                false,
                system_accent.unwrap_or_else(|| c(DEFAULT_ACCENT_LIGHT)),
                display,
            ),
        }
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

    /// The focus ring (2 pt [`Theme::ring`]). On an `accent` fill it switches to `on_accent`, or
    /// the ring would vanish into the fill (spike finding, lane 4).
    pub fn focus_ring(&self, on_accent_fill: bool) -> Hsla {
        if on_accent_fill {
            self.on_accent
        } else {
            self.ring
        }
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

/// Installs the theme for the current appearance, accent, and [`Display`], and keeps GPUI Kit's
/// own components in step in every appearance: the same mode, the direction C focus ring, the
/// floating-surface fill, text, scrim, and dialog radius (`docs/design/floating-surfaces.md`
/// section 6), Increase Contrast's borders and missing shadows, and the text scale as Kit's rem
/// size. Call it at launch and on every change of any input.
pub fn sync(cx: &mut App) {
    let display = cx.try_global::<Display>().copied().unwrap_or_default();
    let appearance = cx.window_appearance();
    let theme = Theme::for_appearance(appearance, crate::platform::system_accent(), display);
    cx.set_global(theme);
    gpui_kit::component::Theme::change(appearance, None, cx);
    let scrim = if crate::glass::reduces_transparency(cx) {
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
        if theme.high_contrast {
            kit.border = theme.control_border;
            kit.input = theme.control_border;
            kit.muted_foreground = theme.text_secondary;
        }
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

/// White text on `fill` when it passes, else the dark text color.
pub fn readable_on(fill: Hsla) -> Hsla {
    let white = c(0xFFFFFF);
    if contrast(white, fill) >= MIN_CONTRAST {
        white
    } else {
        c(LIGHT.text)
    }
}

/// A11Y-8: moves `color`'s HSL lightness away from `against` in 1% steps until the pair passes
/// [`MIN_CONTRAST`]. Against a light color it darkens, against a dark one it lightens, so the
/// same rule derives `accent` (under white text) and `accent_text` (on either background).
pub fn with_contrast(color: Hsla, against: Hsla) -> Hsla {
    with_contrast_all(color, &[against], MIN_CONTRAST)
}

/// [`with_contrast`] against every background the color is painted on, all of which share one
/// lightness side, until the worst pair reaches `min`.
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

    #[test]
    fn the_default_blue_fails_as_is_and_is_darkened_only_as_far_as_needed() {
        let blue = c(DEFAULT_ACCENT_LIGHT);
        let white = c(0xFFFFFF);
        assert!(contrast(blue, white) < MIN_CONTRAST);
        let accent = Theme::light(blue).accent;
        assert!(accent.l < blue.l);
        let one_step_back = Hsla {
            l: accent.l + 0.01,
            ..accent
        };
        assert!(contrast(one_step_back, white) < MIN_CONTRAST);
    }

    #[test]
    fn dark_accent_text_is_lightened_against_the_dark_window() {
        let blue = c(DEFAULT_ACCENT_DARK);
        let theme = Theme::dark(blue);
        assert!(theme.accent_text.l >= blue.l);
        assert!(theme.accent.l <= blue.l);
    }

    #[test]
    fn the_secondary_text_token_meets_its_stated_floor() {
        for theme in [
            Theme::light(c(DEFAULT_ACCENT_LIGHT)),
            Theme::dark(c(DEFAULT_ACCENT_DARK)),
        ] {
            for bg in [theme.window_bg, theme.sidebar_bg, theme.selection_inactive] {
                let ratio = contrast(theme.text_secondary, bg);
                assert!(ratio >= 5.2, "{ratio:.2} dark={}", theme.dark);
            }
        }
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

    #[test]
    fn increase_contrast_drops_shadows_and_draws_borders_in_secondary_text() {
        let display = Display {
            increase_contrast: true,
            ..Display::default()
        };
        let theme = Theme::new(false, c(DEFAULT_ACCENT_LIGHT), display);
        assert!(!theme.shadows);
        assert_eq!(theme.control_border, theme.text_secondary);
        assert_eq!(theme.panel_border, theme.text_secondary);
        assert!(contrast(theme.wave_rest, theme.card_bg) >= MIN_CONTRAST);
    }
}
