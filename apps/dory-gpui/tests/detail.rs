mod common;

use std::sync::Arc;

use dory_core::domain::{
    FailureClass, Goal, JobError, JobEvent, JobId, Priority, Recording, RecordingId, Revision,
    Stage, Summary, Timestamp, Transcript,
};
use dory_core::ports::{Change, Store as _};
use dory_core::testing::store::MemStore;
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::library::Selection;
use dory_gpui::summary_view::STALE;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, ScrollDelta, TestAppContext, point, px,
    test::TestWindowExt as _,
};

use common::{dory_on, id_of, open_with, sample_store};

const SILENT: &str = "Silent standup";
const LOST: &str = "Lost take";
const STALE_NOTES: &str = "Planning notes";
const BROKEN: &str = "Broken upload";

fn design_review() -> &'static fixtures::Sample {
    SAMPLES.iter().find(|s| s.title == "Design review").unwrap()
}

/// A recording like Design review, `days` earlier and under its own id.
fn extra(title: &str, days: i64) -> Recording {
    let mut recording = design_review().recording();
    recording.created_at = Timestamp(recording.created_at.0 - days * 86_400_000);
    recording.id = RecordingId::generate(recording.created_at, 0);
    recording.title = Some(title.to_owned());
    recording
}

fn transcript(revision: u32, text: &str) -> Transcript {
    let mut transcript = design_review().transcript().unwrap();
    transcript.revision = Revision(revision);
    transcript.segments[0].text = text.to_owned();
    transcript
}

fn by_title(title: &str) -> RecordingId {
    if SAMPLES.iter().any(|s| s.title == title) {
        id_of(title)
    } else {
        let days = [SILENT, LOST, STALE_NOTES, BROKEN]
            .iter()
            .position(|t| *t == title)
            .unwrap() as i64
            + 20;
        extra(title, days).id
    }
}

/// The samples plus one recording per detail state, with Weekly sync transcribing.
fn states_store() -> Arc<MemStore> {
    let store = sample_store();
    let silent = extra(SILENT, 20);
    let mut lost = extra(LOST, 21);
    lost.take = None;
    let stale = extra(STALE_NOTES, 22);
    let broken = extra(BROKEN, 23);
    let long_summary: String = std::iter::once("## Decisions\n\n".to_owned())
        .chain((1..=40).map(|n| format!("- Decision number {n} for the plan.\n")))
        .collect();
    let at = Timestamp(fixtures::now().timestamp_millis());
    store
        .apply(vec![
            Change::RecordingCreated(silent.clone()),
            Change::TranscriptReplaced {
                id: silent.id.clone(),
                transcript: Transcript {
                    segments: Vec::new(),
                    ..transcript(1, "")
                },
            },
            Change::RecordingCreated(lost),
            Change::RecordingCreated(stale.clone()),
            Change::TranscriptReplaced {
                id: stale.id.clone(),
                transcript: transcript(1, "We plan the quarter."),
            },
            Change::SummaryReplaced {
                id: stale.id.clone(),
                summary: Summary {
                    markdown: long_summary,
                    ..design_review().summary().unwrap()
                },
            },
            Change::TranscriptReplaced {
                id: stale.id.clone(),
                transcript: transcript(2, "We plan the quarter again."),
            },
            Change::RecordingCreated(broken.clone()),
            Change::JobEnqueued {
                id: broken.id.clone(),
                goal: Goal::ALL,
                priority: Priority::Normal,
                at,
            },
            Change::Job {
                job: JobId(1),
                event: JobEvent::Started {
                    stage: Stage::Transcribe,
                },
            },
            Change::Job {
                job: JobId(1),
                event: JobEvent::Failed {
                    stage: Stage::Transcribe,
                    error: JobError {
                        class: FailureClass::Permanent,
                        message: "The upload broke.".into(),
                        cause: dory_core::domain::FailureCause::Other,
                    },
                },
            },
        ])
        .expect("seed the detail states");
    store
}

/// Opens the window on [`states_store`]. Weekly sync starts transcribing after `Dory::open`, so
/// startup recovery never sees a running job to interrupt, and in one change so the idle
/// pipeline never sees it queued.
fn open_states(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    let store = states_store();
    let dory = dory_on(Arc::clone(&store));
    store
        .apply(vec![
            Change::JobEnqueued {
                id: id_of("Weekly sync"),
                goal: Goal::ALL,
                priority: Priority::Normal,
                at: Timestamp(fixtures::now().timestamp_millis()),
            },
            Change::Job {
                job: JobId(2),
                event: JobEvent::Started {
                    stage: Stage::Transcribe,
                },
            },
        ])
        .expect("start transcribing Weekly sync");
    let opened = open_with(cx, dory);
    cx.update(dory_gpui::menus::install);
    opened
}

