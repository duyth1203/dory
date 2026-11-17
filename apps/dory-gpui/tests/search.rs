//! UX-32: the sidebar while the search field has text. Two sections, "Titles" and "In
//! conversations", each a heading; a snippet and a VoiceOver sentence for every conversation
//! hit; one spoken result count when a search settles; the line that names a correction; and the
//! empty state. `TestAppContext` never starts AccessKit, so the tree is read from the elements
//! each frame registers with Kit's test support.

mod common;

use std::path::Path;
use std::sync::Arc;

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{
    Channel, ChannelLayout, LanguagePref, Ms, Recording, RecordingId, Revision, Segment, SpeakerId,
    SpeakerNames, Summary, TimeRange, Timestamp, Transcript,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store as _, Takes as _};
use dory_core::search::{Correction, Excerpt, Query, Section, Snippet, Source};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{FakeCapture, ManualClock, MemTakes};
use dory_gpui::actions::{FindNext, FindPrevious};
use dory_gpui::app::{AppState, DetailTab, SEARCH_DEBOUNCE};
use dory_gpui::find::Place;
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::library::{
    Selection, StatusFilter, corrected_words, match_words, results_announcement, section_label,
    title_marks,
};
use dory_gpui::prefs::Prefs;
use dory_gpui::transcript_view::PAGED_TURNS;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Entity, Role, TestAppContext,
    WindowBounds, WindowOptions, px, size, test::TestWindowExt as _,
};

use common::{id_of, open};

fn search(app: &Entity<AppState>, text: &str, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| app.set_search(text, cx));
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
}

fn id(name: &str) -> ElementId {
    ElementId::Name(name.to_owned().into())
}

fn row_label(window: AnyWindowHandle, title: &str, cx: &mut TestAppContext) -> String {
    let row = id(&format!("row-{}", id_of(title)));
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.find(row).label().unwrap_or_default().to_owned()
    })
    .unwrap()
}

/// Which of the sidebar's headings the last frame drew, as (id, label).
fn headings(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<(String, String)> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        ["section-Titles", "section-In conversations"]
            .into_iter()
            .filter_map(|name| {
                let found = window.try_find(id(name))?;
                assert_eq!(found.role(), Some(Role::Heading), "{name}");
                Some((name.to_owned(), found.label()?.to_owned()))
            })
            .collect()
    })
    .unwrap()
}

fn labels(headings: &[(String, String)]) -> Vec<&str> {
    headings.iter().map(|(_, label)| label.as_str()).collect()
}

fn order(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<String> {
    app.read_with(cx, |app, _| {
        app.visible()
            .iter()
            .map(|(row, _)| row.recording.title.clone().unwrap_or_default())
            .collect()
    })
}

#[gpui_kit::test]
fn titles_come_first_then_conversations_each_under_a_heading(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "design", cx);
    assert_eq!(
        labels(&headings(window, cx)),
        ["Titles", "In conversations"]
    );
    assert_eq!(
        order(&app, cx),
        ["Design review", "Hiring debrief"],
        "the title hit, then the transcript hit"
    );
    app.read_with(cx, |app, _| {
        let sections = app.visible_sections().expect("a settled search");
        let shown: Vec<(Section, usize)> = sections
            .iter()
            .map(|(section, rows)| (*section, rows.len()))
            .collect();
        assert_eq!(shown, [(Section::Titles, 1), (Section::Conversations, 1)]);
    });
}

#[gpui_kit::test]
fn a_word_found_only_in_transcripts_draws_no_titles_heading(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "mentoring", cx);
    assert_eq!(labels(&headings(window, cx)), ["In conversations"]);
    assert_eq!(order(&app, cx), ["1:1 with Sam"]);
}

#[gpui_kit::test]
fn a_word_found_only_in_titles_draws_no_conversations_heading(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "weekly", cx);
    assert_eq!(labels(&headings(window, cx)), ["Titles"]);
}

#[gpui_kit::test]
fn a_conversation_row_says_where_it_matched_and_a_title_row_says_so(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "design", cx);
    let said = row_label(window, "Hiring debrief", cx);
    assert!(
        said.starts_with("Hiring debrief, ")
            && said.ends_with(", 19 min, Done. In transcript, strong on systems design."),
        "{said}"
    );
    let said = row_label(window, "Design review", cx);
    assert!(said.ends_with(", 48 min, Done. Title match"), "{said}");
    app.read_with(cx, |app, _| {
        let excerpt = &app.snippets[&id_of("Hiring debrief")];
        assert_eq!(excerpt.source, Source::Transcript);
        let marked: Vec<&str> = excerpt
            .snippet
            .marks
            .iter()
            .map(|m| &excerpt.snippet.text[m.clone()])
            .collect();
        assert_eq!(marked, ["design"]);
        assert!(
            !app.snippets.contains_key(&id_of("Design review")),
            "a title hit has no snippet"
        );
    });
}

