//! Layout bugs found by the visual pass: the detail pane's action row at 200% text, the
//! Processing Queue's rows, badges, and bottom edge, and Copy and Export before there is text.

mod common;

use dory_core::domain::{Goal, JobEvent, JobId, Priority, Revision, Stage, Timestamp, Transcript};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store as _};
use dory_core::testing::store::MemStore;
use dory_gpui::actions::{CopySummary, CopyTranscript, ExportMarkdown, ExportSrt, ExportText};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::library::Selection;
use dory_gpui::menus;
use dory_gpui::prefs::{MIN_WINDOW_SIZE, Prefs};
use dory_gpui::processing::Phase;
use dory_gpui::{fixtures, processing_window};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ClipboardItem, ElementId, Entity, Role, ScrollDelta,
    TestAppContext, point, px, test::TestWindowExt as _,
};

use common::{dory_on, id_of, open_with_prefs, sample_store};

fn at_200_percent() -> Prefs {
    Prefs {
        text_scale: 2.0,
        window_size: MIN_WINDOW_SIZE,
        ..common::past_first_run()
    }
}

fn settle(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.run_until_parked();
    let _ = cx.update_window(window, |_, window, cx| window.render_frame(cx));
}

fn select(app: &Entity<AppState>, title: &str, tab: DetailTab, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of(title)), cx);
        app.set_tab(tab, cx);
    });
    cx.run_until_parked();
}

/// The samples with Weekly sync transcribing, so its Transcript tab is the placeholder.
fn transcribing_weekly() -> std::sync::Arc<dory_core::testing::store::MemStore> {
    let store = sample_store();
    store
        .apply(vec![
            Change::JobEnqueued {
                id: id_of("Weekly sync"),
                goal: Goal::ALL,
                priority: Priority::Normal,
                picks: Default::default(),
                at: Timestamp(fixtures::now().timestamp_millis()),
            },
            Change::Job {
                job: JobId(1),
                event: JobEvent::Started {
                    stage: Stage::Transcribe,
                },
            },
        ])
        .expect("start transcribing Weekly sync");
    store
}

#[gpui_kit::test]
fn more_actions_stays_in_the_window_on_the_summary_tab_at_200_percent(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), at_200_percent());
    let ortega = "Customer interview: Ortega";
    for tab in [DetailTab::Summary, DetailTab::Transcript] {
        select(&app, ortega, tab, cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let more = window.find("more").bounds();
            let page = window.find("detail").bounds();
            assert!(
                more.left() >= page.left() && more.right() <= page.right(),
                "{tab:?}: More actions {more:?} runs out of the page {page:?}"
            );
            assert!(more.size.width > px(0.), "{tab:?}: More actions has a size");
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn copy_and_export_wait_for_text_and_say_so_to_voiceover(cx: &mut TestAppContext) {
    let (window, app) =
        open_with_prefs(cx, dory_on(transcribing_weekly()), common::past_first_run());
    let disabled = |window: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
        })
        .unwrap()
    };
    select(&app, "Weekly sync", DetailTab::Transcript, cx);
    assert!(disabled(window, "copy", cx), "no transcript yet: Copy");
    assert!(disabled(window, "export", cx), "no transcript yet: Export");
    select(&app, "Design review", DetailTab::Transcript, cx);
    assert!(!disabled(window, "copy", cx), "a transcript to copy");
    assert!(!disabled(window, "export", cx), "a transcript to export");
    // The store still holds Design review's old transcript, but a new one is on its way: the
    // tab shows the placeholder.
    app.update(cx, |app, cx| {
        let phase = Phase::Stopped {
            starts_in: 5,
            auto: true,
        };
        app.preview_phase(id_of("Design review"), phase, Vec::new(), cx)
    });
    cx.run_until_parked();
    assert!(
        disabled(window, "copy", cx),
        "a transcript on its way: Copy"
    );
    assert!(
        disabled(window, "export", cx),
        "a transcript on its way: Export"
    );
}

