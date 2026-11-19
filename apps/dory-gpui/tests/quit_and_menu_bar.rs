//! Quit, the window's lifetime, the menu-bar item's commands, and the notification settings
//! (UX-8, UX-14), through the app as `main` starts it: closing the main window keeps the state
//! and the recorder, Quit asks only when something is in progress, and the menu-bar item records
//! with no window open.

mod common;

use std::sync::Arc;

use dory_core::app::RecorderStatus;
use dory_core::ports::StoreRead as _;
use dory_core::testing::store::MemStore;
use dory_gpui::actions::Quit;
use dory_gpui::app::AppState;
use dory_gpui::library::Selection;
use dory_gpui::notify::{self, NoteKind};
use dory_gpui::status_item::{self, Command};
use dory_gpui::{quit, settings_window};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Entity, TestAppContext, test::TestWindowExt as _,
};

use crate::common::{dory_on, id_of, launch, past_first_run, recordable, sample_store, until};

fn empty() -> Arc<MemStore> {
    Arc::new(MemStore::new())
}

fn main_window(cx: &mut TestAppContext) -> Option<AnyWindowHandle> {
    cx.update(|cx| dory_gpui::main_window(cx))
}

fn exits(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| quit::exits(cx))
}

fn dialog_open(window: AnyWindowHandle, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, cx| {
        dory_gpui::surface::has_dialog(window, cx)
    })
    .unwrap()
}

fn element(
    window: AnyWindowHandle,
    id: &str,
    cx: &mut TestAppContext,
) -> Option<gpui_kit::base::test_support::ElementSnapshot> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(ElementId::Name(id.to_owned().into()))
    })
    .unwrap()
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(ElementId::Name(id.to_owned().into()), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn quit_from(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.dispatch_action(window, Quit);
    cx.run_until_parked();
}

fn record(app: &Entity<AppState>, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| app.start_recording(cx));
    until(cx, "a take is recording", |cx| {
        app.read_with(cx, |app, _| {
            matches!(app.recorder(), RecorderStatus::Recording { .. })
        })
    });
}

fn close(window: AnyWindowHandle, cx: &mut TestAppContext) {
    press(window, "cmd-w", cx);
    assert_eq!(main_window(cx), None, "⌘W closes the main window");
}

#[gpui_kit::test]
fn quit_with_nothing_in_progress_quits_at_once(cx: &mut TestAppContext) {
    let (window, app) = launch(cx, dory_on(empty()), past_first_run());
    quit_from(window, cx);
    assert!(!dialog_open(window, cx), "nothing to ask about");
    assert_eq!(exits(cx), 1);
    assert!(app.read_with(cx, |app, _| app.is_shut_down()));
}

#[gpui_kit::test]
fn quit_works_from_settings_and_with_no_window(cx: &mut TestAppContext) {
    let (window, app) = launch(cx, dory_on(empty()), past_first_run());
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| settings_window::open(&app, cx));
    let settings = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Settings window opened");
    quit_from(settings, cx);
    assert_eq!(exits(cx), 1, "Quit reaches the app from Settings");

    cx.update_window(settings, |_, window, _| window.remove_window())
        .unwrap();
    close(window, cx);
    cx.update(|cx| cx.dispatch_action(&Quit));
    cx.run_until_parked();
    assert_eq!(exits(cx), 2, "Quit reaches the app with no window open");
}

#[gpui_kit::test]
fn quit_while_recording_asks_with_cancel_focused(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, app) = launch(cx, recordable(empty()), past_first_run());
    record(&app, cx);

    quit_from(window, cx);
    assert!(dialog_open(window, cx), "a live take asks first");
    assert_eq!(
        element(window, "quit-cancel", cx).and_then(|e| e.focused()),
        Some(true),
        "Cancel has focus, so Return never quits by accident"
    );
    let confirm = element(window, "quit-confirm", cx).expect("the quit button");
    assert_eq!(confirm.label(), Some("Stop and Quit"));
    let line = element(window, "quit-line-0", cx).expect("the take's sentence");
    assert!(
        line.label().is_some_and(
            |l| l.starts_with("Dory is recording “") && l.ends_with("saves what was recorded.")
        ),
        "{:?}",
        line.label()
    );

    press(window, "enter", cx);
    assert!(!dialog_open(window, cx), "Return pressed Cancel");
    quit_from(window, cx);
    press(window, "escape", cx);
    assert!(!dialog_open(window, cx), "Escape cancels");
    assert_eq!(exits(cx), 0);
    assert!(app.read_with(cx, |app, _| matches!(
        app.recorder(),
        RecorderStatus::Recording { .. }
    )));
    assert!(!app.read_with(cx, |app, _| app.is_shut_down()));
}

