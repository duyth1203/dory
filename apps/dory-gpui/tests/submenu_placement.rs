//! A model picker's submenu stays inside the window, whichever side of its menu it opens on: the
//! row menu near the sidebar and the detail ⋯ menu near the right edge, at the narrowest window
//! and at widths where a long model name only just fits or only just does not. Kit's own
//! placement is patched for this in `vendor/gpui-component` (see its PATCH.md).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Ports};
use dory_core::domain::Timestamp;
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeMachine, FakeTrash, ManualClock, MemTakes,
    llm_entry,
};
use dory_gpui::app::DetailTab;
use dory_gpui::fixtures;
use dory_gpui::library::Selection;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Pixels, Role, Size, TestAppContext, point,
    px, size, test::TestWindowExt as _,
};

use common::{id_of, past_first_run, sample_store, until};

const TITLE: &str = "Design review";
const PICKER: &str = "Re-summarize with…";
/// As long as the real names get, with a download size, so the submenu is near Kit's widest.
const LONG: &str = "As in Settings (Whisper large-v3 turbo, download 480 MB, and a few words more)";

fn dory() -> Dory {
    let mut entry = llm_entry("fake/llm");
    entry.display_name = LONG.into();
    let dir = std::env::temp_dir().join(format!("dory-gpui-submenu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = Arc::new(FakeLanguageModel::answering(|_, _| {
        "## S\n\n- x.".to_owned()
    }));
    Dory::open(
        Ports {
            store: sample_store(),
            takes: Arc::new(MemTakes::in_dir(&dir)),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            trash: Arc::new(FakeTrash::new(&dir)),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: vec![Arc::new(FakeLlmProvider::new("fake", vec![entry], model))],
            diar: Vec::new(),
            machine: Arc::new(FakeMachine::default()),
            guard: None,
        },
        common::options(),
    )
    .expect("open Dory with a long-named summary model")
}

/// Every menu item drawn now, as its label and its box in window points.
fn drawn_items(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<(String, Bounds<Pixels>)> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|el| el.role() == Some(Role::MenuItem))
            .map(|el| (el.label().unwrap_or_default().to_owned(), el.bounds()))
            .collect()
    })
    .unwrap()
}

fn viewport(window: AnyWindowHandle, cx: &mut TestAppContext) -> Size<Pixels> {
    cx.update_window(window, |_, window, _| window.viewport_size())
        .unwrap()
}

/// Kit keeps a menu this far from every window edge (`EDGE_PADDING` in its popup menu).
const MARGIN: f32 = 4.;

/// The items of `items`, separators included, that come closer than [`MARGIN`] to a window edge.
fn cut_off(
    items: &[(String, Bounds<Pixels>)],
    viewport: Size<Pixels>,
) -> Vec<(String, Bounds<Pixels>)> {
    let inside = Bounds::new(
        point(px(MARGIN), px(MARGIN)),
        size(
            viewport.width - px(2. * MARGIN),
            viewport.height - px(2. * MARGIN),
        ),
    );
    items
        .iter()
        .filter(|(_, b)| {
            b.left() < inside.left()
                || b.right() > inside.right()
                || b.top() < inside.top()
                || b.bottom() > inside.bottom()
        })
        .cloned()
        .collect()
}

/// Draws until `done` holds or `wait` passes. Kit's menus animate on the machine's clock.
fn settles(
    window: AnyWindowHandle,
    wait: Duration,
    cx: &mut TestAppContext,
    done: impl Fn(&[(String, Bounds<Pixels>)]) -> bool,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        cx.run_until_parked();
        if done(&drawn_items(window, cx)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn entry_id(window: AnyWindowHandle, label: &str, cx: &mut TestAppContext) -> ElementId {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|el| el.role() == Some(Role::MenuItem) && el.label() == Some(label))
            .and_then(|el| el.path().last().cloned())
    })
    .unwrap()
    .unwrap_or_else(|| panic!("{label} is not in the menu"))
}

/// Opens a recording menu with `open`, opens the model picker, and returns what it drew. Closes
/// the menu after.
fn open_picker(
    window: AnyWindowHandle,
    open: &dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App),
    cx: &mut TestAppContext,
) -> Vec<(String, Bounds<Pixels>)> {
    const SECOND: Duration = Duration::from_secs(1);
    let opened = (0..10).any(|_| {
        cx.update_window(window, |_, window, cx| open(window, cx))
            .unwrap();
        settles(window, SECOND / 4, cx, |items| !items.is_empty())
    });
    assert!(opened, "the menu did not open");
    let entry = entry_id(window, PICKER, cx);
    cx.update_window(window, |_, window, cx| window.click(entry, cx))
        .unwrap();
    assert!(
        settles(window, SECOND, cx, |items| items
            .iter()
            .any(|(label, _)| label == LONG)),
        "{PICKER} did not open its models"
    );
    // Kit learns the submenu's width from its first frame and places it again on the next.
    settles(window, SECOND / 4, cx, |_| false);
    let drawn = drawn_items(window, cx);
    for _ in 0..2 {
        cx.update_window(window, |_, window, cx| window.press("escape", cx))
            .unwrap();
        cx.run_until_parked();
    }
    assert!(
        settles(window, SECOND, cx, |items| items.is_empty()),
        "Escape closes the menu"
    );
    drawn
}

#[gpui_kit::test]
fn a_model_pickers_submenu_stays_inside_the_window(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    // 800 is the narrowest window; at 892 the submenu no longer fits right of the row menu and
    // flips left; at 898 it stays on the right with under a margin to spare; 1100 is the default.
    for width in [800., 892., 898., 1100.] {
        let mut prefs = past_first_run();
        prefs.window_size = (width, 600.);
        let (window, app) = common::launch(cx, dory(), prefs);
        let id = id_of(TITLE);
        app.update(cx, |app, cx| {
            app.select(Selection::One(id.clone()), cx);
            app.set_tab(DetailTab::Summary, cx);
        });
        until(cx, "the summary models", |cx| {
            app.read_with(cx, |app, cx| {
                app.session()
                    .is_some_and(|s| s.read(cx).rerun_models().summary.is_some())
            })
        });
        let row = format!("row-{id}");
        for (surface, open) in [
            (
                "⋯ menu",
                Box::new(|window: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
                    window.click("more", cx)
                }) as Box<dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App)>,
            ),
            (
                "row menu",
                Box::new(
                    move |window: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
                        window.right_click(row.clone(), cx)
                    },
                ),
            ),
        ] {
            let drawn = open_picker(window, &*open, cx);
            let cut = cut_off(&drawn, viewport(window, cx));
            assert!(
                cut.is_empty(),
                "{width} pt window, the {surface}: cut off {cut:?}"
            );
        }
        cx.update_window(window, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }
}