#[gpui_kit::test]
fn a_summary_match_says_in_summary(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "key points", cx);
    let rows = order(&app, cx);
    assert!(!rows.is_empty());
    for title in &rows {
        let said = row_label(window, title, cx);
        assert!(said.ends_with(". In summary, Key points"), "{said}");
    }
    assert_eq!(labels(&headings(window, cx)), ["In conversations"]);
}

#[gpui_kit::test]
fn the_result_count_is_spoken_once_when_the_search_settles(cx: &mut TestAppContext) {
    let (_, app) = open(cx);
    let said = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.announced.clone());
    assert_eq!(said(cx), None);

    app.update(cx, |app, cx| app.set_search("design", cx));
    cx.run_until_parked();
    assert_eq!(said(cx), None, "typing that has not settled says nothing");
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(said(cx).as_deref(), Some("2 results"));

    app.update(cx, |app, _| app.announced = None);
    app.update(cx, |app, cx| app.set_search("mentoring", cx));
    cx.run_until_parked();
    assert_eq!(said(cx), None, "a reload for an event is not a settle");
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(said(cx).as_deref(), Some("1 result"));

    search(&app, "zzqx", cx);
    assert_eq!(said(cx).as_deref(), Some("No results"));

    search(&app, "", cx);
    assert_eq!(said(cx), None, "clearing the field says nothing");
}

#[gpui_kit::test]
fn a_corrected_search_names_the_words_above_the_sections(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "roadmpa", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let line = window.find("search-correction");
        assert_eq!(line.label(), Some("Showing results for roadmap"));
        assert_eq!(line.role(), Some(Role::Label));
    })
    .unwrap();
    assert_eq!(labels(&headings(window, cx)), ["Titles"]);
    assert_eq!(order(&app, cx), ["Roadmap review"]);
    assert_eq!(
        app.read_with(cx, |app, _| app.announced.clone()).as_deref(),
        Some("Showing results for roadmap, 1 result")
    );

    search(&app, "roadmap", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("search-correction").is_none(),
            "an exact search shows no correction"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn nothing_matching_says_so_and_draws_no_headings(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "zzqx", cx);
    assert!(headings(window, cx).is_empty());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("sidebar-empty").label(),
            Some("No recordings match “zzqx”.")
        );
        assert!(window.try_find("group-Today").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_status_filter_applies_on_top_of_a_search(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "design", cx);
    app.update(cx, |app, cx| app.set_filter(StatusFilter::Done, cx));
    cx.run_until_parked();
    assert_eq!(order(&app, cx), ["Design review", "Hiring debrief"]);
    assert_eq!(
        labels(&headings(window, cx)),
        ["Titles", "In conversations"]
    );
    app.update(cx, |app, cx| app.set_filter(StatusFilter::Processing, cx));
    cx.run_until_parked();
    assert!(order(&app, cx).is_empty());
    assert!(headings(window, cx).is_empty());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("sidebar-empty").label(),
            Some("No recordings match “design”.")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn clearing_the_search_brings_the_date_groups_back_and_keeps_the_selection(
    cx: &mut TestAppContext,
) {
    let (window, app) = open(cx);
    search(&app, "design", cx);
    let kept = id_of("Hiring debrief");
    app.update(cx, |app, cx| {
        app.select(dory_gpui::library::Selection::One(kept.clone()), cx)
    });
    search(&app, "", cx);
    assert!(headings(window, cx).is_empty());
    let dated = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .filter(|e| e.role() == Some(Role::Heading))
                .count()
        })
        .unwrap();
    assert!(dated >= 2, "date group headings are back: {dated}");
    assert_eq!(
        app.read_with(cx, |app, _| app.selection.id().cloned()),
        Some(kept)
    );
    app.read_with(cx, |app, _| {
        assert!(app.search.is_none() && app.snippets.is_empty());
    });
}

#[gpui_kit::test]
fn rows_grow_to_fit_their_snippet_and_never_overlap_at_200_percent(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    search(&app, "design", cx);
    let heights = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let bounds = |title: &str| window.find(id(&format!("row-{}", id_of(title)))).bounds();
            (bounds("Design review"), bounds("Hiring debrief"))
        })
        .unwrap()
    };
    let (title_row, talk_row) = heights(cx);
    assert!(
        f32::from(talk_row.size.height) > f32::from(title_row.size.height) + 10.,
        "the snippet adds a line: {title_row:?} {talk_row:?}"
    );

    app.update(cx, |app, cx| app.set_text_scale(2.0, cx));
    cx.run_until_parked();
    let (big_title, big_talk) = heights(cx);
    assert!(f32::from(big_talk.size.height) > f32::from(talk_row.size.height));
    assert!(
        big_title.bottom() <= big_talk.top(),
        "rows stack without overlap: {big_title:?} {big_talk:?}"
    );
}

