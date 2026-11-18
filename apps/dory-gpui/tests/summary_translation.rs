//! Translating a summary on demand, through a real window over a real `Dory`: the **Translate…**
//! menu and **Recording > Translate Summary…**, the Original / Translated switch and the note
//! under it, Copy and Export putting out what is shown, and the translation going away when the
//! summary is written again.

mod common;

use std::sync::Arc;

use dory_core::app::{Dory, Ports};
use dory_core::domain::{Goal, LangTag, Revision, Summary, SummaryTranslation, Timestamp};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store as _};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeTrash, ManualClock, MemTakes, llm_entry,
};
use dory_gpui::actions::{CopySummary, TranslateSummaryTo};
use dory_gpui::app::{AppState, DetailTab, SummaryVersion};
use dory_gpui::export::{ExportFormat, render};
use dory_gpui::library::Selection;
use dory_gpui::menus::{self, MenuState};
use dory_gpui::{fixtures, languages};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, MenuItem, Role, TestAppContext, Window,
    test::TestWindowExt as _,
};

use common::{id_of, open_with_prefs, past_first_run, sample_changes, until};

const TITLE: &str = "Design review";
const TAGALOG: &str = "## Mga punto\n\n- Ihatid sa **Biyernes**.\n- Ayos na ang login.\n";
const ENGLISH: &str = "## Key points\n\n- Ship on **Friday**.\n- Login is fixed.\n";

fn tag(s: &str) -> LangTag {
    LangTag::parse(s).unwrap()
}

fn summary(markdown: &str) -> Summary {
    Summary {
        markdown: markdown.into(),
        language: Some(tag("tl")),
        produced_by: ModelRef::parse("fake/llm").unwrap(),
        transcript_revision: Revision(1),
    }
}

/// The samples, with Design review's summary written in Tagalog and, when `translated`, already
/// translated into English by gemma4:e4b. The summary before it is kept, so a restore has one.
fn store(translated: bool) -> Arc<MemStore> {
    let store = Arc::new(MemStore::new());
    let id = id_of(TITLE);
    let mut changes = sample_changes();
    changes.push(Change::SummaryReplaced {
        id: id.clone(),
        summary: summary(TAGALOG),
    });
    if translated {
        changes.push(Change::SummaryTranslationWanted {
            id: id.clone(),
            target: tag("en"),
        });
        changes.push(Change::SummaryTranslated {
            id,
            translation: SummaryTranslation {
                target: tag("en"),
                produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
                markdown: ENGLISH.into(),
            },
        });
    }
    store.apply(changes).expect("seed the samples");
    store
}

/// A summary model that translates by prefixing each numbered line with `EN: `, and answers any
/// other request with a new summary.
fn translator() -> Arc<FakeLanguageModel> {
    Arc::new(FakeLanguageModel::answering(|system, user| {
        if !system.contains("You translate") {
            return "## Fresh summary\n\n- Written again.".to_owned();
        }
        user.lines()
            .map(|line| match line.split_once("] ") {
                Some((n, text)) => format!("{n}] EN: {text}"),
                None => line.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }))
}

fn open(
    store: Arc<MemStore>,
    model: Arc<FakeLanguageModel>,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<AppState>) {
    let dir = std::env::temp_dir().join(format!("dory-gpui-b5-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dory = Dory::open(
        Ports {
            store,
            takes: Arc::new(MemTakes::in_dir(&dir)),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            trash: Arc::new(FakeTrash::new(&dir)),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: vec![Arc::new(FakeLlmProvider::new(
                "fake",
                vec![llm_entry("fake/llm")],
                model,
            ))],
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        common::options(),
    )
    .expect("open Dory");
    open_with_prefs(cx, dory, past_first_run())
}

/// Selects Design review and shows its Summary tab.
fn show(app: &Entity<AppState>, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of(TITLE)), cx);
        app.set_tab(DetailTab::Summary, cx);
    });
    cx.run_until_parked();
}