#[gpui_kit::test]
fn confirming_shuts_the_core_down_saves_the_take_then_quits(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let store = empty();
    let (window, app) = launch(cx, recordable(store.clone()), past_first_run());
    record(&app, cx);
    let id = app.read_with(cx, |app, _| app.recorder().id().cloned().unwrap());
    quit_from(window, cx);
    click(window, "quit-confirm", cx);
    until(cx, "the quit", |cx| exits(cx) == 1);
    assert!(app.read_with(cx, |app, _| app.is_shut_down()));
    let take = store.recording(&id).unwrap().unwrap().take;
    assert!(
        take.is_some(),
        "the shutdown sealed the take before quitting"
    );
}

#[gpui_kit::test]
fn closing_the_window_keeps_the_state_and_the_recorder(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, app) = launch(cx, recordable(sample_store()), past_first_run());
    record(&app, cx);
    let weak = app.downgrade();
    drop(app);
    close(window, cx);
    assert!(cx.update(|cx| cx.windows()).is_empty());
    let app = weak.upgrade().expect("the state outlives the window");
    cx.run_until_parked();
    assert!(!app.read_with(cx, |app, _| app.is_shut_down()));
    assert!(
        matches!(
            app.read_with(cx, |app, _| app.recorder().clone()),
            RecorderStatus::Recording { .. }
        ),
        "the take is still recording with the window closed"
    );

    cx.update(|cx| status_item::run(Command::Show, &app, cx));
    let reopened = main_window(cx).expect("Show Dory reopens the window");
    assert_ne!(reopened, window);
    assert!(
        element(reopened, "recordings", cx).is_some(),
        "the reopened window shows the library"
    );
    assert!(matches!(
        app.read_with(cx, |app, _| app.recorder().clone()),
        RecorderStatus::Recording { .. }
    ));
    cx.update(|cx| status_item::run(Command::Show, &app, cx));
    assert_eq!(
        main_window(cx),
        Some(reopened),
        "an open window comes forward"
    );
    assert_eq!(cx.update(|cx| cx.windows()).len(), 1);
}

#[gpui_kit::test]
fn a_reopened_window_shows_the_selected_recording(cx: &mut TestAppContext) {
    let (window, app) = launch(cx, dory_on(sample_store()), past_first_run());
    let row = format!("row-{}", id_of("Design review"));
    click(window, &row, cx);
    assert!(element(window, "start-recording", cx).is_none());

    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| settings_window::open(&app, cx));
    let settings = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Settings window opened");
    cx.update_window(settings, |_, window, _| window.remove_window())
        .unwrap();
    close(window, cx);
    cx.update(|cx| status_item::run(Command::Show, &app, cx));
    let reopened = main_window(cx).expect("Show Dory reopens the window");

    assert!(
        element(reopened, "start-recording", cx).is_none(),
        "the reopened window shows the selected recording, not the start screen"
    );
    assert_eq!(
        element(reopened, "recordings", cx).and_then(|list| list.focused()),
        Some(true),
        "the keyboard lands in the list, since the start screen is not shown"
    );
    click(reopened, &row, cx);
    assert!(
        element(reopened, "start-recording", cx).is_none(),
        "clicking the selected row shows it"
    );
}

#[gpui_kit::test]
fn the_menu_bar_item_records_with_no_window_open(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, app) = launch(cx, recordable(empty()), past_first_run());
    close(window, cx);
    cx.update(|cx| status_item::run(Command::Toggle, &app, cx));
    until(cx, "the toggle starts a take", |cx| {
        app.read_with(cx, |app, _| {
            matches!(app.recorder(), RecorderStatus::Recording { .. })
        })
    });
    cx.update(|cx| status_item::run(Command::Pause, &app, cx));
    until(cx, "Pause pauses it", |cx| {
        app.read_with(cx, |app, _| {
            matches!(app.recorder(), RecorderStatus::Paused { .. })
        })
    });
    cx.update(|cx| status_item::run(Command::Toggle, &app, cx));
    until(cx, "the toggle stops it", |cx| {
        app.read_with(cx, |app, _| {
            matches!(app.recorder(), RecorderStatus::Grace { .. })
        })
    });
    assert!(
        cx.update(|cx| cx.windows()).is_empty(),
        "recording from the menu bar never opens the window"
    );
    let sent = cx.update(|cx| {
        cx.default_global::<notify::Sent>()
            .0
            .iter()
            .map(|(kind, note)| (*kind, note.title.clone()))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        sent,
        vec![(NoteKind::Recording, "Dory is recording".to_owned())],
        "the start posts, since the window is closed"
    );
}