#[gpui_kit::test]
fn snippets_past_the_first_screen_fill_in_after_the_rows(cx: &mut TestAppContext) {
    use dory_core::domain::{
        Channel, ChannelLayout, LanguagePref, Ms, Recording, RecordingId, Revision, Segment,
        SpeakerId, TimeRange, Timestamp, Transcript,
    };
    use dory_core::models::ModelRef;
    use dory_core::ports::{Change, Store as _};
    use dory_core::testing::store::MemStore;

    let store = std::sync::Arc::new(MemStore::new());
    let mut changes = Vec::new();
    for n in 0..45_i64 {
        let id = RecordingId::generate(Timestamp(1_763_000_000_000 + n * 60_000), 0);
        changes.push(Change::RecordingCreated(Recording {
            id: id.clone(),
            title: Some(format!("Call {n}")),
            created_at: Timestamp(1_763_000_000_000 + n * 60_000),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: None,
        }));
        changes.push(Change::TranscriptReplaced {
            id,
            transcript: Transcript {
                revision: Revision(1),
                segments: vec![Segment {
                    range: TimeRange::new(Ms(0), Ms(1_000)),
                    speaker: SpeakerId(1),
                    channel: Channel::System,
                    text: format!("We talked about kilimanjaro on call {n}."),
                }],
                detected_languages: Vec::new(),
                produced_by: ModelRef::parse("fake/sample").unwrap(),
            },
        });
    }
    store.apply(changes).unwrap();
    let (_, app) = common::open_with(cx, common::dory_on(store));
    search(&app, "kilimanjaro", cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.recordings.len(), 45);
        assert_eq!(app.snippets.len(), 45, "the first 30 and the 15 after them");
    });
}

#[test]
fn the_announcement_names_the_count_and_any_correction() {
    let fixed = |typed: &str, used: &[&str]| Correction {
        typed: typed.into(),
        used: used.iter().map(|u| (*u).into()).collect(),
    };
    let cases: [(usize, Vec<Correction>, &str); 6] = [
        (12, vec![], "12 results"),
        (1, vec![], "1 result"),
        (0, vec![], "No results"),
        (
            3,
            vec![fixed("roadmpa", &["roadmap"])],
            "Showing results for roadmap, 3 results",
        ),
        (
            1,
            vec![fixed("budgte", &["budget", "budgets"])],
            "Showing results for budget, budgets, 1 result",
        ),
        (
            2,
            vec![fixed("a", &["alpha"]), fixed("b", &["beta"])],
            "Showing results for alpha, beta, 2 results",
        ),
    ];
    for (count, corrections, want) in cases {
        assert_eq!(results_announcement(count, &corrections), want);
    }
    assert_eq!(corrected_words(&[]), None);
}

#[test]
fn the_section_headings_and_the_where_words() {
    assert_eq!(section_label(Section::Titles), "Titles");
    assert_eq!(section_label(Section::Conversations), "In conversations");
    let excerpt = |source, text: &str| Excerpt {
        source,
        item: 0,
        snippet: Snippet {
            text: text.into(),
            marks: Vec::new(),
        },
    };
    let said = |section, excerpt: Option<&Excerpt>| match_words(section, excerpt);
    assert_eq!(said(Section::Titles, None).as_deref(), Some("Title match"));
    let talk = excerpt(Source::Transcript, "…we agreed the roadmap slips to Q3…");
    assert_eq!(
        said(Section::Conversations, Some(&talk)).as_deref(),
        Some("In transcript, we agreed the roadmap slips to Q3"),
        "the cut marks are not spoken"
    );
    assert_eq!(
        said(
            Section::Conversations,
            Some(&excerpt(Source::Summary, "Budget approved"))
        )
        .as_deref(),
        Some("In summary, Budget approved")
    );
    assert_eq!(
        said(
            Section::Conversations,
            Some(&excerpt(Source::Speakers, "Rita"))
        )
        .as_deref(),
        Some("Speaker name, Rita")
    );
    assert_eq!(said(Section::Conversations, None), None, "not loaded yet");
}

#[gpui_kit::test]
fn escape_in_the_field_clears_the_search_and_keeps_focus_there(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("search", cx);
        window.input("design", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(
        labels(&headings(window, cx)),
        ["Titles", "In conversations"]
    );

    cx.update_window(window, |_, window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
    app.read_with(cx, |app, _| {
        assert!(app.typed.is_empty() && app.search.is_none())
    });
    assert!(headings(window, cx).is_empty());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("search").focused(), Some(true));
    })
    .unwrap();
}

/// Timed, with a take and a summary: the roadmap comes up in turns 1 and 3 and in the summary.
const TALK: &str = "Design review";
/// Longer than [`PAGED_TURNS`], so its turns virtualize. The roadmap comes up in turns 100 and 140.
const LONG: &str = "Roadmap review";
const LONG_TURNS: usize = 150;
/// Short enough to lay out whole in the page, and long enough that turn 65 is far below the fold.
const MID: &str = "Customer interview: Ortega";
const MID_TURNS: usize = 70;
/// A word that only its summary has.
const SUMMARY_ONLY: &str = "Hiring debrief";
/// Imported from the old app: no times and no take.
const LEGACY: &str = "1:1 with Sam";
/// Its summary is longer than the text view parses at once, so its parse lands a moment later.
const LONG_SUMMARY_PARAGRAPHS: usize = 150;
/// Its summary has a link whose address holds the word "roadmap".
const LINKED: &str = "Partner call, Lyon office";