#[gpui_kit::test]
fn the_file_menu_waits_for_the_transcript_as_the_buttons_do(cx: &mut TestAppContext) {
    let (_, app) = open_with_prefs(cx, dory_on(sample_store()), common::past_first_run());
    let design = id_of("Design review");
    select(&app, "Design review", DetailTab::Transcript, cx);
    let state = |cx: &mut TestAppContext| cx.update(|cx| menus::menu_state(app.read(cx), cx));
    let ready = state(cx);
    assert!(ready.has_transcript, "a transcript to copy and export");
    assert!(ready.srt, "a timed transcript offers SRT");
    // The store still holds the old transcript, but the tab shows the placeholder.
    app.update(cx, |app, cx| {
        let phase = Phase::Stopped {
            starts_in: 5,
            auto: true,
        };
        app.preview_phase(design.clone(), phase, Vec::new(), cx)
    });
    cx.run_until_parked();
    let waiting = state(cx);
    assert!(
        !waiting.has_transcript,
        "File > Copy Transcript and Export wait while the transcript is on its way"
    );
    assert!(!waiting.srt, "SRT waits too");
    assert_eq!(
        waiting.has_summary, ready.has_summary,
        "the summary does not wait for the transcript"
    );
}

/// A queue window over the `name` fixture, with the app at `prefs`.
fn open_queue(
    name: &str,
    prefs: Prefs,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<AppState>) {
    open_queue_edited(name, prefs, |_| {}, cx)
}

