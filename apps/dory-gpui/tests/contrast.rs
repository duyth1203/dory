//! A11Y-8: every color pair the views paint, `Theme::pairs`, meets WCAG 2.2 AA (4.5 to 1 for
//! text, 3 to 1 for focus rings and graphics) for all eight macOS accent colors, in light and
//! dark, with and without Increase Contrast (A11Y-6), in both themes. Book keeps its own accent,
//! so it is checked once per appearance and setting rather than once per accent.

use dory_gpui::theme::{Display, MIN_CONTRAST, Theme, ThemeName, blend, contrast};
use gpui_kit::{Hsla, rgb};

/// The system accents in sRGB, light then dark: blue, purple, pink, red, orange, yellow, green,
/// graphite.
const ACCENTS: [(&str, u32, u32); 8] = [
    ("blue", 0x007AFF, 0x0A84FF),
    ("purple", 0xAF52DE, 0xBF5AF2),
    ("pink", 0xFF2D55, 0xFF375F),
    ("red", 0xFF3B30, 0xFF453A),
    ("orange", 0xFF9500, 0xFF9F0A),
    ("yellow", 0xFFCC00, 0xFFD60A),
    ("green", 0x34C759, 0x30D158),
    ("graphite", 0x8E8E93, 0x98989D),
];

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Every theme the app can build from the system settings.
fn every_theme() -> Vec<(String, Theme)> {
    let mut themes = Vec::new();
    for (theme, accents) in [
        (ThemeName::Modern, &ACCENTS[..]),
        (ThemeName::Book, &ACCENTS[..1]),
    ] {
    for &(name, light, dark) in accents {
        let name = match theme {
            ThemeName::Modern => name.to_owned(),
            ThemeName::Book => "Book".to_owned(),
        };
        for increase_contrast in [false, true] {
            for differentiate_without_color in [false, true] {
                let display = Display {
                    theme,
                    increase_contrast,
                    differentiate_without_color,
                    text_scale: 1.0,
                };
                let variant = format!(
                    "{name}{}{}",
                    if increase_contrast {
                        ", increase contrast"
                    } else {
                        ""
                    },
                    if differentiate_without_color {
                        ", differentiate without color"
                    } else {
                        ""
                    }
                );
                themes.push((
                    format!("light {variant}"),
                    Theme::new(false, c(light), display),
                ));
                themes.push((
                    format!("dark {variant}"),
                    Theme::new(true, c(dark), display),
                ));
            }
        }
    }
    }
    themes
}

#[test]
fn every_token_pair_meets_its_floor_in_every_theme() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for (variant, theme) in every_theme() {
        for pair in theme.pairs() {
            checked += 1;
            let ratio = contrast(pair.fg, pair.bg);
            if ratio < pair.min {
                failures.push(format!(
                    "{variant}: {} is {ratio:.2}, needs {}",
                    pair.name, pair.min
                ));
            }
        }
    }
    assert!(checked > 1000, "only {checked} pairs checked");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_default_accent_is_close_to_the_handoffs_values() {
    let light = Theme::light(c(0x007AFF));
    let dark = Theme::dark(c(0x0A84FF));
    assert!(contrast(light.on_accent, light.accent) >= MIN_CONTRAST);
    assert!(contrast(dark.accent_text, dark.window_bg) >= MIN_CONTRAST);
    assert!(
        contrast(light.accent_text, light.selection_inactive) >= MIN_CONTRAST,
        "accent text stays readable on an inactive selected row"
    );
}

#[test]
fn increase_contrast_makes_secondary_text_and_borders_stronger() {
    for dark in [false, true] {
        let accent = c(if dark { 0x0A84FF } else { 0x007AFF });
        let normal = Theme::new(dark, accent, Display::default());
        let high = Theme::new(
            dark,
            accent,
            Display {
                increase_contrast: true,
                ..Display::default()
            },
        );
        assert!(
            contrast(high.text_secondary, high.window_bg)
                > contrast(normal.text_secondary, normal.window_bg),
            "dark={dark}"
        );
        assert!(
            contrast(high.control_border, high.control_bg)
                > contrast(normal.control_border, normal.control_bg),
            "dark={dark}"
        );
        assert!(!high.shadows && normal.shadows);
    }
}

/// The wallpapers behind the blurred window that are hardest on sidebar text: plain white, plain
/// black, and a saturated photo color, a sunset orange (#E8590C, HSL 22°, 90%, 48%).
const WALLPAPERS: [(&str, u32); 3] = [
    ("white", 0xFFFFFF),
    ("black", 0x000000),
    ("photo", 0xE8590C),
];

/// `top`, with its own alpha, over an opaque `under`.
fn over(top: Hsla, under: Hsla) -> Hsla {
    blend(top, under, top.a)
}

/// Every text color the sidebar paints straight on the panel: titles, the date and duration,
/// group headers, and the status words and glyphs in each tone.
fn sidebar_text(theme: &Theme) -> [(&'static str, Hsla); 5] {
    [
        ("text", theme.text),
        ("text_secondary", theme.text_secondary),
        ("accent_text", theme.accent_text),
        ("attention", theme.attention),
        ("record_glyph", theme.record_glyph),
    ]
}

/// The themes that draw glass: Increase Contrast turns it off.
fn glass_themes() -> Vec<(String, Theme)> {
    every_theme()
        .into_iter()
        .filter(|(_, theme)| !theme.high_contrast)
        .collect()
}

/// The lowest ratio any sidebar text reaches over any wallpaper, with the sidebar tint at
/// `alpha`, and where.
fn worst_on_glass(dark: bool, alpha: f32) -> (f32, String) {
    let mut worst = (f32::INFINITY, String::new());
    for (variant, theme) in glass_themes().into_iter().filter(|(_, t)| t.dark == dark) {
        let panel = Hsla {
            a: alpha,
            ..theme.sidebar_tint
        };
        for (wallpaper, hex) in WALLPAPERS {
            let bg = over(panel, over(theme.glass_tint, c(hex)));
            for (name, fg) in sidebar_text(&theme) {
                let ratio = contrast(fg, bg);
                if ratio < worst.0 {
                    worst = (ratio, format!("{variant}: {name} over {wallpaper}"));
                }
            }
        }
    }
    worst
}

#[test]
fn sidebar_text_passes_on_glass_over_white_black_and_a_photo() {
    for dark in [false, true] {
        let alpha = if dark {
            Theme::dark(c(0x0A84FF))
        } else {
            Theme::light(c(0x007AFF))
        }
        .sidebar_tint
        .a;
        let (ratio, at) = worst_on_glass(dark, alpha);
        let lowest_passing = (0..=100)
            .map(|step| step as f32 / 100.)
            .find(|alpha| worst_on_glass(dark, *alpha).0 >= MIN_CONTRAST);
        eprintln!(
            "glass dark={dark}: sidebar_tint alpha {alpha:.2}, worst {ratio:.2} ({at}); the lowest passing alpha is {lowest_passing:?}"
        );
        assert!(
            ratio >= MIN_CONTRAST,
            "dark={dark}: {at} is {ratio:.2}; raise sidebar_tint to {lowest_passing:?}"
        );
    }
}

#[test]
fn the_glass_behind_sidebar_text_is_at_least_seventy_percent_opaque() {
    for theme in [Theme::light(c(0x007AFF)), Theme::dark(c(0x0A84FF))] {
        let opacity = 1. - (1. - theme.glass_tint.a) * (1. - theme.sidebar_tint.a);
        assert!(opacity >= 0.7, "dark={}: {opacity:.2}", theme.dark);
    }
}
