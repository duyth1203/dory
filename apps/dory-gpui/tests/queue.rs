mod common;

use std::cell::RefCell;
use std::rc::Rc;

use dory_core::domain::{Goal, Job, JobEvent, JobId, JobState, Priority, Stage, Timestamp};
use dory_core::ports::{Change, Store as _};
use dory_gpui::app::AppState;
use dory_gpui::library::Selection;
use dory_gpui::{fixtures, notify, processing_window, set_up_summaries, settings_window, surface};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Entity, Role, TestAppContext,
    test::TestWindowExt as _,
};

use common::{dory_on, id_of, open_with, sample_store};

const FIRST: &str = "Weekly sync";
const SECOND: &str = "Design review";
const THIRD: &str = "1:1 with Sam";
const FAR_FUTURE: Timestamp = Timestamp(i64::MAX / 2);

/// Three recordings with a queued job each. The retry backoff keeps the worker from taking
/// them, so the queue holds still while the test reads it.
fn queued_store() -> std::sync::Arc<dory_core::testing::store::MemStore> {
    let store = sample_store();
    let mut changes = Vec::new();
    for (n, title) in [FIRST, SECOND, THIRD].into_iter().enumerate() {
        let job = JobId(n as u64 + 1);
        changes.push(Change::JobEnqueued {
            id: id_of(title),
            goal: Goal::ALL,
            priority: Priority::Normal,
            picks: Default::default(),
            at: Timestamp(1_000 + n as i64),
        });
        changes.push(Change::Job {
            job,
            event: JobEvent::Started {
                stage: Stage::Transcribe,
            },
        });
        changes.push(Change::Job {
            job,
            event: JobEvent::Retry {
                not_before: FAR_FUTURE,
            },
        });
    }
    store.apply(changes).expect("seed the queued jobs");
    store
}

/// Opens the Processing window over the app and returns it.
fn open_window(app: &Entity<AppState>, cx: &mut TestAppContext) -> AnyWindowHandle {
    cx.update(processing_window::install);
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| processing_window::open(app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Processing window opened");
    settle(window, cx);
    window
}

fn open_queue(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    let (_, app) = open_with(cx, dory_on(queued_store()));
    let window = open_window(&app, cx);
    (window, app)
}

fn settle(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.render_frame(cx));
}

fn queue(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<Job> {
    let task = app.update(cx, |app, cx| app.queue(cx));
    let jobs = Rc::new(RefCell::new(None));
    let out = jobs.clone();
    cx.update(|cx| {
        cx.spawn(async move |_| *out.borrow_mut() = Some(task.await))
            .detach()
    });
    cx.run_until_parked();
    let read = jobs.borrow_mut().take().expect("the queue read finished");
    read.expect("read the queue")
}

fn order(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<u64> {
    queue(app, cx).iter().map(|job| job.id.0).collect()
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.click(id.to_owned(), cx))
        .unwrap();
    settle(window, cx);
}

fn press(window: AnyWindowHandle, keys: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(keys, cx))
        .unwrap();
    settle(window, cx);
}

fn focus_row(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    for _ in 0..60 {
        let focused = cx
            .update_window(window, |_, window, _| {
                window.find(id.to_owned()).focused() == Some(true)
            })
            .unwrap();
        if focused {
            return;
        }
        press(window, "tab", cx);
    }
    panic!("{id} never took focus");
}