/// As [`open_queue`], with the fixture changed by `edit` first.
fn open_queue_edited(
    name: &str,
    prefs: Prefs,
    edit: impl FnOnce(&mut dory_gpui::processing_queue::Queue),
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<AppState>) {
    let (_, app) = open_with_prefs(cx, dory_on(sample_store()), prefs);
    let mut state = fixtures::queue_state(name).unwrap();
    edit(&mut state);
    app.update(cx, |app, cx| app.preview_queue(Some(state), cx));
    cx.update(processing_window::install);
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| processing_window::open(&app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Processing window opened");
    settle(window, cx);
    (window, app)
}

#[gpui_kit::test]
fn a_finished_row_keeps_its_title_and_puts_the_status_under_it(cx: &mut TestAppContext) {
    let (window, _) = open_queue("finished", at_200_percent(), cx);
    cx.update_window(window, |_, window, _| {
        for job in [7, 8] {
            let title = window.find(format!("queue-finished-title-{job}")).bounds();
            let line = window.find(format!("queue-finished-line-{job}")).bounds();
            assert!(
                title.size.width >= px(200.),
                "job {job}: the title is starved to {:?}",
                title.size.width
            );
            assert!(
                line.top() >= title.bottom(),
                "job {job}: the status {line:?} sits under the title {title:?}"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn queue_badges_grow_with_the_text(cx: &mut TestAppContext) {
    let (window, _) = open_queue("running", at_200_percent(), cx);
    cx.update_window(window, |_, window, _| {
        let badge = window.find("queue-badge-2").bounds();
        assert!(
            badge.size.width >= px(44.) && badge.size.height >= px(44.),
            "the badge stayed {:?} at 200% text",
            badge.size
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_queue_longer_than_its_window_scrolls_and_ends_clear_of_the_footer(cx: &mut TestAppContext) {
    let (window, app) = open_queue("busy", common::past_first_run(), cx);
    let hiring = id_of("Hiring debrief");
    let card_clear_of_body = |window: AnyWindowHandle, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let card = window.find(format!("queue-attention-{hiring}")).bounds();
            let body = window.find("queue-body").bounds();
            (card.bottom(), body.bottom())
        })
        .unwrap()
    };
    let (card, body) = card_clear_of_body(window, cx);
    assert!(
        card > body,
        "the last card ends at {card:?}, past the list's bottom at {body:?}, so the list has to scroll to show it"
    );
    cx.update_window(window, |_, window, cx| {
        window.scroll(
            "queue-body",
            ScrollDelta::Pixels(point(px(0.), px(-4000.))),
            cx,
        );
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    settle(window, cx);
    let (card, body) = card_clear_of_body(window, cx);
    assert!(
        card <= body - px(8.),
        "the last card ends at {card:?}, under the footer; the list ends at {body:?}"
    );
    drop(app);
}

fn finished_row(
    window: AnyWindowHandle,
    job: u64,
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Bounds<gpui_kit::Pixels>,
    gpui_kit::Bounds<gpui_kit::Pixels>,
) {
    cx.update_window(window, |_, window, _| {
        (
            window.find(format!("queue-finished-title-{job}")).bounds(),
            window.find(format!("queue-finished-line-{job}")).bounds(),
        )
    })
    .unwrap()
}

#[gpui_kit::test]
fn a_finished_row_keeps_the_status_beside_the_title_when_both_fit(cx: &mut TestAppContext) {
    let (window, _) = open_queue("finished", common::past_first_run(), cx);
    for job in [7, 8] {
        let (title, line) = finished_row(window, job, cx);
        assert!(
            line.top() < title.bottom() && line.left() >= title.right(),
            "job {job}: the status {line:?} should sit beside the title {title:?}, not under it"
        );
    }
}

#[gpui_kit::test]
fn a_finished_row_wraps_the_status_under_a_title_too_long_to_share_its_line(
    cx: &mut TestAppContext,
) {
    let long =
        "A planning conversation with a very long title that cannot share a line with anything";
    let (window, _) = open_queue_edited(
        "finished",
        common::past_first_run(),
        |queue| {
            for finished in &mut queue.finished {
                finished.title = long.into();
            }
        },
        cx,
    );
    for job in [7, 8] {
        let (title, line) = finished_row(window, job, cx);
        assert!(
            line.top() >= title.bottom(),
            "job {job}: the status {line:?} should wrap under the title {title:?}"
        );
        assert!(
            title.size.width >= px(200.),
            "job {job}: the title is starved to {:?}",
            title.size.width
        );
    }
}

const SENTINEL: &str = "untouched";

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

fn transcript_on_its_way(app: &Entity<AppState>, title: &str, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| {
        let phase = Phase::Stopped {
            starts_in: 5,
            auto: true,
        };
        app.preview_phase(id_of(title), phase, Vec::new(), cx)
    });
    cx.run_until_parked();
}

/// The samples with Weekly sync's transcript stored but empty: a recording nobody spoke in.
fn weekly_with_an_empty_transcript() -> std::sync::Arc<MemStore> {
    let store = sample_store();
    store
        .apply(vec![Change::TranscriptReplaced {
            id: id_of("Weekly sync"),
            transcript: Transcript {
                revision: Revision(1),
                segments: Vec::new(),
                detected_languages: Vec::new(),
                produced_by: ModelRef::parse("fake/sample").unwrap(),
            },
        }])
        .expect("store the empty transcript");
    store
}

#[gpui_kit::test]
fn copy_and_export_do_nothing_by_any_route_while_the_transcript_is_on_its_way(
    cx: &mut TestAppContext,
) {
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), common::past_first_run());
    select(&app, "Design review", DetailTab::Summary, cx);
    transcript_on_its_way(&app, "Design review", cx);
    cx.write_to_clipboard(ClipboardItem::new_string(SENTINEL.into()));

    // The action handlers are the one place the rule lives: the buttons, both menus, and the
    // key bindings all dispatch these.
    cx.dispatch_action(window, CopyTranscript);
    cx.run_until_parked();
    assert_eq!(
        clipboard(cx).as_deref(),
        Some(SENTINEL),
        "Copy Transcript copied a transcript that is about to be replaced"
    );
    cx.dispatch_action(window, ExportSrt);
    cx.run_until_parked();
    assert!(
        !cx.did_prompt_for_new_path(),
        "Export as SRT offered to save a transcript that is about to be replaced"
    );

    // The summary does not wait, so it still copies and exports, alone.
    cx.dispatch_action(window, CopySummary);
    cx.run_until_parked();
    assert!(
        clipboard(cx).is_some_and(|text| text.contains("Design review")),
        "the summary copies"
    );
    let path = std::env::temp_dir().join(format!("dory-export-{}.md", std::process::id()));
    cx.dispatch_action(window, ExportMarkdown);
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path(), "the summary exports");
    let target = path.clone();
    cx.simulate_new_path_selection(move |_| Some(target));
    common::until(cx, "the export to be written", |_| {
        std::fs::read_to_string(&path).is_ok_and(|text| !text.is_empty())
    });
    let written = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(written.contains("## Summary"), "{written}");
    assert!(
        !written.contains("Transcript"),
        "the export holds the old transcript:\n{written}"
    );
}

#[gpui_kit::test]
fn an_empty_transcript_copies_and_exports_nothing(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(
        cx,
        dory_on(weekly_with_an_empty_transcript()),
        common::past_first_run(),
    );
    select(&app, "Weekly sync", DetailTab::Transcript, cx);
    cx.write_to_clipboard(ClipboardItem::new_string(SENTINEL.into()));
    cx.dispatch_action(window, CopyTranscript);
    cx.run_until_parked();
    assert_eq!(
        clipboard(cx).as_deref(),
        Some(SENTINEL),
        "Copy Transcript replaced the clipboard with nothing"
    );
    cx.dispatch_action(window, ExportMarkdown);
    cx.dispatch_action(window, ExportText);
    cx.dispatch_action(window, ExportSrt);
    cx.run_until_parked();
    assert!(
        !cx.did_prompt_for_new_path(),
        "an export offered to save a transcript with no words in it"
    );
}

/// The File menu's Copy and Export items as `(name, disabled)`.
fn file_menu(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<(String, bool)> {
    let menus = cx.update(|cx| menus::build(menus::menu_state(app.read(cx), cx)));
    let file = menus.iter().find(|menu| menu.name == "File").unwrap();
    file.items
        .iter()
        .filter_map(|item| match item {
            gpui_kit::MenuItem::Action { name, disabled, .. }
                if name.starts_with("Copy") || name.starts_with("Export") =>
            {
                Some((name.to_string(), *disabled))
            }
            _ => None,
        })
        .collect()
}

/// The menu items drawn now, as `(label, id)`.
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

/// Draws frames until the menu is open (`open`) or closed, or `wait` has passed, and says whether
/// it got there. Kit's menus animate, on the clock of the machine rather than the test's.
fn menu_reaches(
    window: AnyWindowHandle,
    open: bool,
    wait: std::time::Duration,
    cx: &mut TestAppContext,
) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        cx.run_until_parked();
        if menu_items(window, cx).is_empty() != open {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Whether the item `name` of the menu that `open` shows is dimmed. Kit's menu items do not say
/// so to the accessibility tree, so this clicks it: an enabled item runs and closes the menu, and
/// a dimmed one leaves it open.
fn is_dimmed(
    window: AnyWindowHandle,
    open: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App),
    name: &str,
    cx: &mut TestAppContext,
) -> bool {
    const SECOND: std::time::Duration = std::time::Duration::from_secs(1);
    assert!(
        menu_reaches(window, false, SECOND, cx),
        "the last menu is still open"
    );
    // A click that lands while the last menu is still going away is ignored, so ask again.
    let opened = (0..10).any(|_| {
        cx.update_window(window, |_, window, cx| open(window, cx))
            .unwrap();
        menu_reaches(window, true, SECOND / 4, cx)
    });
    assert!(opened, "the menu did not open");
    let items = menu_items(window, cx);
    let (_, id) = items
        .iter()
        .find(|(label, _)| label == name)
        .unwrap_or_else(|| panic!("no {name} in {items:?}"));
    let id = id.clone();
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    let still_open = !menu_reaches(window, false, SECOND * 7 / 10, cx);
    if still_open {
        cx.update_window(window, |_, window, cx| window.press("escape", cx))
            .unwrap();
        assert!(
            menu_reaches(window, false, SECOND, cx),
            "Escape closes the menu"
        );
    }
    still_open
}

fn disabled_in(items: &[(String, bool)], name: &str) -> bool {
    items
        .iter()
        .find(|(label, _)| label == name)
        .unwrap_or_else(|| panic!("no {name} in {items:?}"))
        .1
}

const COPY_AND_EXPORT: [&str; 5] = [
    "Copy Summary",
    "Copy Transcript",
    "Export as Markdown…",
    "Export as Text…",
    "Export as SRT…",
];

/// A recording menu, opened by `open`, offers Copy and Export exactly as the File menu does,
/// and `expect_dimmed` are the ones the File menu dims.
fn assert_a_recording_menu_follows_the_file_menu(
    store: std::sync::Arc<MemStore>,
    title: &str,
    waiting: bool,
    open: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App),
    expect_dimmed: &[&str],
    cx: &mut TestAppContext,
) {
    // The menus are the app-wide ones, as `main` installs them.
    let (window, app) = common::launch(cx, dory_on(store), common::past_first_run());
    select(&app, title, DetailTab::Transcript, cx);
    if waiting {
        transcript_on_its_way(&app, title, cx);
    }
    let file = file_menu(&app, cx);
    for name in COPY_AND_EXPORT {
        assert_eq!(
            disabled_in(&file, name),
            expect_dimmed.contains(&name),
            "File > {name}"
        );
        assert_eq!(
            is_dimmed(window, &open, name, cx),
            disabled_in(&file, name),
            "{name}: the recording menu differs from the File menu"
        );
    }
}

fn more(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.click("more", cx)
}

#[gpui_kit::test]
fn the_more_menu_offers_everything_when_the_recording_has_text(cx: &mut TestAppContext) {
    assert_a_recording_menu_follows_the_file_menu(
        sample_store(),
        "Design review",
        false,
        more,
        &[],
        cx,
    );
}

#[gpui_kit::test]
fn the_more_menu_waits_for_the_transcript_as_the_file_menu_does(cx: &mut TestAppContext) {
    assert_a_recording_menu_follows_the_file_menu(
        sample_store(),
        "Design review",
        true,
        more,
        &["Copy Transcript", "Export as SRT…"],
        cx,
    );
}

#[gpui_kit::test]
fn a_row_menu_offers_nothing_for_an_empty_transcript_as_the_file_menu_does(
    cx: &mut TestAppContext,
) {
    let row = format!("row-{}", id_of("Weekly sync"));
    assert_a_recording_menu_follows_the_file_menu(
        weekly_with_an_empty_transcript(),
        "Weekly sync",
        false,
        move |window, cx| window.right_click(row.clone(), cx),
        &COPY_AND_EXPORT,
        cx,
    );
}

#[gpui_kit::test]
fn a_row_menu_on_a_row_that_is_not_open_yet_offers_what_the_row_has(cx: &mut TestAppContext) {
    let (window, app) = common::launch(cx, dory_on(sample_store()), common::past_first_run());
    select(&app, "1:1 with Sam", DetailTab::Transcript, cx);
    let row = format!("row-{}", id_of("Design review"));
    for name in COPY_AND_EXPORT {
        let row = row.clone();
        assert!(
            !is_dimmed(
                window,
                move |w, cx| w.right_click(row.clone(), cx),
                name,
                cx
            ),
            "{name}: Design review has a summary and a transcript"
        );
        // The click selected the row, so the next menu is for the open recording.
        select(&app, "1:1 with Sam", DetailTab::Transcript, cx);
    }
}

#[gpui_kit::test]
fn the_summary_tabs_export_menu_leaves_out_subtitles_while_the_transcript_is_on_its_way(
    cx: &mut TestAppContext,
) {
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), common::past_first_run());
    select(&app, "Design review", DetailTab::Summary, cx);
    transcript_on_its_way(&app, "Design review", cx);
    let file = file_menu(&app, cx);
    for (name, file_name) in [
        ("Markdown", "Export as Markdown…"),
        ("Plain Text", "Export as Text…"),
        ("Subtitles (SRT)", "Export as SRT…"),
    ] {
        assert_eq!(
            is_dimmed(window, |w, cx| w.click("export", cx), name, cx),
            disabled_in(&file, file_name),
            "{name}: the Export menu differs from File > {file_name}"
        );
    }
    assert!(disabled_in(&file, "Export as SRT…"));
    assert!(!disabled_in(&file, "Export as Markdown…"));
}

#[gpui_kit::test]
fn the_selected_row_draws_its_ring_between_two_bands(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), common::past_first_run());
    let (picked, other) = (id_of("Design review"), id_of("1:1 with Sam"));
    app.update(cx, |app, cx| app.select(Selection::One(picked.clone()), cx));
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(format!("row-{picked}"), cx);
        window.render_frame(cx);
        let row = window.find(format!("row-{picked}")).bounds();
        let ring = window.find(format!("row-{picked}-ring")).bounds();
        // The line fills the row inside its 2 pt edge band, so the edge shows outside it and the
        // fill inside it.
        for (what, got, want) in [
            ("left", ring.left(), row.left() + px(2.)),
            ("top", ring.top(), row.top() + px(2.)),
            ("right", ring.right(), row.right() - px(2.)),
            ("bottom", ring.bottom(), row.bottom() - px(2.)),
        ] {
            assert!(
                (f32::from(got) - f32::from(want)).abs() < 0.5,
                "the ring's {what} is {got:?}, the row's inner {what} is {want:?}"
            );
        }
        assert!(
            window.try_find(format!("row-{other}-ring")).is_none(),
            "an unselected row has no ring"
        );
    })
    .unwrap();
}

/// At 800 x 560 and 200% text the sidebar shows about three rows. Scrolled to the selected last
/// row, the header of the group above it was cut in half under the search field.
#[gpui_kit::test]
fn a_group_header_scrolled_off_the_list_stays_at_its_top(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), at_200_percent());
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of("Roadmap review")), cx)
    });
    for _ in 0..2 {
        settle(window, cx);
    }
    for _ in 0..4 {
        settle(window, cx);
    }
    cx.update_window(window, |_, window, _| {
        let list = window.find("recordings").bounds();
        let sticky = window.try_find("sidebar-sticky-header");
        let sticky = sticky
            .expect("the header of the group the list scrolled into is pinned")
            .bounds();
        assert!(
            sticky.top() >= list.top() && sticky.top() <= list.top() + px(4.),
            "the pinned header {sticky:?} sits at the list's top {list:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_list_that_fits_pins_no_header(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), common::past_first_run());
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of("Weekly sync")), cx)
    });
    for _ in 0..4 {
        settle(window, cx);
    }
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("sidebar-sticky-header").is_none());
    })
    .unwrap();
}
