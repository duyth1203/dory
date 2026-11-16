//! Direction C's tokens (section 2 of `docs/design/handoff-direction-c.md`) as one `Theme`, kept
//! as a GPUI global that follows the system appearance and accent color.
//!
//! Views read it with `Theme::get(cx)` and never hard-code a color or a text size. A11Y-6's
//! high-contrast set and A11Y-7's text scale land here in P7.

use gpui_kit::{App, Global, Hsla, Pixels, Rgba, Window, WindowAppearance, px, rgb, rgba};

use crate::library::Tone;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
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
    pub on_accent: Hsla,
    pub selection_inactive: Hsla,
    pub wave_rest: Hsla,
    pub record_red: Hsla,
    pub record_glyph: Hsla,
    pub attention: Hsla,
    pub playing_bg: Hsla,
    pub avatar_bg: Hsla,
    pub toast_bg: Hsla,
    pub menu_bg: Hsla,
    pub menu_border: Hsla,
    /// View > Bigger and Smaller (A11Y-7), 0.85 to 2.0. Every text size goes through
    /// [`Theme::text`].
    pub text_scale: f32,
}

impl Global for Theme {}

/// macOS's default blue, used when AppKit has no accent to offer (tests, other platforms).
pub const DEFAULT_ACCENT_LIGHT: u32 = 0x007AFF;
pub const DEFAULT_ACCENT_DARK: u32 = 0x0A84FF;

/// WCAG 2.2 AA for body text.
pub const MIN_CONTRAST: f32 = 4.5;

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    /// `system_accent` is `NSColor.controlAccentColor` in sRGB.
    pub fn light(system_accent: Hsla) -> Theme {
        let window_bg = c(0xFFFFFF);
        let on_accent = c(0xFFFFFF);
        let accent = with_contrast(system_accent, on_accent);
        Theme {
            dark: false,
            window_bg,
            sidebar_bg: c(0xF1F3F7),
            panel_border: c(0xDDE0E6),
            text: c(0x1D1D1F),
            text_secondary: c(0x56585F),
            divider: c(0xE1E3E8),
            field_bg: c(0xE3E6EB),
            control_bg: c(0xFFFFFF),
            control_border: c(0xC9CCD3),
            card_bg: c(0xF3F5FA),
            card_border: c(0xE0E4EE),
            accent,
            accent_text: with_contrast(system_accent, window_bg),
            on_accent,
            selection_inactive: c(0xDADDE4),
            wave_rest: c(0x8A90A0),
            record_red: c(0xD70015),
            record_glyph: c(0xD70015),
            attention: c(0xA35400),
            playing_bg: Hsla { a: 0.07, ..accent },
            avatar_bg: c(0xE3E7F0),
            toast_bg: c(0x2C2D31),
            menu_bg: c(0xFFFFFF),
            menu_border: c(0xD5D8DE),
            text_scale: 1.0,
        }
    }

    pub fn dark(system_accent: Hsla) -> Theme {
        let window_bg = c(0x1A1B1F);
        let on_accent = c(0xFFFFFF);
        let accent_text = with_contrast(system_accent, window_bg);
        Theme {
            dark: true,
            window_bg,
            sidebar_bg: c(0x25262C),
            panel_border: c(0x3A3B42),
            text: c(0xF2F2F4),
            text_secondary: c(0xAEB0B8),
            divider: c(0x36373D),
            field_bg: c(0x32333A),
            control_bg: c(0x2C2D33),
            control_border: c(0x4A4B52),
            card_bg: c(0x212637),
            card_border: c(0x323A52),
            accent: with_contrast(system_accent, on_accent),
            accent_text,
            on_accent,
            selection_inactive: c(0x393A41),
            wave_rest: c(0x6E7385),
            record_red: c(0xE0303A),
            record_glyph: c(0xFF6961),
            attention: c(0xFFB340),
            playing_bg: Hsla {
                a: 0.12,
                ..accent_text
            },
            avatar_bg: c(0x323A52),
            toast_bg: c(0x3A3B41),
            menu_bg: c(0x2C2D33),
            menu_border: c(0x45464D),
            text_scale: 1.0,
        }
    }

    pub fn for_appearance(appearance: WindowAppearance, system_accent: Option<Hsla>) -> Theme {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => {
                Theme::dark(system_accent.unwrap_or_else(|| c(DEFAULT_ACCENT_DARK)))
            }
            WindowAppearance::Light | WindowAppearance::VibrantLight => {
                Theme::light(system_accent.unwrap_or_else(|| c(DEFAULT_ACCENT_LIGHT)))
            }
        }
    }

    pub fn get(cx: &App) -> Theme {
        *cx.global::<Theme>()
    }

    /// A text size in points, scaled by View > Bigger and Smaller.
    pub fn text(&self, points: f32) -> Pixels {
        px(points * self.text_scale)
    }

    /// The focus ring (2 pt `accent`). On an `accent` fill it switches to `on_accent`, or the
    /// ring would vanish into the fill (spike finding, lane 4).
    pub fn focus_ring(&self, on_accent_fill: bool) -> Hsla {
        if on_accent_fill {
            self.on_accent
        } else {
            self.accent
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

    pub fn sidebar_shadow_color(&self) -> Hsla {
        rgba(0x00000014).into()
    }
}

/// Installs the theme for `window`'s appearance and the current accent, and keeps GPUI Kit's own
/// components in the same mode. Call it at launch and from `observe_window_appearance`.
pub fn sync(window: &mut Window, cx: &mut App) {
    let appearance = window.appearance();
    cx.set_global(Theme::for_appearance(
        appearance,
        crate::platform::system_accent(),
    ));
    gpui_kit::component::Theme::change(appearance, Some(window), cx);
    window.refresh();
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

/// A11Y-8: moves `color`'s HSL lightness away from `against` in 1% steps until the pair passes
/// [`MIN_CONTRAST`]. Against a light color it darkens, against a dark one it lightens, so the
/// same rule derives `accent` (under white text) and `accent_text` (on either background).
pub fn with_contrast(color: Hsla, against: Hsla) -> Hsla {
    let step = if luminance(against.into()) > 0.18 {
        -0.01
    } else {
        0.01
    };
    let mut out = Hsla { a: 1.0, ..color };
    while contrast(out, against) < MIN_CONTRAST && (0.0..=1.0).contains(&(out.l + step)) {
        out.l += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The system accents in sRGB, light then dark: blue, purple, pink, red, orange, yellow,
    /// green, graphite.
    const ACCENTS: [(u32, u32); 8] = [
        (0x007AFF, 0x0A84FF),
        (0xAF52DE, 0xBF5AF2),
        (0xFF2D55, 0xFF375F),
        (0xFF3B30, 0xFF453A),
        (0xFF9500, 0xFF9F0A),
        (0xFFCC00, 0xFFD60A),
        (0x34C759, 0x30D158),
        (0x8E8E93, 0x98989D),
    ];

    #[test]
    fn every_macos_accent_passes_aa_in_both_appearances() {
        for (light, dark) in ACCENTS {
            for theme in [Theme::light(c(light)), Theme::dark(c(dark))] {
                let pairs = [
                    ("on_accent on accent", theme.on_accent, theme.accent),
                    ("accent_text on window", theme.accent_text, theme.window_bg),
                ];
                for (what, fg, bg) in pairs {
                    let ratio = contrast(fg, bg);
                    assert!(
                        ratio >= MIN_CONTRAST,
                        "{what} is {ratio:.2} for {light:06X}/{dark:06X}, dark={}",
                        theme.dark
                    );
                }
            }
        }
    }

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
}
