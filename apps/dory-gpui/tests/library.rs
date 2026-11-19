mod common;

use chrono::NaiveDate;
use std::collections::HashMap;

use dory_core::app::RecorderStatus;
use dory_core::domain::{Fraction, JobState, Ms, RecordingId, RecordingRow, Stage};
use dory_gpui::estimate::Left;
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::library::{
    DateGroup, Percent, Progress, RowStatus, SearchText, Selection, StatusFilter, after_removal,
    group_by_date, row_meta, short_duration,
};
use dory_gpui::processing::{Facts, Phase};

/// The row status `AppState` derives for a row, with nothing in the queue and no download.
fn status(row: &RecordingRow, recorder: &RecorderStatus, progress: Option<Progress>) -> RowStatus {
    let (models, downloads) = (HashMap::new(), HashMap::new());
    Phase::derive(&Facts {
        row,
        recorder,
        grace_left: None,
        sealing: false,
        progress,
        done: &[],
        translating: false,
        translating_summary: false,
        spot: None,
        left: Left::Estimating,
        auto_process: true,
        models: &models,
        downloads: &downloads,
    })
    .row()
}

fn titles(rows: &[&dory_core::domain::RecordingRow]) -> Vec<String> {
    rows.iter()
        .map(|r| r.recording.title.clone().unwrap_or_default())
        .collect()
}

#[test]
fn the_samples_group_by_today_yesterday_previous_week_and_month() {
    let rows = fixtures::rows();
    let groups = group_by_date(&rows, &fixtures::now());
    let today = fixtures::today();
    let shown: Vec<(String, Vec<String>)> = groups
        .iter()
        .map(|(group, members)| (group.label(today), titles(members)))
        .collect();
    let want = |label: &str, titles: &[&str]| {
        (
            label.to_owned(),
            titles.iter().map(|t| (*t).to_owned()).collect(),
        )
    };
    assert_eq!(
        shown,
        [
            want("Today", &["Weekly sync", "Design review", "1:1 with Sam"]),
            want(
                "Yesterday",
                &[
                    "Kundengespräch Müller GmbH",
                    "Partner call, Lyon office",
                    "Hiring debrief"
                ]
            ),
            want("Previous 7 days", &["Customer interview: Ortega"]),
            want("October", &["Roadmap review"]),
        ]
    );
}

#[test]
fn date_group_boundaries() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
    let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
    let cases = [
        (day(11, 13), DateGroup::Today),
        (day(11, 12), DateGroup::Today),
        (day(11, 11), DateGroup::Yesterday),
        (day(11, 10), DateGroup::Previous7Days),
        (day(11, 5), DateGroup::Previous7Days),
        (
            day(11, 4),
            DateGroup::Month {
                year: 2026,
                month: 11,
            },
        ),
    ];
    for (date, want) in cases {
        assert_eq!(DateGroup::of(date, today), want, "{date}");
    }
    let last_year = DateGroup::of(NaiveDate::from_ymd_opt(2025, 10, 28).unwrap(), today);
    assert_eq!(last_year.label(today), "October 2025");
    assert_eq!(
        DateGroup::Month {
            year: 2026,
            month: 11
        }
        .label(today),
        "November"
    );
}

/// The meta line and the player's clock round the same way, down, so one take never reads
/// "43 s" in one place and "0:42" in the other.
#[test]
fn durations_round_down_like_the_player_clock() {
    use dory_core::domain::Ms;
    assert_eq!(short_duration(Ms(42_600)), "42 s");
    assert_eq!(dory_gpui::a11y::clock(Ms(42_600)), "0:42");
    assert_eq!(short_duration(Ms(59_999)), "59 s");
    assert_eq!(short_duration(Ms(60_000)), "1 min");
    assert_eq!(short_duration(Ms(7_905_600)), "131 min");
    assert_eq!(dory_gpui::a11y::clock(Ms(7_905_600)), "2:11:45");
}

#[test]
fn row_meta_follows_the_canvas_per_group() {
    let now = fixtures::now();
    let today = fixtures::today();
    let meta = |title: &str| {
        let row = SAMPLES.iter().find(|s| s.title == title).unwrap().row();
        let day = dory_gpui::library::local(row.recording.created_at, &now.timezone());
        row_meta(&row, DateGroup::of(day.date_naive(), today), &now)
    };
    assert_eq!(meta("Weekly sync"), "14:00 · 62 min");
    assert_eq!(meta("Kundengespräch Müller GmbH"), "16:15 · 35 min");
    assert_eq!(meta("Customer interview: Ortega"), "Mon 9 Nov · 94 min");
    assert_eq!(meta("Roadmap review"), "28 Oct · 71 min");
}

