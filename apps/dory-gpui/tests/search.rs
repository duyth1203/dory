//! UX-32: the sidebar while the search field has text. Two sections, "Titles" and "In
//! conversations", each a heading; a snippet and a VoiceOver sentence for every conversation
//! hit; one spoken result count when a search settles; the line that names a correction; and the
//! empty state. `TestAppContext` never starts AccessKit, so the tree is read from the elements
//! each frame registers with Kit's test support.

mod common;

use dory_core::search::{Correction, Excerpt, Section, Snippet, Source};
use dory_gpui::app::{AppState, SEARCH_DEBOUNCE};
use dory_gpui::library::{
    StatusFilter, corrected_words, match_words, results_announcement, section_label,
};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Entity, Role, TestAppContext,
    test::TestWindowExt as _,
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