fn segment(speaker: u16, channel: Channel, range: Option<(u64, u64)>, text: &str) -> Segment {
    Segment {
        range: range.and_then(|(a, b)| TimeRange::new(Ms(a), Ms(b))),
        speaker: SpeakerId(speaker),
        channel,
        text: text.to_owned(),
    }
}

fn spoken_transcript(segments: Vec<Segment>) -> Transcript {
    Transcript {
        revision: Revision(1),
        segments,
        detected_languages: Vec::new(),
        produced_by: ModelRef::parse("fake/sample").unwrap(),
    }
}

fn corpus_transcript(title: &str) -> Option<Transcript> {
    let segments = match title {
        TALK => vec![
            segment(2, Channel::System, Some((0, 5_000)), "Welcome everyone."),
            segment(
                3,
                Channel::System,
                Some((750_000, 760_000)),
                "The roadmap slips to Q3.",
            ),
            segment(2, Channel::System, Some((760_000, 770_000)), "Fine by me."),
            segment(
                3,
                Channel::System,
                Some((780_000, 790_000)),
                "Roadmap items and the roadmap review.",
            ),
        ],
        LEGACY => vec![
            segment(1, Channel::System, None, "Sam wants a mentoring plan."),
            segment(0, Channel::Mic, None, "Let's draft one."),
        ],
        MID => (0..MID_TURNS)
            .map(|i| {
                let start = i as u64 * 10_000;
                let text = match i {
                    65 => format!("Line {i} on the roadmap."),
                    68 => format!("Line {i} on the roadmap, says Ortega."),
                    _ => format!("Line {i}."),
                };
                segment(
                    1 + (i % 2) as u16,
                    Channel::System,
                    Some((start, start + 9_000)),
                    &text,
                )
            })
            .collect(),
        LONG => (0..LONG_TURNS)
            .map(|i| {
                let start = i as u64 * 10_000;
                let text = match i {
                    100 | 140 => format!("Line {i} on the roadmap."),
                    _ => format!("Line {i}."),
                };
                segment(
                    1 + (i % 2) as u16,
                    Channel::System,
                    Some((start, start + 9_000)),
                    &text,
                )
            })
            .collect(),
        _ => return SAMPLES.iter().find(|s| s.title == title)?.transcript(),
    };
    Some(spoken_transcript(segments))
}

fn corpus_summary(title: &str) -> Option<Summary> {
    let markdown = match title {
        TALK => "## Roadmap notes\n\n- Ship the docked player.\n- The roadmap moves to Q3.\n",
        SUMMARY_ONLY => "## Notes\n\n- Kilimanjaro came up twice.\n",
        MID => {
            return Some(Summary {
                markdown: format!(
                    "{}\n\nZanzibar ends it, and the Zanzibar trip is last.\n",
                    (0..LONG_SUMMARY_PARAGRAPHS)
                        .map(|i| format!("Paragraph {i} is filler about nothing at all."))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                ),
                language: None,
                produced_by: ModelRef::parse("fake/sample").unwrap(),
                transcript_revision: Revision(1),
            });
        }
        LINKED => {
            "## Plan\n\nSee [the plan](https://roadmap.example.com/x) for the roadmap.\n\nThe roadmap slips.\n"
        }
        _ => return None,
    };
    Some(Summary {
        markdown: markdown.to_owned(),
        language: None,
        produced_by: ModelRef::parse("fake/sample").unwrap(),
        transcript_revision: Revision(1),
    })
}

/// The samples with the transcripts and summaries above, and takes for [`TALK`] and [`LONG`].
fn open_corpus(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>, Arc<FakePlayback>) {
    open_corpus_with(Vec::new(), cx)
}

