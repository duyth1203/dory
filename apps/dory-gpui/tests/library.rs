use chrono::NaiveDate;
use dory_core::app::RecorderStatus;
use dory_core::domain::{Fraction, JobState, Ms, RecordingId, Stage};
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::library::{
    DateGroup, Progress, RowStatus, SearchQuery, Selection, StatusFilter, after_removal,
    group_by_date, row_meta,
};

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
    };
    let weekly = row("Weekly sync");
    assert_eq!(
        RowStatus::derive(weekly, &idle, Some(forty)).label(),
        "Transcribing, 40%"
    );
    assert_eq!(
        RowStatus::derive(weekly, &idle, None).label(),
        "Transcribing"
    );
    assert_eq!(
        RowStatus::derive(row("Design review"), &idle, None),
        RowStatus::Done
    );
    assert_eq!(
        RowStatus::derive(row("Hiring debrief"), &idle, None).label(),
        "Needs attention"
    );

    let recording = RecorderStatus::Recording {
        id: weekly.recording.id.clone(),
        recorded: Ms(1_000),
    };
    assert_eq!(
        RowStatus::derive(weekly, &recording, Some(forty)),
        RowStatus::Recording
    );

    let mut fresh = row("Design review").clone();
    fresh.job = None;
    fresh.has_transcript = false;
    assert_eq!(
        RowStatus::derive(&fresh, &idle, None),
        RowStatus::ReadyToTranscribe
    );
    fresh.job = Some(JobState::Queued);
    assert_eq!(RowStatus::derive(&fresh, &idle, None), RowStatus::Queued);
    fresh.job = Some(JobState::Running {
        stage: Stage::Summarize,
    });
    assert_eq!(
        RowStatus::derive(&fresh, &idle, Some(forty)),
        RowStatus::Summarizing
    );
}

#[test]
fn the_status_filter_keeps_only_matching_samples() {
    let rows = fixtures::rows();
    let kept = |filter: StatusFilter| -> Vec<String> {
        rows.iter()
            .filter(|r| filter.admits(RowStatus::derive(r, &RecorderStatus::Idle, None)))
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
    assert_eq!(SearchQuery::new("   ").to_query().text, None);
    assert_eq!(
        SearchQuery::new("  Lieferung ").to_query().text.as_deref(),
        Some("Lieferung")
    );
}

mod window {
    use std::sync::Arc;

    use dory_core::app::{Dory, Options, Ports};
    use dory_core::domain::RecordingId;
    use dory_core::domain::Timestamp;
    use dory_core::ports::{Change, Store as _};
    use dory_core::testing::playback::FakePlayback;
    use dory_core::testing::store::MemStore;
    use dory_core::testing::{FakeCapture, ManualClock, MemTakes};
    use dory_gpui::app::{AppState, SEARCH_DEBOUNCE};
    use dory_gpui::fixtures::{self, SAMPLES};
    use dory_gpui::library::{Selection, StatusFilter, group_by_date};
    use dory_gpui::prefs::Prefs;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, ElementId, Entity, Role, TestAppContext,
        WindowBounds, WindowOptions, px, size, test::TestWindowExt as _,
    };

    /// A real `Dory` over the core's in-memory store, seeded with the section 6 samples. The
    /// store runs the same contract suite as SQLite, search included.
    fn sample_dory() -> Dory {
        let store = Arc::new(MemStore::new());
        let mut changes = Vec::new();
        for sample in &SAMPLES {
            changes.push(Change::RecordingCreated(sample.recording()));
            if let Some(transcript) = sample.transcript() {
                changes.push(Change::TranscriptReplaced {
                    id: sample.id(),
                    transcript,
                });
            }
            if let Some(summary) = sample.summary() {
                changes.push(Change::SummaryReplaced {
                    id: sample.id(),
                    summary,
                });
            }
        }
        store.apply(changes).expect("seed the samples");
        Dory::open(
            Ports {
                store,
                takes: Arc::new(MemTakes::new()),
                capture: Arc::new(FakeCapture::new()),
                playback: Arc::new(FakePlayback::new()),
                trash: Arc::new(dory_core::testing::FakeTrash::new(std::env::temp_dir())),
                clock: Arc::new(ManualClock::new(Timestamp(
                    fixtures::now().timestamp_millis(),
                ))),
                stt: Vec::new(),
                llm: Vec::new(),
                diar: Vec::new(),
                guard: None,
            },
            Options::default(),
        )
        .expect("open Dory on the samples")
    }

    fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
        cx.update(gpui_kit::init);
        let dory = sample_dory();
        let opened = cx.update(|cx| {
            let mut state = None;
            let (window, _) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1100.), px(720.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let (app, shell) = dory_gpui::build(dory, Prefs::default(), window, cx);
                    state = Some(app);
                    shell
                },
            )
            .expect("open the window");
            (window, state.expect("the app state"))
        });
        cx.run_until_parked();
        opened
    }

    fn titles(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<String> {
        app.read_with(cx, |app, _| {
            app.visible()
                .iter()
                .map(|(row, _)| row.recording.title.clone().unwrap_or_default())
                .collect()
        })
    }

    fn id_of(title: &str) -> RecordingId {
        SAMPLES.iter().find(|s| s.title == title).unwrap().id()
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
    fn the_start_screen_has_focus_and_tab_walks_sidebar_then_toolbar(cx: &mut TestAppContext) {
        let (window, _) = open(cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let start = window.find("start-recording");
            assert_eq!(start.role(), Some(Role::Button));
            assert_eq!(start.label(), Some("Start Recording"));
            assert_eq!(start.focused(), Some(true), "focused at launch");

            let order: [ElementId; 5] = [
                "search".into(),
                "filter".into(),
                "recordings".into(),
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
                label.starts_with("Design review, ") && label.ends_with(", 48 minutes, Done"),
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
}
