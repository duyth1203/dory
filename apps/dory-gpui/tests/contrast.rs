//! A11Y-8: every color pair the views paint, `Theme::pairs`, meets WCAG 2.2 AA (4.5 to 1 for
//! text, 3 to 1 for focus rings and graphics) for all eight macOS accent colors, in light and
//! dark, with and without Increase Contrast (A11Y-6).

use dory_gpui::theme::{Display, MIN_CONTRAST, Theme, contrast};
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
    for (name, light, dark) in ACCENTS {
        for increase_contrast in [false, true] {
            for differentiate_without_color in [false, true] {
                let display = Display {
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