/// Book's contents line moves the length beside the title; the two parts are the same words.
#[test]
fn book_splits_the_meta_line_into_its_time_and_length() {
    let now = fixtures::now();
    let today = fixtures::today();
    for sample in SAMPLES {
        let row = sample.row();
        let day = dory_gpui::library::local(row.recording.created_at, &now.timezone());
        let group = DateGroup::of(day.date_naive(), today);
        let (when, length) = dory_gpui::library::row_meta_parts(&row, group, &now);
        let joined = match &length {
            Some(length) => format!("{when} · {length}"),
            None => when.clone(),
        };
        assert_eq!(joined, row_meta(&row, group, &now), "{}", sample.title);
    }
}

#[test]
fn row_status_reads_the_job_the_recorder_and_progress() {
    let rows = fixtures::rows();
    let row = |title: &str| {
        rows.iter()
            .find(|r| r.recording.title.as_deref() == Some(title))
            .unwrap()
    };
    let idle = RecorderStatus::Idle;
    let forty = Progress {
        stage: Stage::Transcribe,
        fraction: Fraction::new(0.4),
        budget: Some(dory_core::app::Budget::Fast { threads: 4 }),
    };
    let weekly = row("Weekly sync");
    assert_eq!(
        status(weekly, &idle, Some(forty)).label(),
        "Transcribing 40%"
    );
    assert_eq!(
        status(weekly, &idle, None).label(),
        "Preparing",
        "running, before the first window"
    );
    assert_eq!(status(row("Design review"), &idle, None), RowStatus::Done);
    assert_eq!(
        status(row("Hiring debrief"), &idle, None).label(),
        "Needs attention"
    );

    let recording = RecorderStatus::Recording {
        id: weekly.recording.id.clone(),
        recorded: Ms(1_000),
    };
    assert_eq!(
        status(weekly, &recording, Some(forty)),
        RowStatus::Recording
    );

    let mut fresh = row("Design review").clone();
    fresh.job = None;
    fresh.has_transcript = false;
    assert_eq!(status(&fresh, &idle, None), RowStatus::ReadyToTranscribe);
    fresh.job = Some(JobState::Queued);
    assert_eq!(status(&fresh, &idle, None), RowStatus::Queued { ahead: 0 });
    fresh.job = Some(JobState::Running {
        stage: Stage::Summarize,
    });
    assert_eq!(status(&fresh, &idle, Some(forty)), RowStatus::Summarizing);
    assert_eq!(
        RowStatus::Transcribing {
            percent: Percent::of(Fraction::new(0.4)),
            light: true
        }
        .label(),
        "40%, light",
        "the short form UX-30 gives the row"
    );
}

#[test]
fn the_status_filter_keeps_only_matching_samples() {
    let rows = fixtures::rows();
    let kept = |filter: StatusFilter| -> Vec<String> {
        rows.iter()
            .filter(|r| filter.admits(status(r, &RecorderStatus::Idle, None)))
            .map(|r| r.recording.title.clone().unwrap())
            .collect()
    };
    assert_eq!(kept(StatusFilter::All).len(), SAMPLES.len());
    assert_eq!(kept(StatusFilter::Processing), ["Weekly sync"]);
    assert_eq!(kept(StatusFilter::NeedsAttention), ["Hiring debrief"]);
    assert_eq!(kept(StatusFilter::Done).len(), 6);
    assert!(StatusFilter::Processing.admits(RowStatus::ReadyToTranscribe));
    assert!(StatusFilter::Processing.admits(RowStatus::Recording));
}

#[test]
fn selection_after_delete_moves_to_the_next_then_the_previous_row() {
    let ids: Vec<RecordingId> = fixtures::rows()
        .into_iter()
        .map(|r| r.recording.id)
        .collect();
    let one = |ix: usize| Selection::One(ids[ix].clone());
    assert_eq!(after_removal(&ids, &ids[0]), one(1));
    assert_eq!(after_removal(&ids, &ids[3]), one(4));
    assert_eq!(after_removal(&ids, &ids[ids.len() - 1]), one(ids.len() - 2));
    assert_eq!(after_removal(&ids[..1], &ids[0]), Selection::None);
    assert_eq!(after_removal(&ids[1..], &ids[0]), Selection::None);
}