/// [`open_corpus`] with `extra` changes applied after the samples.
fn open_corpus_with(
    extra: Vec<Change>,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<AppState>, Arc<FakePlayback>) {
    cx.update(gpui_kit::init);
    let store = Arc::new(MemStore::new());
    let mut changes = Vec::new();
    for sample in &SAMPLES {
        changes.push(Change::RecordingCreated(sample.recording()));
        if let Some(transcript) = corpus_transcript(sample.title) {
            changes.push(Change::TranscriptReplaced {
                id: sample.id(),
                transcript,
            });
        }
        if let Some(summary) = corpus_summary(sample.title) {
            changes.push(Change::SummaryReplaced {
                id: sample.id(),
                summary,
            });
        }
    }
    changes.extend(extra);
    store.apply(changes).expect("seed the corpus");
    let takes = Arc::new(MemTakes::new());
    for title in [TALK, LONG] {
        takes
            .import(&id_of(title), Path::new("take.flac"))
            .expect("a take");
    }
    let playback = Arc::new(FakePlayback::new());
    let dory = Dory::open(
        Ports {
            store,
            takes,
            capture: Arc::new(FakeCapture::new()),
            playback: playback.clone(),
            trash: Arc::new(dory_core::testing::FakeTrash::new(std::env::temp_dir())),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: Vec::new(),
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        Options::default(),
    )
    .expect("open Dory");
    let prefs = Prefs {
        onboarding: Some(dory_gpui::onboarding::OnboardingStep::Done),
        ..Prefs::default()
    };
    let (window, app) = cx.update(|cx| {
        let mut state = None;
        let (window, shell) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1100.), px(900.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let (app, shell) = dory_gpui::build(dory, prefs, window, cx);
                state = Some(app);
                shell
            },
        )
        .expect("open the window");
        SHELL.with(|slot| *slot.borrow_mut() = Some(shell));
        (window, state.expect("the app state"))
    });
    cx.run_until_parked();
    (window, app, playback)
}

thread_local! {
    /// The window root of the corpus window this test opened. Each test runs on its own thread.
    static SHELL: std::cell::RefCell<Option<Entity<dory_gpui::shell::Shell>>> = const { std::cell::RefCell::new(None) };
}

/// Opens `title` the way a click on its search hit does, once `typed` has settled.
fn open_hit(
    title: &str,
    typed: &str,
    app: &Entity<AppState>,
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    open_id(id_of(title), typed, app, window, cx);
}

fn open_id(
    id: RecordingId,
    typed: &str,
    app: &Entity<AppState>,
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    search(app, typed, cx);
    app.update(cx, |app, cx| app.select(Selection::One(id), cx));
    settle(window, cx);
}

/// Draws frames until a reveal that is a few frames long has finished.
fn settle(window: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..8 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    cx.run_until_parked();
}

/// The summary's marks as the words they cover in the rendered text.
fn summary_marks(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<String> {
    let (rendered, ranges) = summary_ranges(window, cx);
    ranges
        .iter()
        .map(|r| rendered[r.clone()].to_owned())
        .collect()
}

/// The text the summary draws, and the ranges of it that are marked.
fn summary_ranges(
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
) -> (String, Vec<std::ops::Range<usize>>) {
    let _ = window;
    let shell = SHELL
        .with(|slot| slot.borrow().clone())
        .expect("the corpus window");
    let summary = shell
        .read_with(cx, |shell, cx| {
            shell.detail().map(|d| d.read(cx).summary().clone())
        })
        .expect("a detail pane");
    summary.read_with(cx, |text, cx| (text.rendered(cx), text.marks().to_vec()))
}

fn current(app: &Entity<AppState>, cx: &mut TestAppContext) -> Option<(usize, usize, Place)> {
    app.read_with(cx, |app, cx| {
        let session = app.session()?.read(cx);
        let found = session.found()?;
        Some((found.current(), found.len(), found.current_at().place))
    })
}

fn readout(window: AnyWindowHandle, cx: &mut TestAppContext) -> Option<String> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find("match-readout")
            .and_then(|node| node.label().map(str::to_owned))
    })
    .unwrap()
}

fn tab(app: &Entity<AppState>, cx: &mut TestAppContext) -> DetailTab {
    app.read_with(cx, |app, _| app.tab)
}

fn described(window: AnyWindowHandle, turn: usize, cx: &mut TestAppContext) -> Option<String> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .find(format!("turn-{turn}"))
            .description()
            .map(str::to_owned)
    })
    .unwrap()
}

/// Whether `id` is drawn inside the window's height.
fn in_view(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id.to_owned()).is_some_and(|element| {
            let at = element.bounds();
            at.top() >= px(0.) && at.bottom() <= window.viewport_size().height
        })
    })
    .unwrap()
}

fn step(window: AnyWindowHandle, forward: bool, cx: &mut TestAppContext) {
    if forward {
        cx.dispatch_action(window, FindNext);
    } else {
        cx.dispatch_action(window, FindPrevious);
    }
    settle(window, cx);
}

#[gpui_kit::test]
fn opening_a_hit_reveals_the_first_matching_turn(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    assert_eq!(
        tab(&app, cx),
        DetailTab::Transcript,
        "a transcript match opens the Transcript tab"
    );
    assert_eq!(current(&app, cx), Some((0, 5, Place::Turn(1))));
    assert_eq!(
        readout(window, cx).as_deref(),
        Some("1 of 5 matches"),
        "the tab row counts the matches"
    );
    assert!(in_view(window, "turn-1", cx), "the first matching turn");
    app.read_with(cx, |app, cx| {
        let turns = app.session().unwrap().read(cx).detail().unwrap();
        assert!(turns.turns[1].text.contains("roadmap"));
    });
}

