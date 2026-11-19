//! The two gears that lead to Settings: the button at the sidebar's bottom left, which sends the
//! same `OpenSettings` as Command-Comma, and the icon before the Processing Queue's
//! "Processing Settings…" link.

mod common;

use dory_gpui::prefs::{MIN_WINDOW_SIZE, Prefs};
use dory_gpui::processing_window;
use dory_gpui::theme::ThemeName;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Pixels, Role, TestAppContext,
    test::TestWindowExt as _,
};

use common::{dory_on, launch, past_first_run, sample_store};

const GEAR: &str = "sidebar-settings";

fn bounds(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Bounds<Pixels> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id.to_owned()).bounds()
    })
    .unwrap()
}

fn new_windows(before: &[AnyWindowHandle], cx: &mut TestAppContext) -> Vec<AnyWindowHandle> {
    cx.update(|cx| cx.windows())
        .into_iter()
        .filter(|w| !before.contains(w))
        .collect()
}

fn is_settings(window: AnyWindowHandle, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find("settings-panes").is_some()
    })
    .unwrap_or(false)
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

/// The gear sits under the list, inside the panel's bottom-left corner, and is on screen.
fn assert_pinned_bottom_left(window: AnyWindowHandle, cx: &mut TestAppContext) {
    let panel = bounds(window, "sidebar", cx);
    let list = bounds(window, "recordings", cx);
    let gear = bounds(window, GEAR, cx);
    assert!(gear.top() >= list.bottom(), "{gear:?} is below {list:?}");
    assert!(
        panel.bottom() - gear.bottom() <= Pixels::from(12.),
        "{gear:?} sits at the bottom of {panel:?}"
    );
    assert!(
        gear.left() - panel.left() <= Pixels::from(16.),
        "{gear:?} sits at the left of {panel:?}"
    );
    assert!(gear.bottom() <= panel.bottom() && gear.right() <= panel.right());
}

#[gpui_kit::test]
fn the_sidebar_gear_is_a_settings_button_at_the_bottom_left(cx: &mut TestAppContext) {
    let (window, _app) = launch(cx, dory_on(sample_store()), past_first_run());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let gear = window.find(GEAR);
        assert_eq!(gear.role(), Some(Role::Button));
        assert_eq!(gear.label(), Some("Settings"));
    })
    .unwrap();
    assert_pinned_bottom_left(window, cx);
}

#[gpui_kit::test]
fn the_gear_stays_in_the_corner_in_book_at_200_percent_in_the_smallest_window(
    cx: &mut TestAppContext,
) {
    let prefs = Prefs {
        theme: ThemeName::Book,
        text_scale: 2.0,
        window_size: MIN_WINDOW_SIZE,
        ..past_first_run()
    };
    let (window, _app) = launch(cx, dory_on(sample_store()), prefs);
    assert_pinned_bottom_left(window, cx);
}

#[gpui_kit::test]
fn clicking_the_gear_opens_settings_once_as_command_comma_does(cx: &mut TestAppContext) {
    let (window, _app) = launch(cx, dory_on(sample_store()), past_first_run());
    let before = cx.update(|cx| cx.windows());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(ElementId::Name(GEAR.into()), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let opened = new_windows(&before, cx);
    assert_eq!(opened.len(), 1, "the gear opens one window");
    assert!(is_settings(opened[0], cx), "the gear opens Settings");

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(ElementId::Name(GEAR.into()), cx);
    })
    .unwrap();
    cx.run_until_parked();
    press(window, "cmd-,", cx);
    assert_eq!(
        new_windows(&before, cx),
        opened,
        "the gear and Command-Comma bring back the one Settings window"
    );
}

#[gpui_kit::test]
fn tab_reaches_the_gear_and_space_or_return_opens_settings(cx: &mut TestAppContext) {
    let (window, _app) = launch(cx, dory_on(sample_store()), past_first_run());
    let focused = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.find(GEAR).focused()
        })
        .unwrap()
    };
    let mut reached = false;
    for _ in 0..12 {
        press(window, "tab", cx);
        if focused(cx) == Some(true) {
            reached = true;
            break;
        }
    }
    assert!(reached, "Tab reaches the gear");

    for key in ["space", "enter"] {
        let before = cx.update(|cx| cx.windows());
        press(window, key, cx);
        let opened = new_windows(&before, cx);
        assert_eq!(opened.len(), 1, "{key} on the gear opens one window");
        assert!(
            is_settings(opened[0], cx),
            "{key} on the gear opens Settings"
        );
        cx.update_window(opened[0], |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, _| window.activate_window())
            .unwrap();
        assert_eq!(focused(cx), Some(true), "focus stays on the gear");
    }
}

#[gpui_kit::test]
fn the_queue_settings_link_starts_with_a_gear(cx: &mut TestAppContext) {
    let (_, app) = launch(cx, dory_on(sample_store()), past_first_run());
    cx.update(processing_window::install);
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| processing_window::open(&app, cx));
    cx.run_until_parked();
    let queue = new_windows(&before, cx)[0];
    let link = bounds(queue, "queue-settings", cx);
    let gear = bounds(queue, "queue-settings-icon", cx);
    assert_eq!(gear.size.width, gear.size.height, "the gear is square");
    assert!(
        link.contains(&gear.origin) && link.contains(&gear.bottom_right()),
        "{gear:?} is inside {link:?}"
    );
    assert!(
        gear.left() - link.left() <= Pixels::from(8.),
        "the gear comes first: {gear:?} in {link:?}"
    );
    cx.update_window(queue, |_, window, _| {
        assert_eq!(
            window.find("queue-settings").label(),
            Some("Processing Settings…"),
            "VoiceOver still reads the words, not the icon"
        );
    })
    .unwrap();
}
