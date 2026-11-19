//! The visual pass's contrast findings: the focus ring on the selected sidebar row, the outline of
//! a disabled radio or checkbox, Increase Contrast's control outlines, focus ring and search
//! marks, and the search field fill.

use dory_gpui::combobox;
use dory_gpui::marks;
use dory_gpui::sidebar;
use dory_gpui::theme::{
    ATTENTION_BORDER_MIN, DISABLED_OUTLINE_HIGH_MIN, Display, MIN_CONTRAST, MIN_NON_TEXT_CONTRAST,
    Theme, contrast,
};
use gpui_kit::{FontWeight, Hsla, Styled as _, TestAppContext, px};

/// Every theme the system can ask for: both appearances, Increase Contrast on and off.
fn every_theme() -> Vec<(String, Theme)> {
    let mut themes = Vec::new();
    for increase_contrast in [false, true] {
        let display = Display {
            increase_contrast,
            ..Display::default()
        };
        for (appearance, dark) in [("light", false), ("dark", true)] {
            themes.push((
                format!("{appearance}{}", if increase_contrast { " ic" } else { "" }),
                Theme::new(dark, display),
            ));
        }
    }
    themes
}

fn control_fills(theme: &Theme) -> [(&'static str, Hsla); 3] {
    [
        ("control_bg", theme.control_bg),
        ("group_bg", theme.group_bg),
        ("window_bg", theme.window_bg),
    ]
}

#[test]
fn the_ring_on_the_selected_row_shows_against_the_sidebar_and_the_fill() {
    let mut failures = Vec::new();
    for (name, theme) in every_theme() {
        let ring = theme.row_ring();
        // The edge is the band outside the line and the row's own fill is the band inside it, and
        // both are `accent`. So one comparison tells the line from both, and the edge has to be
        // told apart from the sidebar's paper. A white line that touched a white sidebar was the
        // bug.
        assert_eq!(
            ring.edge, theme.accent,
            "{name}: the bands are the row's fill"
        );
        for (what, a, b, min) in [
            (
                "line on the accent bands",
                ring.line,
                ring.edge,
                MIN_NON_TEXT_CONTRAST,
            ),
            (
                "edge on the sidebar",
                ring.edge,
                theme.sidebar_bg,
                ATTENTION_BORDER_MIN,
            ),
        ] {
            let ratio = contrast(a, b);
            if ratio < min {
                failures.push(format!("{name}: {what} is {ratio:.2}, needs {min}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_selected_rows_two_bands_and_line_take_their_colors_from_row_ring() {
    for (name, theme) in every_theme() {
        let ring = theme.row_ring();
        assert_eq!(sidebar::row_edge(&theme, true), ring.edge, "{name}");
        assert_eq!(
            sidebar::row_edge(&theme, false),
            gpui_kit::transparent_black(),
            "{name}: only the filled row has an edge"
        );
        let mut line = sidebar::row_ring_line("row-x", &theme);
        assert_eq!(line.style().border_color, Some(ring.line), "{name}");
    }
}

#[test]
fn a_disabled_radio_or_checkbox_outline_still_shows_on_its_row() {
    let mut failures = Vec::new();
    for (name, theme) in every_theme() {
        let floor = if theme.high_contrast {
            DISABLED_OUTLINE_HIGH_MIN
        } else {
            ATTENTION_BORDER_MIN
        };
        for (fill, bg) in control_fills(&theme) {
            let ratio = contrast(theme.disabled_outline(), bg);
            if ratio < floor {
                failures.push(format!(
                    "{name}: outline on {fill} is {ratio:.2}, needs {floor}"
                ));
            }
        }
        // Dimmed means weaker than the live control, not absent.
        for (fill, bg) in control_fills(&theme) {
            if contrast(theme.disabled_outline(), bg) > contrast(theme.radio_border, bg) {
                failures.push(format!(
                    "{name}: disabled outline is stronger than the live one on {fill}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn under_increase_contrast_radio_outlines_strengthen_like_separators() {
    for (appearance, dark) in [("light", false), ("dark", true)] {
        let build = |increase_contrast| {
            Theme::new(
                dark,
                Display {
                    increase_contrast,
                    ..Display::default()
                },
            )
        };
        let (standard, high) = (build(false), build(true));
        assert_eq!(high.radio_border, high.divider, "{appearance}");
        for ((fill, bg), (_, standard_bg)) in control_fills(&high)
            .into_iter()
            .zip(control_fills(&standard))
        {
            assert!(
                contrast(high.radio_border, bg) >= MIN_CONTRAST,
                "{appearance}: radio outline on {fill}"
            );
            assert!(
                contrast(high.radio_border, bg) > contrast(standard.radio_border, standard_bg),
                "{appearance}: the outline on {fill} is no stronger than the standard one"
            );
        }
    }
}

#[test]
fn under_increase_contrast_the_focus_ring_is_the_text_color() {
    for (name, theme) in every_theme() {
        if theme.high_contrast {
            assert_eq!(theme.ring, theme.text, "{name}");
            assert!(contrast(theme.ring, theme.window_bg) >= 7.0, "{name}");
        } else {
            assert_ne!(theme.ring, theme.text, "{name}");
        }
    }
}

#[test]
fn a_focused_language_field_is_told_from_an_unfocused_one_in_every_variant() {
    let mut failures = Vec::new();
    for (name, theme) in every_theme() {
        let focus = combobox::focus_edge(&theme);
        assert_eq!(focus.color, theme.ring, "{name}: the ring color");
        // Under Increase Contrast the resting border (`text_secondary`) and the ring (`text`) are
        // about 1.4 to 1, so recoloring the 1 pt border told focus from rest by almost nothing.
        // The ring has to add width, outside the border, to be seen.
        if focus.halo < px(2.) {
            failures.push(format!("{name}: the ring adds only {:?}", focus.halo));
        }
        for (what, around) in [
            ("window_bg", theme.window_bg),
            ("group_bg", theme.group_bg),
            ("card_bg", theme.card_bg),
        ] {
            let ratio = contrast(focus.color, around);
            if ratio < MIN_NON_TEXT_CONTRAST {
                failures.push(format!(
                    "{name}: the ring on {what} is {ratio:.2}, needs {MIN_NON_TEXT_CONTRAST}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn under_increase_contrast_a_search_mark_has_a_stronger_fill_and_an_underline() {
    for (appearance, dark) in [("light", false), ("dark", true)] {
        let build = |increase_contrast| {
            Theme::new(
                dark,
                Display {
                    increase_contrast,
                    ..Display::default()
                },
            )
        };
        let (standard, high) = (build(false), build(true));
        for (theme, expect_underline) in [(&standard, false), (&high, true)] {
            let style = marks::style(FontWeight::BOLD, false, theme);
            assert_eq!(
                style.underline.is_some(),
                expect_underline,
                "{appearance}: the underline appears only under Increase Contrast"
            );
            if let Some(underline) = style.underline {
                assert_eq!(underline.color, Some(theme.text), "{appearance}");
            }
            assert_eq!(style.font_weight, Some(FontWeight::BOLD), "{appearance}");
        }
        // The fill only has to be stronger; the underline carries the rest.
        assert!(
            contrast(high.search_match, high.sidebar_bg)
                > contrast(standard.search_match, standard.sidebar_bg),
            "{appearance}: the fill is no stronger than the standard one"
        );
        assert!(
            contrast(high.text, high.search_match) >= MIN_CONTRAST,
            "{appearance}: text on the fill"
        );
    }
}

#[test]
fn a_mark_on_the_accent_row_is_underlined_in_on_accent() {
    for (name, theme) in every_theme() {
        let style = marks::style(FontWeight::BOLD, true, &theme);
        let underline = style.underline.expect(&name);
        assert_eq!(underline.color, Some(theme.on_accent), "{name}");
        assert_eq!(style.background_color, None, "{name}");
    }
}

#[gpui_kit::test]
fn search_fields_take_the_warm_control_fill_in_light(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        dory_gpui::theme::sync(cx);
        let ours = Theme::get(cx);
        let kit = gpui_kit::component::Theme::global(cx);
        assert!(!kit.is_dark(), "the test window is light");
        assert_eq!(
            kit.input_background(),
            ours.control_bg,
            "the field is #FFFDF7, not pure white"
        );
    });
}
