//! Window glass follows the display settings live (A11Y-6, `docs/design/settings-icons-blur.md`
//! section 5): Reduce Transparency and Increase Contrast each turn the window opaque and the
//! sidebar solid, and turning them off brings the glass back.

mod common;

use common::{dory_on, past_first_run, sample_store};
use dory_gpui::glass;
use dory_gpui::system_prefs::{self, SystemPrefs};
use dory_gpui::theme::Theme;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, TestAppContext, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, px, size,
};

fn open(cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.update(gpui_kit::init);
    let window = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1100.), px(760.)),
                })),
                window_background: glass::window_background(cx),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let (_, shell) =
                    dory_gpui::build(dory_on(sample_store()), past_first_run(), window, cx);
                glass::follow(window, cx);
                shell
            },
        )
        .expect("open the window")
        .0
    });
    cx.run_until_parked();
    window
}

#[derive(Debug, PartialEq)]
struct Look {
    window: Option<WindowBackgroundAppearance>,
    column: gpui_kit::Hsla,
    panel: gpui_kit::Hsla,
}

fn look(window: AnyWindowHandle, cx: &mut TestAppContext) -> (Look, Theme) {
    cx.update_window(window, |_, window, cx| {
        let theme = Theme::get(cx);
        let look = Look {
            window: glass::applied(window, cx),
            column: glass::sidebar_column(&theme, cx),
            panel: glass::sidebar_panel(&theme, cx),
        };
        (look, theme)
    })
    .unwrap()
}

fn set(prefs: SystemPrefs, cx: &mut TestAppContext) {
    cx.update(|cx| system_prefs::apply(prefs, cx));
    cx.run_until_parked();
}

fn glass_look(theme: &Theme) -> Look {
    Look {
        window: Some(WindowBackgroundAppearance::Blurred),
        column: theme.glass_tint,
        panel: theme.sidebar_tint,
    }
}

fn solid_look(theme: &Theme) -> Look {
    Look {
        window: Some(WindowBackgroundAppearance::Opaque),
        column: theme.window_bg,
        panel: theme.sidebar_bg,
    }
}

#[gpui_kit::test]
fn reduce_transparency_turns_the_glass_off_and_back_on(cx: &mut TestAppContext) {
    let window = open(cx);
    let (now, theme) = look(window, cx);
    assert_eq!(now, glass_look(&theme), "the window opens blurred");

    set(
        SystemPrefs {
            reduce_transparency: true,
            ..SystemPrefs::default()
        },
        cx,
    );
    let (now, theme) = look(window, cx);
    assert_eq!(now, solid_look(&theme));
    assert!(now.panel.a >= 1.0 && now.column.a >= 1.0);

    set(SystemPrefs::default(), cx);
    let (now, theme) = look(window, cx);
    assert_eq!(
        now,
        glass_look(&theme),
        "turning it off brings the glass back"
    );
}

#[gpui_kit::test]
fn increase_contrast_also_turns_the_glass_off_and_keeps_its_borders(cx: &mut TestAppContext) {
    let window = open(cx);
    set(
        SystemPrefs {
            increase_contrast: true,
            ..SystemPrefs::default()
        },
        cx,
    );
    let (now, theme) = look(window, cx);
    assert_eq!(now, solid_look(&theme));
    assert_eq!(
        theme.panel_border, theme.text_secondary,
        "Increase Contrast draws the panel border in secondary text"
    );
}