#[test]
fn search_trims_and_an_empty_search_lists_everything() {
    assert!(SearchText::new("   ").is_empty());
    assert_eq!(SearchText::new("  Lieferung ").text(), "Lieferung");
}

mod window {
    use dory_gpui::app::{AppState, SEARCH_DEBOUNCE};
    use dory_gpui::fixtures::{self, SAMPLES};
    use dory_gpui::library::{Selection, StatusFilter, group_by_date};
    use gpui_kit::{
        AppContext as _, ElementId, Entity, Role, TestAppContext, test::TestWindowExt as _,
    };

    use crate::common::{id_of, open};

    fn titles(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<String> {
        app.read_with(cx, |app, _| {
            app.visible()
                .iter()
                .map(|(row, _)| row.recording.title.clone().unwrap_or_default())
                .collect()
        })
    }

    #[gpui_kit::test]
    fn the_window_lists_every_sample_in_date_groups(cx: &mut TestAppContext) {
        let (_, app) = open(cx);
        let groups: Vec<(String, usize)> = app.read_with(cx, |app, _| {
            group_by_date(&app.recordings, &fixtures::now())
                .into_iter()
                .map(|(group, rows)| (group.label(fixtures::today()), rows.len()))
                .collect()
        });
        assert_eq!(
            groups,
            [
                ("Today".to_owned(), 3),
                ("Yesterday".to_owned(), 3),
                ("Previous 7 days".to_owned(), 1),
                ("October".to_owned(), 1),
            ]
        );
    }

    #[gpui_kit::test]
    fn search_waits_for_typing_to_pause_then_matches_transcript_text(cx: &mut TestAppContext) {
        let (_, app) = open(cx);
        app.update(cx, |app, cx| app.set_search("  lieferung ", cx));
        cx.run_until_parked();
        assert_eq!(titles(&app, cx).len(), SAMPLES.len(), "before the debounce");
        cx.executor().advance_clock(SEARCH_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(titles(&app, cx), ["Kundengespräch Müller GmbH"]);

        app.update(cx, |app, cx| app.set_search("mull gmb", cx));
        cx.executor().advance_clock(SEARCH_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(
            titles(&app, cx),
            ["Kundengespräch Müller GmbH"],
            "word starts in any order, with accents folded"
        );

        app.update(cx, |app, cx| app.set_search("", cx));
        cx.executor().advance_clock(SEARCH_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(titles(&app, cx).len(), SAMPLES.len());
    }

    #[gpui_kit::test]
    fn the_filter_narrows_the_visible_rows(cx: &mut TestAppContext) {
        let (_, app) = open(cx);
        app.update(cx, |app, cx| app.set_filter(StatusFilter::Processing, cx));
        assert_eq!(titles(&app, cx), ["Weekly sync"]);
        app.update(cx, |app, cx| app.set_filter(StatusFilter::Done, cx));
        assert!(!titles(&app, cx).contains(&"Weekly sync".to_owned()));
        app.update(cx, |app, cx| app.set_filter(StatusFilter::All, cx));
        assert_eq!(titles(&app, cx).len(), SAMPLES.len());
    }

    #[gpui_kit::test]
    fn deleting_the_selection_moves_it_to_the_next_row_then_the_previous(cx: &mut TestAppContext) {
        let (_, app) = open(cx);
        app.update(cx, |app, cx| {
            app.select(Selection::One(id_of("Design review")), cx)
        });
        app.update(cx, |app, cx| app.delete(id_of("Design review"), cx));
        cx.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.selection.clone()),
            Selection::One(id_of("1:1 with Sam"))
        );
        assert!(!titles(&app, cx).contains(&"Design review".to_owned()));

        app.update(cx, |app, cx| {
            app.select(Selection::One(id_of("Roadmap review")), cx)
        });
        app.update(cx, |app, cx| app.delete(id_of("Roadmap review"), cx));
        cx.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.selection.clone()),
            Selection::One(id_of("Customer interview: Ortega"))
        );
    }

