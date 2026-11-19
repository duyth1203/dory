//! A11Y-8: every color pair the views paint, `Theme::pairs`, meets WCAG 2.2 AA (4.5 to 1 for
//! text, 3 to 1 for focus rings and graphics) in light and dark, with and without Increase
//! Contrast (A11Y-6) and Differentiate Without Color.

use dory_gpui::theme::{Display, Theme, contrast};

/// Every theme the app can build from the system settings.
fn every_theme() -> Vec<(String, Theme)> {
    let mut themes = Vec::new();
    for increase_contrast in [false, true] {
        for differentiate_without_color in [false, true] {
            let display = Display {
                increase_contrast,
                differentiate_without_color,
                text_scale: 1.0,
            };
            let variant = format!(
                "{}{}",
                if increase_contrast {
                    "increase contrast"
                } else {
                    ""
                },
                if differentiate_without_color {
                    ", differentiate without color"
                } else {
                    ""
                }
            );
            themes.push((format!("light {variant}"), Theme::new(false, display)));
            themes.push((format!("dark {variant}"), Theme::new(true, display)));
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
    assert!(checked > 500, "only {checked} pairs checked");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn increase_contrast_makes_borders_stronger_and_drops_shadows() {
    for dark in [false, true] {
        let normal = Theme::new(dark, Display::default());
        let high = Theme::new(
            dark,
            Display {
                increase_contrast: true,
                ..Display::default()
            },
        );
        assert!(
            contrast(high.control_border, high.control_bg)
                > contrast(normal.control_border, normal.control_bg),
            "dark={dark}"
        );
        assert!(!high.shadows && normal.shadows);
    }
}
