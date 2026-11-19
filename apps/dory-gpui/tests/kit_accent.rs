//! The accent from System Settings › Appearance: under Multicolor Dory keeps Book's ink, and a
//! chosen accent replaces it. Either way GPUI Kit's own controls take it too: a Kit primary
//! button, switch, checkbox, or slider, and every text field's caret and selection paint in it,
//! never in Kit's near-black `primary` or its fixed blue selection.

mod common;

use common::{open_with_prefs, past_first_run, sample_store};
use dory_gpui::system_prefs::{self, SystemPrefs};
use dory_gpui::theme::Theme;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, TestAppContext, rgb};

const INK: u32 = 0x2E4A86;
const PINK: u32 = 0xFF2D55;

/// Hue distance on the color wheel, 0 to 0.5.
fn hue_gap(a: Hsla, b: Hsla) -> f32 {
    let d = (a.h - b.h).abs();
    d.min(1.0 - d)
}

fn assert_hue(name: &str, color: Hsla, hue_of: Hsla) {
    assert!(
        color.s > 0.2 && hue_gap(color, hue_of) < 0.03,
        "{name} {color:?} keeps the hue of {hue_of:?}"
    );
}

/// Kit's accent-colored fields all come from Dory's accent.
fn assert_kit_follows(cx: &App) {
    let dory = Theme::get(cx);
    let kit = cx.theme();
    assert_eq!(kit.primary, dory.accent, "Kit's primary");
    assert_eq!(kit.primary_foreground, dory.on_accent);
    assert_eq!(kit.button_primary, dory.accent);
    assert_eq!(kit.button_primary_foreground, dory.on_accent);
    assert_eq!(kit.tokens.primary.color, dory.accent, "the tokens follow");
    for (name, color) in [
        ("primary_hover", kit.primary_hover),
        ("primary_active", kit.primary_active),
        ("button_primary_hover", kit.button_primary_hover),
        ("button_primary_active", kit.button_primary_active),
        ("caret", kit.caret),
        ("selection", kit.selection),
        ("link", kit.link),
    ] {
        assert_hue(name, color, dory.accent);
    }
    assert!(
        kit.selection.a < 1.0,
        "the selection stays see-through over the text"
    );
}

#[gpui_kit::test]
fn multicolor_keeps_the_ink_and_a_chosen_accent_replaces_it(cx: &mut TestAppContext) {
    open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    cx.update(|cx| {
        system_prefs::apply(SystemPrefs::default(), cx);
        assert_eq!(Theme::get(cx).accent, rgb(INK).into(), "Multicolor is ink");
        assert_kit_follows(cx);

        system_prefs::apply(
            SystemPrefs {
                accent: Some(PINK),
                ..SystemPrefs::default()
            },
            cx,
        );
        assert_hue("accent", Theme::get(cx).accent, rgb(PINK).into());
        assert_kit_follows(cx);

        system_prefs::apply(SystemPrefs::default(), cx);
        assert_eq!(
            Theme::get(cx).accent,
            rgb(INK).into(),
            "back to Multicolor, back to ink"
        );
    });
}