fn with_window<R>(
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut Window, &mut gpui_kit::App) -> R,
) -> R {
    let out = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            f(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    out
}

/// What the summary tab shows and Copy and Export put out.
fn shown(app: &Entity<AppState>, cx: &mut TestAppContext) -> String {
    app.read_with(cx, |app, cx| {
        let session = app.session().unwrap().read(cx);
        let detail = session.detail().unwrap();
        detail
            .shown_summary(session.summary_version())
            .unwrap()
            .to_owned()
    })
}

fn clipboard(cx: &mut TestAppContext) -> String {
    cx.read_from_clipboard().and_then(|c| c.text()).unwrap()
}

fn items_of<'a>(menus: &'a [gpui_kit::Menu], menu: &str, submenu: &str) -> &'a [MenuItem] {
    let menu = menus.iter().find(|m| m.name == menu).unwrap();
    menu.items
        .iter()
        .find_map(|item| match item {
            MenuItem::Submenu(sub) if sub.name == submenu => Some(sub.items.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {submenu} in {menu:?}", menu = menu.name))
}

fn names_of(items: &[MenuItem]) -> Vec<Option<String>> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Action { name, .. } => Some(name.to_string()),
            MenuItem::Separator => None,
            other => panic!("unexpected item {:?}", std::mem::discriminant(other)),
        })
        .collect()
}

#[test]
fn the_recording_menu_lists_the_meeting_languages_then_every_other_language_but_the_summarys() {
    let state = MenuState {
        has_selection: true,
        has_summary: true,
        languages: vec![tag("tl"), tag("en")],
        summary_language: Some(tag("tl")),
        ..MenuState::default()
    };
    let built = menus::build(state.clone());
    let items = items_of(&built, "Recording", "Translate Summary…");
    let names = names_of(items);
    assert_eq!(names[0].as_deref(), Some("English"));
    assert_eq!(names[1], None, "a separator follows the meeting languages");
    assert!(
        !names.contains(&Some("Tagalog".to_owned())),
        "the summary's own language is not offered"
    );
    let others: Vec<&str> = names[2..].iter().flatten().map(String::as_str).collect();
    assert!(others.contains(&"French") && others.contains(&"German"));
    assert!(!others.contains(&"English"), "no language is offered twice");
    let mut sorted = others.clone();
    sorted.sort_by_key(|n| languages::fold(n));
    assert_eq!(others, sorted, "the rest is alphabetical");
    assert_eq!(others.len() + 1, languages::LANGUAGES.len() - 1);

    let MenuItem::Action {
        action, disabled, ..
    } = &items[0]
    else {
        panic!("English is an action");
    };
    assert!(!disabled);
    assert_eq!(
        action.as_any().downcast_ref::<TranslateSummaryTo>(),
        Some(&TranslateSummaryTo { target: tag("en") })
    );

    for (what, state) in [
        (
            "no summary",
            MenuState {
                has_summary: false,
                ..state.clone()
            },
        ),
        (
            "a translation under way",
            MenuState {
                translating_summary: true,
                ..state.clone()
            },
        ),
    ] {
        let built = menus::build(state);
        let items = items_of(&built, "Recording", "Translate Summary…");
        assert!(
            items.iter().all(|item| match item {
                MenuItem::Action { disabled, .. } => *disabled,
                _ => true,
            }),
            "{what}: every language is greyed out"
        );
    }
}

#[gpui_kit::test]
fn a_translated_summary_shows_the_switch_the_note_and_the_translation(cx: &mut TestAppContext) {
    let (window, app) = open(store(true), translator(), cx);
    show(&app, cx);
    assert_eq!(shown(&app, cx), ENGLISH);
    with_window(window, cx, |window, _| {
        let group = window.find("summary-text");
        assert_eq!(group.role(), Some(Role::RadioGroup));
        assert_eq!(window.find("summary-translated").checked(), Some(true));
        assert_eq!(window.find("summary-original").checked(), Some(false));
        assert_eq!(
            window.find("summary-translated").description(),
            Some("The summary written in English")
        );
        let note = window.find("summary-translation-note");
        assert_eq!(note.role(), Some(Role::Status));
        assert_eq!(
            note.label(),
            Some("Translated from Tagalog by gemma4:e4b"),
            "the note names the language it came from and the model"
        );
        assert!(window.try_find("translate").is_some(), "Translate… stays");
    });
}