fn has(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, _| {
        window.try_find(ElementId::from(id.to_owned())).is_some()
    })
    .unwrap()
}

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    cx.update_window(window, |_, window, _| {
        window.find(id.to_owned()).label().map(str::to_owned)
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_queue_lists_jobs_in_the_order_the_worker_takes_them(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    assert_eq!(order(&app, cx), [1, 2, 3]);
    assert_eq!(
        label(window, "queue-job-2", cx).as_deref(),
        Some("Design review, queued, 2 of 3, 48 minutes.")
    );
    assert_eq!(
        label(window, "queue-subtitle", cx).as_deref(),
        Some("3 recordings")
    );
    assert!(
        !has(window, "queue-promote-1", cx),
        "the next job to run has nothing to move past"
    );
    assert_eq!(
        label(window, "queue-promote-3", cx).as_deref(),
        Some("Move 1:1 with Sam to top")
    );
}

#[gpui_kit::test]
fn every_section_shows_with_one_sentence_per_row(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory_on(sample_store()));
    let busy = fixtures::queue_state("busy").unwrap();
    app.update(cx, |app, cx| app.preview_queue(Some(busy), cx));
    let window = open_window(&app, cx);
    for section in [
        "queue-now",
        "queue-next",
        "queue-attention",
        "queue-finished",
    ] {
        assert!(has(window, section, cx), "{section}");
    }
    assert_eq!(
        label(window, "queue-subtitle", cx).as_deref(),
        Some("5 recordings · about 118 min left · 1 needs attention")
    );
    assert_eq!(
        label(window, "queue-job-1", cx).as_deref(),
        Some(
            "Weekly sync, transcribing, 40%, about 9 minutes left, running light because the Mac is on battery."
        )
    );
    assert!(has(window, "queue-pace-chip", cx), "the light chip");
    assert_eq!(
        label(window, "queue-pace", cx).as_deref(),
        Some("Use Fast This Time")
    );
    let hiring = id_of("Hiring debrief").to_string();
    assert_eq!(
        label(window, &format!("queue-attention-{hiring}"), cx).as_deref(),
        Some(
            "Hiring debrief, needs attention. The transcript is ready, but summaries aren't set up."
        )
    );
    assert_eq!(
        label(window, &format!("queue-fix-{hiring}"), cx).as_deref(),
        Some("Set Up Summaries")
    );
    assert_eq!(
        label(window, "queue-finished-7", cx).as_deref(),
        Some("Kundengespräch Müller GmbH, done at 14:32.")
    );
    assert_eq!(
        label(window, "queue-finished-act-8", cx).as_deref(),
        Some("Process Again Partner call, Lyon office")
    );
    let finished = fixtures::queue_state("finished").unwrap();
    app.update(cx, |app, cx| app.preview_queue(Some(finished), cx));
    settle(window, cx);
    click(window, "queue-clear", cx);
    assert!(!has(window, "queue-finished", cx), "Clear empties the list");
}

#[gpui_kit::test]
fn an_empty_queue_says_nothing_is_waiting_and_focuses_close(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory_on(sample_store()));
    let window = open_window(&app, cx);
    assert!(order(&app, cx).is_empty());
    assert_eq!(
        label(window, "queue-empty-title", cx).as_deref(),
        Some(processing_window::EMPTY_TITLE)
    );
    assert!(has(window, "queue-settings", cx));
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("queue-close").focused(), Some(true));
        assert_eq!(window.find("queue-title").label(), Some("Processing Queue"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn move_to_top_works_twice_in_a_row(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    click(window, "queue-promote-3", cx);
    assert_eq!(order(&app, cx), [3, 1, 2]);
    click(window, "queue-promote-2", cx);
    assert_eq!(order(&app, cx), [2, 3, 1]);
    assert!(!has(window, "queue-promote-2", cx), "it is first now");
}

#[gpui_kit::test]
fn the_keyboard_moves_the_focused_row(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    focus_row(window, "queue-job-3", cx);
    press(window, "alt-up", cx);
    assert_eq!(order(&app, cx), [1, 3, 2], "Option-Up");
    focus_row(window, "queue-job-1", cx);
    press(window, "alt-down", cx);
    assert_eq!(order(&app, cx), [3, 1, 2], "Option-Down");
    focus_row(window, "queue-job-2", cx);
    press(window, "alt-cmd-up", cx);
    assert_eq!(order(&app, cx), [2, 3, 1], "Option-Command-Up");
}

#[gpui_kit::test]
fn cancel_acts_at_once_and_undo_restores_the_old_position(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    focus_row(window, "queue-job-2", cx);
    press(window, "backspace", cx);
    assert_eq!(order(&app, cx), [1, 3], "Delete cancels without asking");
    assert_eq!(
        label(window, "queue-toast", cx).as_deref(),
        Some("Cancelled Design review. Undo, 5 seconds left")
    );
    assert!(
        has(window, "queue-finished", cx),
        "the cancelled job is listed as finished this session"
    );
    click(window, "queue-undo", cx);
    // Undo retries the job with no backoff, so the worker takes it at once ahead of the held
    // jobs; the facade's tests hold the worker and check the exact spot it returns to.
    assert!(back(&app, 2, cx), "Undo takes the cancel back");
    assert!(!has(window, "queue-toast", cx));
    assert!(
        !has(window, "queue-finished-2", cx),
        "an undone cancel leaves Finished"
    );

    focus_row(window, "queue-job-3", cx);
    press(window, "delete", cx);
    assert!(!order(&app, cx).contains(&3), "forward Delete cancels too");
    press(window, "cmd-z", cx);
    assert!(back(&app, 3, cx), "Command-Z also undoes");
}

fn back(app: &Entity<AppState>, job: u64, cx: &mut TestAppContext) -> bool {
    queue(app, cx)
        .iter()
        .any(|j| j.id.0 == job && j.state != JobState::Cancelled)
}

#[gpui_kit::test]
fn choosing_a_row_selects_the_recording_and_keeps_the_window(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    click(window, "queue-job-2", cx);
    let selection = app.read_with(cx, |app, _| app.selection.clone());
    assert_eq!(selection, Selection::One(id_of(SECOND)));
    assert!(
        cx.update(|cx| cx.windows()).contains(&window),
        "the Processing window stays open"
    );
}

#[gpui_kit::test]
fn command_w_and_close_shut_the_window_and_open_brings_it_back(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    cx.update(|cx| processing_window::open(&app, cx));
    let count = cx
        .update(|cx| cx.windows())
        .into_iter()
        .filter(|w| *w == window)
        .count();
    assert_eq!(count, 1, "opening again brings the same window forward");
    press(window, "cmd-w", cx);
    assert!(!cx.update(|cx| cx.windows()).contains(&window), "Command-W");
    let again = open_window(&app, cx);
    click(again, "queue-close", cx);
    assert!(!cx.update(|cx| cx.windows()).contains(&again), "Close");
}

#[gpui_kit::test]
fn set_up_summaries_is_named_by_its_title_and_starts_on_get_ollama(cx: &mut TestAppContext) {
    let (window, _) = open_with(cx, dory_on(sample_store()));
    cx.update_window(window, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
    settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        assert!(surface::has_dialog(window, cx));
        let dialog = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|e| e.role() == Some(Role::Dialog))
            .expect("a dialog node");
        assert_eq!(dialog.label(), Some(set_up_summaries::TITLE));
        assert_eq!(window.find("set-up-get-ollama").focused(), Some(true));
        assert_eq!(
            window.find("set-up-subtitle").label(),
            Some("2 short steps. Everything stays on this Mac.")
        );
        assert_eq!(window.find("set-up-close").label(), Some("Close"));
    })
    .unwrap();
    click(window, "set-up-not-now", cx);
    let open = cx
        .update_window(window, |_, window, cx| surface::has_dialog(window, cx))
        .unwrap();
    assert!(!open, "Not Now closes the dialog");
}