#[gpui_kit::test]
fn opening_a_hit_in_a_long_transcript_scrolls_the_list_to_it(cx: &mut TestAppContext) {
    const { assert!(LONG_TURNS > PAGED_TURNS) };
    let (window, app, _) = open_corpus(cx);
    open_hit(LONG, "roadmap", &app, window, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Turn(100)));
    assert!(in_view(window, "turn-100", cx), "the match is in view");
    cx.update_window(window, |_, window, _| {
        assert!(
            window.try_find("turn-0").is_none(),
            "the list scrolled away from the top"
        );
    })
    .unwrap();

    step(window, true, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Turn(140)));
    assert!(in_view(window, "turn-140", cx), "the next match is in view");
}

#[gpui_kit::test]
fn marks_are_on_the_matching_turns_and_nowhere_else(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    let said: Vec<Option<String>> = (0..4).map(|ix| described(window, ix, cx)).collect();
    assert_eq!(
        said,
        [
            None,
            Some("Search match".to_owned()),
            None,
            Some("Search match".to_owned())
        ]
    );
    app.read_with(cx, |app, cx| {
        let session = app.session().unwrap().read(cx);
        let found = session.found().unwrap();
        let turns = session.detail().unwrap();
        let words: Vec<&str> = found
            .in_turn(3)
            .iter()
            .map(|m| &turns.turns[3].text[m.span.clone()])
            .collect();
        assert_eq!(words, ["Roadmap", "roadmap"], "every match in a turn");
        assert!(found.in_turn(0).is_empty() && found.in_turn(2).is_empty());
    });
}

#[gpui_kit::test]
fn find_next_switches_tabs_and_both_directions_wrap(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);

    let mut seen = Vec::new();
    for _ in 0..6 {
        step(window, true, cx);
        seen.push((readout(window, cx).unwrap(), tab(&app, cx)));
    }
    let transcript = DetailTab::Transcript;
    let summary = DetailTab::Summary;
    assert_eq!(
        seen,
        [
            ("2 of 5 matches".to_owned(), transcript),
            ("3 of 5 matches".to_owned(), transcript),
            ("4 of 5 matches".to_owned(), summary),
            ("5 of 5 matches".to_owned(), summary),
            ("1 of 5 matches".to_owned(), transcript),
            ("2 of 5 matches".to_owned(), transcript),
        ],
        "past the last transcript match the Summary tab opens, and the end wraps back to the Transcript"
    );
    assert_eq!(
        current(&app, cx).map(|c| c.2),
        Some(Place::Turn(3)),
        "after the wrap, the Transcript tab is back"
    );

    step(window, false, cx);
    step(window, false, cx);
    assert_eq!(readout(window, cx).as_deref(), Some("5 of 5 matches"));
    assert_eq!(
        tab(&app, cx),
        DetailTab::Summary,
        "previous from the first match wraps to the last"
    );
}

#[gpui_kit::test]
fn each_step_is_announced_with_its_speaker_and_time(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    let said = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.announced.clone());
    app.update(cx, |app, _| app.announced = None);
    step(window, true, cx);
    assert_eq!(
        said(cx).as_deref(),
        Some("Match 2 of 5, Speaker 3, 13 minutes")
    );
    step(window, true, cx);
    step(window, true, cx);
    assert_eq!(
        said(cx).as_deref(),
        Some("Match 4 of 5, in the summary"),
        "a summary match says where it is"
    );
}

#[gpui_kit::test]
fn return_on_the_current_match_plays_from_the_start_of_its_turn(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    step(window, true, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Turn(3)));
    cx.update_window(window, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    let (at, playing) = app.read_with(cx, |app, cx| {
        let player = app.session().unwrap().read(cx).player().read(cx);
        (player.position(), player.is_playing())
    });
    assert_eq!((at, playing), (Ms(780_000), true));
}

#[gpui_kit::test]
fn clearing_search_removes_marks(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    assert_eq!(described(window, 1, cx).as_deref(), Some("Search match"));
    assert!(readout(window, cx).is_some());

    search(&app, "", cx);
    settle(window, cx);
    assert_eq!(current(&app, cx), None, "no match list");
    assert_eq!(described(window, 1, cx), None, "the turn is unmarked");
    assert_eq!(readout(window, cx), None, "the counter is gone");
    assert_eq!(
        tab(&app, cx),
        DetailTab::Transcript,
        "the tab and the page stay where they were"
    );
    app.update(cx, |app, cx| {
        assert!(app.step_match(1, cx).is_none(), "nothing to step through")
    });
}

#[gpui_kit::test]
fn a_summary_only_match_opens_the_summary(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(SUMMARY_ONLY, "kilimanjaro", &app, window, cx);
    assert_eq!(tab(&app, cx), DetailTab::Summary);
    assert_eq!(current(&app, cx), Some((0, 1, Place::Summary)));
    assert_eq!(readout(window, cx).as_deref(), Some("1 of 1 match"));
    assert_eq!(
        summary_marks(window, cx),
        ["Kilimanjaro"],
        "the word is marked in the rendered Markdown"
    );
}

