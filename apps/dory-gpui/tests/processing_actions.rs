//! While a job for a recording is queued or running, Re-transcribe in… and Translate Summary…
//! wait for it on every surface that offers them, say why, and come back when it ends. The rule
//! is the core's (`Underway`), read by `AppState::is_underway`.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::domain::Goal;
use dory_core::testing::FakeLanguageModel;
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::library::Selection;
use dory_gpui::menus::{self, WAITS};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Entity, MenuItem, Role, TestAppContext,
    test::TestWindowExt as _,
};

use common::{id_of, past_first_run, sample_store, until};

const TITLE: &str = "Design review";

/// The Recording menu's submenu `name` as `(item, disabled)`, separators left out.
fn submenu(app: &Entity<AppState>, name: &str, cx: &mut TestAppContext) -> Vec<(String, bool)> {
    let menus = cx.update(|cx| menus::build(menus::menu_state(app.read(cx), cx)));
    let recording = menus.iter().find(|m| m.name == "Recording").unwrap();
    recording
        .items
        .iter()
        .find_map(|item| match item {
            MenuItem::Submenu(sub) if sub.name == name => Some(&sub.items),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {name} in the Recording menu"))
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { name, disabled, .. } => Some((name.to_string(), *disabled)),
            _ => None,
        })
        .collect()
}

fn file_item_disabled(app: &Entity<AppState>, name: &str, cx: &mut TestAppContext) -> bool {
    let menus = cx.update(|cx| menus::build(menus::menu_state(app.read(cx), cx)));
    let file = menus.iter().find(|m| m.name == "File").unwrap();
    file.items
        .iter()
        .find_map(|item| match item {
            MenuItem::Action {
                name: n, disabled, ..
            } if n == name => Some(*disabled),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {name} in the File menu"))
}

/// The labels of the menu items drawn now, with their element ids.
fn menu_items(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<(String, ElementId)> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|el| el.role() == Some(Role::MenuItem))
            .filter_map(|el| Some((el.label()?.to_owned(), el.path().last()?.clone())))
            .collect()
    })
    .unwrap()
}

/// Draws until `done` holds or `wait` passes. Kit's menus animate on the machine's clock.
fn settles(
    window: AnyWindowHandle,
    wait: Duration,
    cx: &mut TestAppContext,
    done: impl Fn(&[(String, ElementId)]) -> bool,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        cx.run_until_parked();
        if done(&menu_items(window, cx)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Opens a recording menu with `open`, points at Re-transcribe in…, and reports whether the
/// reason line shows and whether the languages opened. Closes the menu after.
fn popup(
    window: AnyWindowHandle,
    open: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App),
    cx: &mut TestAppContext,
) -> (bool, bool) {
    const SECOND: Duration = Duration::from_secs(1);
    assert!(
        settles(window, SECOND, cx, |items| items.is_empty()),
        "the last menu is still open"
    );
    let opened = (0..10).any(|_| {
        cx.update_window(window, |_, window, cx| open(window, cx))
            .unwrap();
        settles(window, SECOND / 4, cx, |items| !items.is_empty())
    });
    assert!(opened, "the menu did not open");
    let items = menu_items(window, cx);
    let reason = items.iter().any(|(label, _)| label == WAITS);
    let (_, entry) = items
        .iter()
        .find(|(label, _)| label == "Re-transcribe in…")
        .unwrap_or_else(|| panic!("Re-transcribe in… is not hidden: {items:?}"));
    let entry = entry.clone();
    cx.update_window(window, |_, window, cx| window.click(entry, cx))
        .unwrap();
    let languages = settles(window, SECOND / 2, cx, |items| {
        items.iter().any(|(label, _)| label == "Automatic")
    });
    for _ in 0..2 {
        cx.update_window(window, |_, window, cx| window.press("escape", cx))
            .unwrap();
        cx.run_until_parked();
    }
    assert!(
        settles(window, SECOND, cx, |items| items.is_empty()),
        "Escape closes the menu"
    );
    (reason, languages)
}

/// Every surface offers Re-transcribe in… and Translate… when `offered`, and greys them out
/// with the reason when not. Rename and Move to Trash never wait.
fn assert_offered(
    window: AnyWindowHandle,
    app: &Entity<AppState>,
    offered: bool,
    cx: &mut TestAppContext,
) {
    let when = if offered {
        "after the job"
    } else {
        "during the job"
    };
    for name in ["Re-transcribe in", "Translate Summary…"] {
        let items = submenu(app, name, cx);
        assert_eq!(
            items.first().map(|(label, _)| label.as_str()) == Some(WAITS),
            !offered,
            "{when}: Recording > {name} heads with the reason only while it waits"
        );
        assert!(
            items
                .iter()
                .filter(|(label, _)| label != WAITS)
                .all(|(_, disabled)| *disabled != offered),
            "{when}: Recording > {name}: {items:?}"
        );
        assert!(
            items.iter().any(|(label, _)| label != WAITS),
            "{when}: {name} keeps its items, dimmed rather than hidden"
        );
    }
    for name in ["Rename", "Move to Trash"] {
        assert!(!file_item_disabled(app, name, cx), "{when}: File > {name}");
    }

    let translate = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let node = window.find("translate");
            (
                node.role(),
                node.disabled() == Some(true),
                node.description().map(str::to_owned),
            )
        })
        .unwrap();
    if offered {
        assert_eq!(translate.0, Some(Role::ComboBox), "{when}: Translate…");
        assert!(!translate.1, "{when}: Translate… is on");
    } else {
        assert!(translate.1, "{when}: VoiceOver hears Translate… as dimmed");
        assert_eq!(translate.2.as_deref(), Some(WAITS), "{when}: and why");
    }

    let row = format!("row-{}", id_of(TITLE));
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
        let (reason, languages) = popup(window, open, cx);
        assert_eq!(reason, !offered, "{when}: the {surface} says why");
        assert_eq!(languages, offered, "{when}: the {surface}'s languages");
    }
}

#[gpui_kit::test]
fn re_transcribe_and_translate_wait_for_the_job_on_every_surface(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let model = Arc::new(FakeLanguageModel::answering(|_, _| {
        "## Fresh summary\n\n- Written again.".to_owned()
    }));
    let (window, app) = common::launch(
        cx,
        common::dory_with_llm(sample_store(), model.clone()),
        past_first_run(),
    );
    let id = id_of(TITLE);
    app.update(cx, |app, cx| {
        app.select(Selection::One(id.clone()), cx);
        app.set_tab(DetailTab::Summary, cx);
    });
    cx.run_until_parked();
    assert_offered(window, &app, true, cx);

    model.hold_from(0);
    app.update(cx, |app, cx| {
        app.finish(id.clone(), Goal::SUMMARIZE, false, cx)
    });
    until(cx, "the summary to start", |_| !model.prompts().is_empty());
    until(cx, "the app to see the job", |cx| {
        app.read_with(cx, |app, _| app.is_underway(&id))
    });
    assert_offered(window, &app, false, cx);

    model.hold_from(usize::MAX);
    until(cx, "the job to end", |cx| {
        app.read_with(cx, |app, _| !app.is_underway(&id))
    });
    assert_offered(window, &app, true, cx);
}