fn sent(cx: &mut TestAppContext) -> usize {
    cx.update(|cx| cx.default_global::<notify::Sent>().0.len())
}

#[gpui_kit::test]
fn a_start_with_the_window_open_does_not_post(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (_, app) = launch(cx, recordable(empty()), past_first_run());
    record(&app, cx);
    assert_eq!(sent(cx), 0, "the window shows the take itself");
}

#[gpui_kit::test]
fn a_notification_kind_turned_off_does_not_post(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, app) = launch(cx, recordable(empty()), past_first_run());
    close(window, cx);
    app.update(cx, |app, _| {
        app.prefs.notifications.set(NoteKind::Recording, false)
    });
    cx.update(|cx| status_item::run(Command::Toggle, &app, cx));
    until(cx, "the toggle starts a take", |cx| {
        app.read_with(cx, |app, _| {
            matches!(app.recorder(), RecorderStatus::Recording { .. })
        })
    });
    assert_eq!(sent(cx), 0, "Recording started and saved is off");
}

#[gpui_kit::test]
fn the_notification_settings_save_each_kind(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (_, app) = launch(cx, dory_on(sample_store()), past_first_run());
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| settings_window::open(&app, cx));
    let settings = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Settings window opened");
    for kind in NoteKind::ALL {
        let id = format!("setting-note-{}", kind.slug());
        let row = element(settings, &id, cx).unwrap_or_else(|| panic!("no {id}"));
        assert_eq!(row.label(), Some(kind.label()));
        assert_eq!(row.checked(), Some(true), "{kind:?} is on by default");
    }
    click(settings, "setting-note-transcript", cx);
    let notes = app.read_with(cx, |app, _| app.prefs.notifications.clone());
    for kind in NoteKind::ALL {
        assert_eq!(notes.allows(kind), kind != NoteKind::Transcript, "{kind:?}");
    }
    assert_eq!(
        element(settings, "setting-note-transcript", cx).and_then(|e| e.checked()),
        Some(false)
    );
    assert!(element(settings, "setting-notifications-off", cx).is_none());
    cx.update(|cx| notify::set_access(notify::Access::Off, cx));
    assert!(
        element(settings, "open-notification-settings", cx).is_some(),
        "with notifications off in macOS, the section offers System Settings"
    );
}

#[gpui_kit::test]
fn the_notification_setting_is_written_to_the_prefs_file(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("dory-gpui-notes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = dory::Config {
        data_dir: dir.clone(),
        ollama_url: String::new(),
        ollama_num_ctx: None,
        machine: None,
        processing: None,
        apple: None,
    };
    let (_, app) = launch(cx, dory_on(empty()), past_first_run());
    cx.update(|cx| dory_gpui::prefs::watch(&app, &config, cx));
    app.update(cx, |app, cx| {
        app.prefs.notifications.set(NoteKind::Download, false);
        cx.notify();
    });
    cx.executor()
        .advance_clock(dory_gpui::prefs::SAVE_DEBOUNCE * 2);
    cx.run_until_parked();
    let saved = dory_gpui::prefs::Prefs::read(&dir).notifications;
    assert!(!saved.allows(NoteKind::Download));
    assert!(saved.allows(NoteKind::Summary));
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
fn a_notification_click_reopens_the_window_on_its_recording(cx: &mut TestAppContext) {
    let (window, app) = launch(cx, dory_on(sample_store()), past_first_run());
    close(window, cx);
    let id = id_of("Design review");
    cx.update(|cx| notify::open(&app, &format!("recording:{id}"), cx));
    cx.run_until_parked();
    assert!(main_window(cx).is_some(), "the click reopens the window");
    assert_eq!(
        app.read_with(cx, |app, _| app.selection.clone()),
        Selection::One(id)
    );
}