#[gpui_kit::test]
fn every_match_in_the_summary_is_marked_in_the_rendered_text(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    app.update(cx, |app, cx| app.set_tab(DetailTab::Summary, cx));
    settle(window, cx);
    assert_eq!(summary_marks(window, cx), ["Roadmap", "roadmap"]);
    search(&app, "", cx);
    settle(window, cx);
    assert!(
        summary_marks(window, cx).is_empty(),
        "clearing removes them"
    );
}

#[gpui_kit::test]
fn a_legacy_turn_without_times_is_marked_and_cannot_seek(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(LEGACY, "mentoring", &app, window, cx);
    assert_eq!(tab(&app, cx), DetailTab::Transcript);
    assert_eq!(current(&app, cx), Some((0, 1, Place::Turn(0))));
    assert!(in_view(window, "turn-0", cx));
    let said = described(window, 0, cx).unwrap();
    assert!(
        said.contains("Search match") && said.contains("No timestamp"),
        "{said}"
    );
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("turn-0-time").is_none(), "no timestamp");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_new_search_while_a_recording_is_open_marks_it_again(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(TALK, "roadmap", &app, window, cx);
    search(&app, "welcome", cx);
    settle(window, cx);
    assert_eq!(current(&app, cx), Some((0, 1, Place::Turn(0))));
    assert_eq!(described(window, 0, cx).as_deref(), Some("Search match"));
    assert_eq!(described(window, 1, cx), None);
    assert_eq!(readout(window, cx).as_deref(), Some("1 of 1 match"));
}

#[gpui_kit::test]
fn the_menu_steps_only_while_there_are_matches(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    app.read_with(cx, |app, cx| {
        assert!(!dory_gpui::menus::menu_state(app, cx).can_step);
    });
    open_hit(TALK, "roadmap", &app, window, cx);
    app.read_with(cx, |app, cx| {
        assert!(dory_gpui::menus::menu_state(app, cx).can_step);
    });
    search(&app, "", cx);
    app.read_with(cx, |app, cx| {
        assert!(!dory_gpui::menus::menu_state(app, cx).can_step);
    });
}

#[gpui_kit::test]
fn opening_a_hit_far_down_a_paged_transcript_scrolls_the_page_to_it(cx: &mut TestAppContext) {
    const { assert!(MID_TURNS <= PAGED_TURNS) };
    let (window, app, _) = open_corpus(cx);
    let top_of = |id: &str, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id.to_owned()).map(|e| e.bounds())
        })
        .unwrap()
    };
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of(MID)), cx);
        app.set_tab(DetailTab::Transcript, cx);
    });
    settle(window, cx);
    let unsearched = top_of("turn-65", cx).expect("laid out in the page").top();
    assert!(
        unsearched > px(900.),
        "without a search turn 65 sits below the fold: {unsearched:?}"
    );

    open_hit(MID, "roadmap", &app, window, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Turn(65)));
    let shown = top_of("turn-65", cx).unwrap();
    assert!(
        shown.top() >= px(52.) && shown.bottom() <= px(900.),
        "the page scrolled so the match is in view: {shown:?}"
    );

    step(window, true, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Turn(68)));
    let shown = top_of("turn-68", cx).unwrap();
    assert!(
        shown.top() >= px(52.) && shown.bottom() <= px(900.),
        "the next match is in view: {shown:?}"
    );

    step(window, true, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Turn(65)));
    let shown = top_of("turn-65", cx).unwrap();
    assert!(
        shown.top() >= px(52.) && shown.bottom() <= px(900.),
        "wrapping to the first match scrolls back up to it: {shown:?}"
    );
}

#[gpui_kit::test]
fn a_match_in_the_title_scrolls_the_page_back_to_the_top(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(MID, "ortega roadmap", &app, window, cx);
    assert_eq!(
        current(&app, cx),
        Some((2, 4, Place::Turn(68))),
        "a hit opens at the first match in the turn that holds both words"
    );
    assert!(
        !in_view(window, "title", cx),
        "the page is scrolled down to turn 68"
    );
    step(window, false, cx);
    step(window, false, cx);
    assert_eq!(current(&app, cx).map(|c| c.2), Some(Place::Title));
    assert!(
        in_view(window, "title", cx),
        "a match in the title scrolls the page back to the top"
    );
}

fn untitled_recording() -> Recording {
    Recording {
        id: untitled_id(),
        title: None,
        created_at: untitled_at(),
        language: LanguagePref::Auto,
        layout: ChannelLayout::MicSystem,
        take: None,
    }
}

fn untitled_id() -> RecordingId {
    RecordingId::generate(untitled_at(), 0)
}

fn untitled_at() -> Timestamp {
    Timestamp(fixtures::now().timestamp_millis() - 3_600_000)
}