    #[gpui_kit::test]
    fn delete_moves_to_the_trash_with_an_undo_that_restores_and_selects(cx: &mut TestAppContext) {
        let (_, app) = open(cx);
        let id = id_of("Design review");
        app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
        app.update(cx, |app, cx| app.delete_selection(cx));
        cx.run_until_parked();
        assert!(!titles(&app, cx).contains(&"Design review".to_owned()));
        let notice = app
            .read_with(cx, |app, _| app.notice().cloned())
            .expect("a notice");
        assert!(notice.undo, "{notice:?}");
        assert!(app.read_with(cx, |app, _| app.can_undo()));

        app.update(cx, |app, cx| app.undo_delete(cx));
        cx.run_until_parked();
        assert!(titles(&app, cx).contains(&"Design review".to_owned()));
        assert_eq!(
            app.read_with(cx, |app, _| app.selection.clone()),
            Selection::One(id)
        );
        assert!(!app.read_with(cx, |app, _| app.can_undo()));
        assert!(!app.read_with(cx, |app, _| app.notice().unwrap().undo));

        cx.executor().advance_clock(dory_gpui::app::NOTICE_TTL);
        cx.run_until_parked();
        assert!(
            app.read_with(cx, |app, _| app.notice().is_none()),
            "the notice clears"
        );
    }

    #[gpui_kit::test]
    fn the_start_screen_has_focus_and_tab_walks_sidebar_then_toolbar(cx: &mut TestAppContext) {
        let (window, _) = open(cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("start-recording");
            assert_eq!(start.role(), Some(Role::Button));
            assert_eq!(start.label(), Some("Start Recording"));
            assert_eq!(start.focused(), Some(true), "focused at launch");

            let order: [ElementId; 7] = [
                "search".into(),
                "filter".into(),
                "recordings".into(),
                "sidebar-settings".into(),
                "sidebar-resize".into(),
                "record".into(),
                "start-recording".into(),
            ];
            let mut seen = Vec::new();
            for _ in 0..order.len() {
                window.press("tab", cx);
                window.render_frame(cx);
                seen.push(
                    order
                        .iter()
                        .find(|id| window.find((*id).clone()).focused() == Some(true))
                        .cloned(),
                );
            }
            let order: Vec<Option<ElementId>> = order.into_iter().map(Some).collect();
            assert_eq!(seen, order, "Tab order");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn rows_and_controls_carry_roles_and_labels(cx: &mut TestAppContext) {
        let (window, _) = open(cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("recordings").role(), Some(Role::List));
            assert_eq!(window.find("record").label(), Some("Start recording"));
            assert_eq!(window.find("search").role(), Some(Role::TextInput));
            let row = window.find(ElementId::Name(
                format!("row-{}", id_of("Design review")).into(),
            ));
            assert_eq!(row.role(), Some(Role::ListItem));
            let label = row.label().unwrap_or_default().to_owned();
            assert!(
                label.starts_with("Design review, ") && label.ends_with(", 48 min, Done"),
                "{label}"
            );

            for _ in 0..6 {
                if window.find("recordings").focused() == Some(true) {
                    break;
                }
                window.press("tab", cx);
                window.render_frame(cx);
            }
            assert_eq!(window.find("recordings").focused(), Some(true));
            window.press("down", cx);
            window.render_frame(cx);
            let first = window.find(ElementId::Name(
                format!("row-{}", id_of("Weekly sync")).into(),
            ));
            assert_eq!(first.selected(), Some(true), "Down selects the first row");
            window.press("down", cx);
            window.render_frame(cx);
            assert_eq!(
                window
                    .find(ElementId::Name(
                        format!("row-{}", id_of("Design review")).into()
                    ))
                    .selected(),
                Some(true)
            );
        })
        .unwrap();
    }

    /// The search field shows a search icon, and a "Clear search" button at its end while it holds
    /// text. The button empties the field, keeps focus in it, and brings every recording back.
    #[gpui_kit::test]
    fn the_clear_button_empties_the_search_and_shows_every_recording(cx: &mut TestAppContext) {
        let (window, app) = open(cx);
        let all = titles(&app, cx).len();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("search-clear").is_none(),
                "no clear button while the field is empty"
            );
            window.click("search", cx);
            window.input("Design", cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.executor().advance_clock(SEARCH_DEBOUNCE);
        cx.run_until_parked();
        assert!(titles(&app, cx).len() < all, "typing filters the list");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let clear = window.find("search-clear");
            assert_eq!(clear.label(), Some("Clear search"));
            assert_eq!(clear.role(), Some(Role::Button));
            window.click("search-clear", cx);
            window.render_frame(cx);
            assert!(window.try_find("search-clear").is_none(), "gone once empty");
        })
        .unwrap();
        cx.executor().advance_clock(SEARCH_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(titles(&app, cx).len(), all, "every recording is back");
    }
}