#[gpui_kit::test]
fn the_switch_shows_the_original_and_copy_and_export_follow_it(cx: &mut TestAppContext) {
    let (window, app) = open(store(true), translator(), cx);
    show(&app, cx);
    let detail = |app: &Entity<AppState>, cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let session = app.session().unwrap().read(cx);
            (session.detail().unwrap(), session.shown())
        })
    };

    cx.dispatch_action(window, CopySummary);
    assert_eq!(clipboard(cx), ENGLISH, "Copy puts out the translation");
    let (d, now) = detail(&app, cx);
    let markdown = render(ExportFormat::Markdown, &d, now).unwrap();
    assert!(markdown.contains("- Ship on **Friday**."), "{markdown}");
    assert!(!markdown.contains("Ihatid"), "{markdown}");
    let plain = render(ExportFormat::Text, &d, now).unwrap();
    assert!(plain.contains("- Ship on Friday."), "{plain}");

    with_window(window, cx, |window, cx| {
        window.click("summary-original", cx)
    });
    assert_eq!(shown(&app, cx), TAGALOG);
    with_window(window, cx, |window, _| {
        assert_eq!(window.find("summary-original").checked(), Some(true));
        assert_eq!(window.find("summary-translated").checked(), Some(false));
        assert!(
            window.try_find("summary-translation-note").is_none(),
            "no note on the original"
        );
    });
    cx.dispatch_action(window, CopySummary);
    assert_eq!(clipboard(cx), TAGALOG, "Copy puts out the original");
    let (d, now) = detail(&app, cx);
    assert_eq!(now.summary, SummaryVersion::Original);
    let markdown = render(ExportFormat::Markdown, &d, now).unwrap();
    assert!(markdown.contains("- Ihatid sa **Biyernes**."), "{markdown}");
    assert!(!markdown.contains("Ship on"), "{markdown}");
    assert_eq!(d.summary.as_ref().unwrap().markdown, TAGALOG);

    with_window(window, cx, |window, cx| {
        window.click("summary-translated", cx)
    });
    cx.dispatch_action(window, CopySummary);
    assert_eq!(clipboard(cx), ENGLISH);
}

#[gpui_kit::test]
fn a_summary_with_no_translation_has_no_switch_and_offers_translate(cx: &mut TestAppContext) {
    let (window, app) = open(store(false), translator(), cx);
    show(&app, cx);
    assert_eq!(shown(&app, cx), TAGALOG);
    with_window(window, cx, |window, _| {
        assert!(window.try_find("summary-text").is_none());
        assert!(window.try_find("summary-translation-note").is_none());
        let translate = window.find("translate");
        assert_eq!(translate.label(), Some("Translate summary"));
    });
    app.update(cx, |app, cx| app.set_tab(DetailTab::Transcript, cx));
    cx.run_until_parked();
    with_window(window, cx, |window, _| {
        assert!(
            window.try_find("translate").is_none(),
            "Translate… belongs to the Summary tab"
        );
    });
}

#[gpui_kit::test]
fn choosing_a_language_says_it_is_translating_then_shows_the_translation(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let model = translator();
    let (window, app) = open(store(false), model.clone(), cx);
    show(&app, cx);
    model.hold_from(0);
    cx.dispatch_action(window, TranslateSummaryTo { target: tag("en") });
    until(cx, "the translation to start", |_| {
        !model.prompts().is_empty()
    });
    until(cx, "the tab to say so", |cx| {
        with_window(window, cx, |window, _| {
            window.try_find("summary-translating").is_some()
        })
    });
    with_window(window, cx, |window, _| {
        let status = window.find("summary-translating");
        assert_eq!(status.role(), Some(Role::Status));
        assert_eq!(
            status.label(),
            Some("Translating the summary into English…")
        );
        assert_eq!(
            window.find("translate").label(),
            Some("Translating the summary"),
            "the button offers nothing while it runs"
        );
        assert!(window.try_find("summary-text").is_none());
    });
    assert_eq!(shown(&app, cx), TAGALOG, "the summary stays readable");

    model.hold_from(usize::MAX);
    until(cx, "the switch to appear", |cx| {
        with_window(window, cx, |window, _| {
            window.try_find("summary-text").is_some()
        })
    });
    assert_eq!(
        shown(&app, cx),
        "## EN: Mga punto\n\n- EN: Ihatid sa **Biyernes**.\n- EN: Ayos na ang login.\n"
    );
    with_window(window, cx, |window, _| {
        assert!(window.try_find("summary-translating").is_none());
        assert_eq!(window.find("summary-translated").checked(), Some(true));
        assert_eq!(
            window.find("summary-translation-note").label(),
            Some("Translated from Tagalog by llm")
        );
    });
    with_window(window, cx, |window, cx| {
        window.click("summary-original", cx)
    });
    assert_eq!(shown(&app, cx), TAGALOG, "the original is unchanged");
}

