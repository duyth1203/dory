mod common;

use std::cell::RefCell;
use std::rc::Rc;

use dory_core::domain::{Goal, Job, JobEvent, JobId, JobState, Priority, Stage, Timestamp};
use dory_core::ports::{Change, Store as _};
use dory_gpui::app::AppState;
use dory_gpui::{notify, queue_popover, set_up_summaries};
use gpui_kit::component::WindowExt as _;
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

fn open_queue(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    let (window, app) = open_with(cx, dory_on(queued_store()));
    cx.update_window(window, |_, window, cx| {
        queue_popover::open(&app, window, cx)
    })
    .unwrap();
    settle(window, cx);
    (window, app)
}

fn settle(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
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

fn has(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, _| {
        window.try_find(ElementId::from(id.to_owned())).is_some()
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_queue_lists_jobs_in_the_order_the_worker_takes_them(cx: &mut TestAppContext) {
    let (_, app) = open_queue(cx);
    assert_eq!(order(&app, cx), [1, 2, 3]);
    assert!(
        queue(&app, cx)
            .iter()
            .all(|job| job.state == JobState::Queued)
    );
}

#[gpui_kit::test]
fn the_popover_labels_each_job_and_its_buttons(cx: &mut TestAppContext) {
    let (window, _) = open_queue(cx);
    cx.update_window(window, |_, window, _| {
        let list = window.find("queue");
        assert_eq!(list.role(), Some(Role::List));
        assert_eq!(list.label(), Some("Processing Queue"));
        let second = window.find("queue-job-2");
        assert_eq!(second.role(), Some(Role::ListItem));
        assert_eq!(second.label(), Some("Design review, Queued"));
        let promote = window.find("queue-promote-3");
        assert_eq!(promote.role(), Some(Role::Button));
        assert_eq!(promote.label(), Some("Move 1:1 with Sam to top"));
        assert_eq!(
            window.find("queue-cancel-1").label(),
            Some("Cancel Weekly sync")
        );
        assert!(
            window.try_find("queue-promote-1").is_none(),
            "the next job to run has nothing to move past"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn move_to_top_makes_that_job_run_next(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    click(window, "queue-promote-3", cx);
    assert_eq!(order(&app, cx), [3, 1, 2]);
    assert!(
        !has(window, "queue-promote-3", cx),
        "it is first now, so it offers no Move to top"
    );
    assert!(has(window, "queue-promote-1", cx));
}

#[gpui_kit::test]
fn cancel_removes_the_job_without_asking(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    click(window, "queue-cancel-2", cx);
    assert_eq!(order(&app, cx), [1, 3]);
    assert!(!has(window, "queue-job-2", cx));
    assert!(has(window, "queue-job-1", cx));
}

#[gpui_kit::test]
fn the_buttons_work_from_the_keyboard(cx: &mut TestAppContext) {
    let (window, app) = open_queue(cx);
    for _ in 0..12 {
        let focused = cx
            .update_window(window, |_, window, _| {
                window.find("queue-promote-3").focused() == Some(true)
            })
            .unwrap();
        if focused {
            break;
        }
        cx.update_window(window, |_, window, cx| window.press("tab", cx))
            .unwrap();
        settle(window, cx);
    }
    cx.update_window(window, |_, window, cx| {
        assert_eq!(window.find("queue-promote-3").focused(), Some(true));
        window.press("enter", cx);
    })
    .unwrap();
    settle(window, cx);
    assert_eq!(order(&app, cx), [3, 1, 2]);
}

#[gpui_kit::test]
fn an_empty_queue_says_so(cx: &mut TestAppContext) {
    let (window, app) = open_with(cx, dory_on(sample_store()));
    cx.update_window(window, |_, window, cx| {
        queue_popover::open(&app, window, cx)
    })
    .unwrap();
    settle(window, cx);
    assert!(order(&app, cx).is_empty());
    assert!(has(window, "queue", cx));
    assert!(!has(window, "queue-job-1", cx));
}

#[gpui_kit::test]
fn the_summary_sheet_copies_each_command_and_closes_on_done(cx: &mut TestAppContext) {
    let (window, _) = open_with(cx, dory_on(sample_store()));
    cx.update_window(window, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
    settle(window, cx);
    for command in set_up_summaries::COMMANDS {
        let copy = format!("copy-{command}");
        cx.update_window(window, |_, window, _| {
            assert_eq!(
                window.find(ElementId::from(copy.clone())).label(),
                Some(format!("Copy {command}").as_str())
            );
        })
        .unwrap();
        click(window, &copy, cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(command.to_owned())
        );
    }
    click(window, "set-up-done", cx);
    let open = cx
        .update_window(window, |_, window, cx| window.has_active_dialog(cx))
        .unwrap();
    assert!(!open, "Done closes the sheet");
}

#[gpui_kit::test]
fn notifications_are_a_no_op_without_a_bundle(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory_on(sample_store()));
    cx.update(|cx| {
        notify::install(&app, cx);
        notify::job_finished(&id_of(FIRST), FIRST, cx);
        notify::time_warning(cx);
        notify::cap_reached(cx);
    });
    cx.run_until_parked();
}