#[gpui_kit::test]
fn each_summaries_step_focuses_its_action(cx: &mut TestAppContext) {
    let (window, _) = open_with(cx, dory_on(sample_store()));
    for (step, first) in [
        ("U1", "set-up-get-ollama"),
        ("U2", "set-up-background"),
        ("U3", "set-up-done"),
    ] {
        cx.update_window(window, |_, window, cx| {
            set_up_summaries::preview(fixtures::summaries_step(step).unwrap(), window, cx)
        })
        .unwrap();
        settle(window, cx);
        cx.update_window(window, |_, window, _| {
            assert_eq!(window.find(first).focused(), Some(true), "{step}");
        })
        .unwrap();
        press(window, "escape", cx);
        let open = cx
            .update_window(window, |_, window, cx| surface::has_dialog(window, cx))
            .unwrap();
        assert!(!open, "{step}: Escape closes it");
    }
}

#[gpui_kit::test]
fn notifications_are_a_no_op_without_a_bundle(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory_on(sample_store()));
    cx.update(|cx| {
        notify::install(&app, cx);
        let settings = dory_gpui::prefs::Notifications::default();
        let summary = notify::Note::summary_ready(&id_of(FIRST), FIRST);
        notify::send(notify::NoteKind::Summary, summary, &settings, cx);
        notify::send(
            notify::NoteKind::TimeLimit,
            notify::Note::time_warning(),
            &settings,
            cx,
        );
        notify::send(
            notify::NoteKind::TimeLimit,
            notify::Note::cap_reached(),
            &settings,
            cx,
        );
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_first_in_line_cannot_move_up(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    let first = queue(&app, cx)[0].clone();
    assert_eq!(first.state, JobState::Queued);
    focus_row(window, "queue-job-1", cx);
    press(window, "alt-up", cx);
    assert_eq!(order(&app, cx), [1, 2, 3], "the first in line stays first");
}

fn floats(cx: &mut TestAppContext) -> bool {
    cx.update(|cx| processing_window::current(cx).unwrap().read(cx).floats())
}

fn front(cx: &mut TestAppContext) -> Option<AnyWindowHandle> {
    cx.update(|cx| cx.active_window())
}

/// Settings is a normal window, so a queue left floating covers it.
#[gpui_kit::test]
fn processing_settings_comes_to_the_front_over_the_queue(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory_on(sample_store()));
    let queue = open_window(&app, cx);
    assert!(floats(cx), "the queue floats over the main window");

    click(queue, "queue-settings", cx);
    let settings = cx
        .update(|cx| settings_window::current_window(cx))
        .expect("Settings opened");
    let pane = cx.update(|cx| settings_window::current(cx).unwrap().read(cx).pane());
    assert_eq!(pane, settings_window::Pane::Transcription);
    assert!(!floats(cx), "the queue no longer floats over Settings");

    cx.update_window(queue, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(front(cx), Some(queue));
    click(queue, "queue-settings", cx);
    assert_eq!(
        front(cx),
        Some(settings),
        "an open Settings comes forward too"
    );
    assert!(!floats(cx));

    cx.update_window(settings, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(floats(cx), "the queue floats again once Settings closes");
}

#[gpui_kit::test]
fn a_queue_opened_over_settings_does_not_float(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory_on(sample_store()));
    cx.update(|cx| settings_window::open(&app, cx));
    cx.run_until_parked();
    open_window(&app, cx);
    assert!(!floats(cx), "Settings stays reachable above the queue");
}
