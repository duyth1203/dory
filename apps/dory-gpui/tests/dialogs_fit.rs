//! A modal dialog never outgrows its window: at the smallest window and the largest text the
//! header and footer stay on screen and the body scrolls, so every button can be reached.

mod common;

use dory_gpui::app::AppState;
use dory_gpui::prefs::{MIN_WINDOW_SIZE, Prefs};
use dory_gpui::{fixtures, quit, set_up_summaries, surface};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Entity, Pixels, ScrollDelta,
    TestAppContext, point, px, test::TestWindowExt as _,
};

use common::{dory_with_capture, open_with_prefs, past_first_run, sample_store};

fn small_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    let prefs = Prefs {
        text_scale: 2.0,
        window_size: MIN_WINDOW_SIZE,
        ..past_first_run()
    };
    let dory = dory_with_capture(sample_store(), dory_core::testing::FakeCapture::new());
    open_with_prefs(cx, dory, prefs)
}

fn bounds_of(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Bounds<Pixels> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(ElementId::Name(id.to_owned().into()))
            .unwrap_or_else(|| panic!("{id} was not drawn"))
            .bounds()
    })
    .unwrap()
}

fn viewport(window: AnyWindowHandle, cx: &mut TestAppContext) -> (f32, f32) {
    cx.update_window(window, |_, window, _| {
        let size = window.viewport_size();
        (f32::from(size.width), f32::from(size.height))
    })
    .unwrap()
}

fn bottom(b: Bounds<Pixels>) -> f32 {
    f32::from(b.origin.y + b.size.height)
}

/// The dialog's surface lies inside the window and each button lies inside the surface.
fn assert_reachable(
    window: AnyWindowHandle,
    buttons: &[&str],
    what: &str,
    cx: &mut TestAppContext,
) {
    let (_, height) = viewport(window, cx);
    let dialog = bounds_of(window, "dialog-surface", cx);
    assert!(
        f32::from(dialog.origin.y) >= -0.5 && bottom(dialog) <= height + 0.5,
        "{what}: the dialog spans {:.0} to {:.0} in a window {height:.0} tall",
        f32::from(dialog.origin.y),
        bottom(dialog)
    );
    for id in buttons {
        let button = bounds_of(window, id, cx);
        assert!(
            bottom(button) <= bottom(dialog) + 0.5,
            "{what}: {id} ends at {:.0}, below the dialog's bottom at {:.0}",
            bottom(button),
            bottom(dialog)
        );
    }
}

#[gpui_kit::test]
fn the_quit_dialog_keeps_its_buttons_in_the_smallest_window_at_the_largest_text(
    cx: &mut TestAppContext,
) {
    let (window, app) = small_window(cx);
    cx.update_window(window, |_, window, cx| {
        let prompt = quit::prompt(&fixtures::busy()).unwrap();
        quit::ask(prompt, app.clone(), window, cx);
    })
    .unwrap();
    assert_reachable(window, &["quit-cancel", "quit-confirm"], "quit", cx);
}

#[gpui_kit::test]
fn set_up_summaries_keeps_its_buttons_in_the_smallest_window_at_the_largest_text(
    cx: &mut TestAppContext,
) {
    let (window, _app) = small_window(cx);
    for (step, buttons) in [
        ("U1", ["set-up-not-now", "set-up-download"]),
        ("U2", ["set-up-cancel", "set-up-background"]),
    ] {
        cx.update_window(window, |_, window, cx| {
            set_up_summaries::preview(fixtures::summaries_step(step).unwrap(), window, cx)
        })
        .unwrap();
        assert_reachable(window, &buttons, step, cx);
        cx.update_window(window, |_, window, cx| surface::close_dialog(window, cx))
            .unwrap();
    }
}

#[gpui_kit::test]
fn the_body_that_does_not_fit_scrolls_to_its_last_line(cx: &mut TestAppContext) {
    let (window, app) = small_window(cx);
    cx.update_window(window, |_, window, cx| {
        let prompt = quit::prompt(&fixtures::busy()).unwrap();
        quit::ask(prompt, app.clone(), window, cx);
    })
    .unwrap();
    let last = "quit-line-2";
    let body = bounds_of(window, "quit-body", cx);
    assert!(
        bottom(bounds_of(window, last, cx)) > bottom(body) + 0.5,
        "the last line starts out below the body, so the dialog has more to show"
    );
    for _ in 0..3 {
        cx.update_window(window, |_, window, cx| {
            window.scroll(
                "quit-body",
                ScrollDelta::Pixels(point(px(0.), px(-300.))),
                cx,
            )
        })
        .unwrap();
    }
    let line = bounds_of(window, last, cx);
    assert!(
        bottom(line) <= bottom(body) + 0.5,
        "scrolling brings the last line to {:.0}, inside the body's bottom at {:.0}",
        bottom(line),
        bottom(body)
    );
}

/// Settings' Transcription pane at `height` and `text_scale`, with the languages popover open.
/// Returns the window, the field's bounds, and the popover's.
fn languages_open(
    height: f32,
    text_scale: f32,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Bounds<Pixels>, Bounds<Pixels>) {
    cx.executor().allow_parking();
    let prefs = Prefs {
        text_scale,
        ..past_first_run()
    };
    let dory = dory_with_capture(sample_store(), dory_core::testing::FakeCapture::new());
    let (_, app) = open_with_prefs(cx, dory, prefs);
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| dory_gpui::settings_window::open(&app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Settings window opened");
    cx.simulate_window_resize(
        window,
        gpui_kit::size(px(dory_gpui::settings_window::WIDTH), px(height)),
    );
    common::views::click(
        window,
        &dory_gpui::settings_window::Pane::Transcription.id(),
        cx,
    );
    common::views::click(window, "add-language", cx);
    let field = bounds_of(window, "meeting-languages", cx);
    let popover = bounds_of(window, "meeting-languages-popover", cx);
    (window, field, popover)
}

/// The meeting-languages popover opens on the side of its field with more room, so it never
/// covers the chips it edits, and it is no taller than that room.
#[gpui_kit::test]
fn the_languages_popover_never_covers_its_field(cx: &mut TestAppContext) {
    let (window, field, popover) = languages_open(dory_gpui::settings_window::MIN_HEIGHT, 2.0, cx);
    let (_, height) = viewport(window, cx);
    assert!(
        bottom(popover) <= f32::from(field.origin.y) + 0.5
            || f32::from(popover.origin.y) >= bottom(field) - 0.5,
        "the popover spans {:.0} to {:.0}, over its field at {:.0} to {:.0}",
        f32::from(popover.origin.y),
        bottom(popover),
        f32::from(field.origin.y),
        bottom(field)
    );
    assert!(f32::from(popover.origin.y) >= -0.5 && bottom(popover) <= height + 0.5);
    let list = bounds_of(window, "language-options-scroll", cx);
    assert!(
        bottom(list) <= bottom(popover) + 0.5 && list.size.height > px(0.),
        "the list scrolls inside the popover"
    );
}