#[gpui_kit::test]
fn writing_the_summary_again_takes_the_switch_away_and_brings_translate_back(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (window, app) = open(store(true), translator(), cx);
    show(&app, cx);
    with_window(window, cx, |window, _| {
        assert!(window.try_find("summary-text").is_some());
    });
    app.update(cx, |app, cx| {
        app.finish(id_of(TITLE), Goal::SUMMARIZE, false, cx)
    });
    until(cx, "the new summary", |cx| {
        app.read_with(cx, |app, cx| {
            app.session()
                .and_then(|s| s.read(cx).detail())
                .is_some_and(|d| {
                    d.summary
                        .as_ref()
                        .is_some_and(|s| s.markdown.contains("Fresh summary"))
                })
        })
    });
    assert_eq!(shown(&app, cx), "## Fresh summary\n\n- Written again.");
    with_window(window, cx, |window, _| {
        assert!(
            window.try_find("summary-text").is_none(),
            "the switch is gone"
        );
        assert!(window.try_find("summary-translation-note").is_none());
        assert_eq!(window.find("translate").label(), Some("Translate summary"));
    });
    app.read_with(cx, |app, cx| {
        let detail = app.session().unwrap().read(cx).detail().unwrap();
        assert_eq!(detail.summary_translation, None);
    });
}

/// The labels of the menu items drawn now, from the top down.
fn menu_labels(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<String> {
    with_window(window, cx, |window, _| {
        let mut items: Vec<_> = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|el| el.role() == Some(Role::MenuItem))
            .collect();
        items.sort_by(|a, b| a.bounds().top().partial_cmp(&b.bounds().top()).unwrap());
        items
            .iter()
            .filter_map(|el| el.label().map(str::to_owned))
            .collect()
    })
}

#[gpui_kit::test]
fn the_translate_menu_lists_the_meeting_languages_first_and_asks_for_the_one_picked(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let model = translator();
    let (window, app) = open(store(false), model.clone(), cx);
    app.update(cx, |app, cx| {
        app.update_settings(
            |settings| settings.meeting_languages = vec![tag("tl"), tag("fr")],
            cx,
        )
    });
    until(cx, "the meeting languages", |cx| {
        app.read_with(cx, |app, _| app.settings().meeting_languages.len() == 2)
    });
    show(&app, cx);
    with_window(window, cx, |window, cx| window.click("translate", cx));
    let labels = menu_labels(window, cx);
    assert_eq!(
        labels.first().map(String::as_str),
        Some("French"),
        "{labels:?}"
    );
    assert!(
        !labels.iter().any(|l| l == "Tagalog"),
        "the summary's language is left out"
    );
    assert!(labels.iter().any(|l| l == "English"));
    assert_eq!(labels.len(), languages::LANGUAGES.len() - 1);

    with_window(window, cx, |window, cx| {
        window.press("down", cx);
        window.press("enter", cx);
    });
    until(cx, "the French translation to be asked for", |_| {
        !model.prompts().is_empty()
    });
    assert!(
        model.prompts()[0].system.contains("BCP 47 tag fr"),
        "{}",
        model.prompts()[0].system
    );
}