fn select(app: &Entity<AppState>, title: &str, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| {
        app.select(Selection::One(by_title(title)), cx)
    });
    cx.run_until_parked();
}

fn label_of(window: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) -> String {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).label().unwrap_or_default().to_owned()
    })
    .unwrap()
}

#[gpui_kit::test]
fn each_state_shows_its_text(cx: &mut TestAppContext) {
    let (window, app) = open_states(cx);

    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of("Design review")), cx)
    });
    assert_eq!(label_of(window, "detail-loading", cx), "Loading…");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("title").label(), Some("Design review"));
        let meta = window
            .find("detail-meta")
            .label()
            .unwrap_or_default()
            .to_owned();
        assert!(meta.ends_with(" · 48 min · Mic and system audio"), "{meta}");
        assert_eq!(
            window.find("summary-document").role(),
            Some(gpui_kit::Role::Document)
        );
        assert!(window.try_find("summary-stale").is_none());
        assert!(window.try_find("detail-state").is_none());
    })
    .unwrap();

    let cases = [
        ("Weekly sync", "Transcribing"),
        (
            SILENT,
            "Nothing was said. The transcript is empty, so there is nothing to summarize.",
        ),
        (
            LOST,
            "No audio was recorded. Nothing reached the microphone or system audio.",
        ),
        (BROKEN, "Transcription failed: The upload broke."),
    ];
    for (title, want) in cases {
        select(&app, title, cx);
        assert_eq!(label_of(window, "detail-state", cx), want, "{title}");
    }

    select(&app, STALE_NOTES, cx);
    assert_eq!(label_of(window, "summary-stale", cx), STALE);

    select(&app, "Hiring debrief", cx);
    assert_eq!(label_of(window, "summary-missing", cx), "No summary yet.");
}

#[gpui_kit::test]
fn command_1_and_command_2_switch_tabs(cx: &mut TestAppContext) {
    let (window, app) = open_states(cx);
    select(&app, "Design review", cx);
    let selected = |window: &mut gpui_kit::Window| {
        (
            window.find("summary-tab").selected(),
            window.find("transcript-tab").selected(),
        )
    };
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("summary-tab", cx);
    })
    .unwrap();
    let press = |key: &str, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            selected(window)
        })
        .unwrap()
    };
    assert_eq!(press("cmd-2", cx), (Some(false), Some(true)));
    assert_eq!(press("cmd-1", cx), (Some(true), Some(false)));
    assert_eq!(app.read_with(cx, |app, _| app.tab), DetailTab::Summary);
}

fn stored_title(app: &Entity<AppState>, cx: &mut TestAppContext) -> Option<String> {
    app.read_with(cx, |app, _| {
        app.recordings
            .iter()
            .find(|r| r.recording.id == id_of("Design review"))
            .and_then(|r| r.recording.title.clone())
    })
}

#[gpui_kit::test]
fn return_saves_the_title_and_escape_cancels(cx: &mut TestAppContext) {
    let (window, app) = open_states(cx);
    select(&app, "Design review", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("title", cx);
        window.render_frame(cx);
        assert!(window.try_find("title-editor").is_some(), "click edits");
        window.input("Export kickoff", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(stored_title(&app, cx).as_deref(), Some("Export kickoff"));
    assert_eq!(label_of(window, "title", cx), "Export kickoff");

    cx.update_window(window, |_, window, cx| {
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find("title-editor").is_some(), "Return edits");
        window.input("Never saved", cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("title-editor").is_none());
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(stored_title(&app, cx).as_deref(), Some("Export kickoff"));
}

#[gpui_kit::test]
fn scrolling_docks_the_player_and_pins_the_tab_row(cx: &mut TestAppContext) {
    let (window, app) = open_states(cx);
    select(&app, STALE_NOTES, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.scroll("detail", ScrollDelta::Pixels(point(px(0.), px(-3000.))), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let page = window.find("detail").bounds();
        let title = window.find("title").bounds();
        assert!(title.bottom() < page.top(), "the title scrolled off");
        let tabs = window.find("summary-tab").bounds();
        assert!(
            tabs.top() >= page.top() && tabs.top() < page.top() + px(8.),
            "the tab row sticks to the top: {tabs:?} in {page:?}"
        );
        let player = window.find("player").bounds();
        assert!(
            player.top() >= page.top() && player.bottom() <= page.bottom() - px(14.),
            "the player docks inside the page: {player:?} in {page:?}"
        );
    })
    .unwrap();
}