/// A recording with no title, so the sidebar shows "Recording 12 Nov, 10:30". Its transcript says
/// "recording", and a speaker is named Novak, so "nov" finds it by that name alone.
fn untitled_changes() -> Vec<Change> {
    let id = untitled_id();
    vec![
        Change::RecordingCreated(untitled_recording()),
        Change::TranscriptReplaced {
            id: id.clone(),
            transcript: spoken_transcript(vec![segment(
                4,
                Channel::System,
                Some((0, 5_000)),
                "The recording starts at ten.",
            )]),
        },
        Change::SpeakerNamesReplaced {
            id,
            names: SpeakerNames {
                for_revision: Revision(1),
                names: [(SpeakerId(4), "Novak".to_owned())].into(),
            },
        },
    ]
}

#[test]
fn the_sidebar_marks_a_given_title_and_never_the_date_label() {
    let query = Query::parse("recording nov").unwrap();
    assert!(title_marks(&query, &untitled_recording()).is_empty());
    let given = Recording {
        title: Some("  Recording plan, Nov  ".into()),
        ..untitled_recording()
    };
    assert_eq!(
        title_marks(&query, &given),
        [0..9, 16..19],
        "bytes of the trimmed title the row shows"
    );
}

#[gpui_kit::test]
fn an_untitled_recording_has_no_title_match(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus_with(untitled_changes(), cx);
    open_id(untitled_id(), "recording", &app, window, cx);
    app.read_with(cx, |app, cx| {
        let found = app.session().unwrap().read(cx).found().unwrap();
        assert!(found.in_title().is_empty(), "the date label is not text");
        assert_eq!(found.len(), 1, "only the transcript's word counts");
        assert_eq!(found.current_at().place, Place::Turn(0));
    });
}

#[gpui_kit::test]
fn a_speaker_only_hit_on_an_untitled_recording_has_no_matches(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus_with(untitled_changes(), cx);
    open_id(untitled_id(), "nov", &app, window, cx);
    assert_eq!(current(&app, cx), None, "a name is not a match to step to");
    assert_eq!(readout(window, cx), None);
    app.read_with(cx, |app, cx| {
        assert!(!dory_gpui::menus::menu_state(app, cx).can_step);
    });
}

#[gpui_kit::test]
fn punctuation_only_text_says_no_results_once_it_settles(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    let said = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.announced.clone());
    app.update(cx, |app, cx| app.set_search("!!!", cx));
    cx.run_until_parked();
    assert_eq!(said(cx), None, "typing that has not settled says nothing");
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
    assert_eq!(said(cx).as_deref(), Some("No results"));
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("sidebar-empty").label(),
            Some("No recordings match “!!!”.")
        );
    })
    .unwrap();

    search(&app, "", cx);
    assert_eq!(said(cx), None, "clearing the field says nothing");
    search(&app, "?", cx);
    assert_eq!(said(cx).as_deref(), Some("No results"));
}

#[gpui_kit::test]
fn a_long_summary_is_counted_once_it_is_rendered_and_the_hit_still_opens_on_it(
    cx: &mut TestAppContext,
) {
    let (window, app, _) = open_corpus(cx);
    open_hit(MID, "zanzibar", &app, window, cx);
    assert_eq!(tab(&app, cx), DetailTab::Summary);
    assert_eq!(readout(window, cx).as_deref(), Some("1 of 2 matches"));
    app.read_with(cx, |app, cx| {
        let reveal = app.session().unwrap().read(cx).reveal();
        assert_eq!(
            reveal, 1,
            "the views were asked to show the first match once"
        );
    });
    assert_eq!(summary_marks(window, cx), ["Zanzibar", "Zanzibar"]);
}

#[gpui_kit::test]
fn a_link_address_in_the_summary_is_not_a_match(cx: &mut TestAppContext) {
    let (window, app, _) = open_corpus(cx);
    open_hit(LINKED, "roadmap", &app, window, cx);
    assert_eq!(tab(&app, cx), DetailTab::Summary);
    assert_eq!(readout(window, cx).as_deref(), Some("1 of 2 matches"));
    let (rendered, ranges) = summary_ranges(window, cx);
    assert_eq!(
        ranges
            .iter()
            .map(|r| &rendered[r.clone()])
            .collect::<Vec<_>>(),
        ["roadmap", "roadmap"],
        "the word in the sentence and the word in the next paragraph"
    );
    app.read_with(cx, |app, cx| {
        let found = app.session().unwrap().read(cx).found().unwrap();
        let counted: Vec<_> = found
            .in_summary()
            .0
            .iter()
            .map(|m| m.span.clone())
            .collect();
        assert_eq!(counted, ranges, "the count and the marks are over one text");
    });
    step(window, true, cx);
    assert_eq!(readout(window, cx).as_deref(), Some("2 of 2 matches"));
    app.read_with(cx, |app, cx| {
        let found = app.session().unwrap().read(cx).found().unwrap();
        assert_eq!(found.in_summary().1, Some(1), "the second mark");
    });
}
