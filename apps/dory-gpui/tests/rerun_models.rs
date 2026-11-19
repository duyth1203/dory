//! Re-running a recording with a model picked for that one run, through a real window over a real
//! `Dory`: **Re-summarize with…** and **Translate Summary with…** list only the summary models
//! that can read the meeting, and a pick runs that model without changing Settings.

mod common;

use std::sync::Arc;

use dory_core::app::{Dory, Ports};
use dory_core::domain::{Goal, JobState, LangTag, Settings, Timestamp};
use dory_core::models::{Choice, LlmCaps};
use dory_core::ports::{Change, Store as _, StoreRead as _};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeTrash, ManualClock, MemTakes, llm_entry,
};
use dory_gpui::actions::{ResummarizeWith, TranslateSummaryTo, TranslateWith};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::fixtures;
use dory_gpui::library::Selection;
use dory_gpui::menus::{self, ModelItem};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Entity, MenuItem, TestAppContext,
    test::TestWindowExt as _,
};

use common::{id_of, open_with_prefs, past_first_run, sample_changes, until};

const TITLE: &str = "Design review";
const ANY: &str = "fake/llm";
const BIG: &str = "fake/big";
const EN_ONLY: &str = "fake/en-only";

fn tag(s: &str) -> LangTag {
    LangTag::parse(s).unwrap()
}

/// The samples, with Tagalog as the one meeting language, and three summary models: `fake/llm`
/// and `fake/big` read every language, `fake/en-only` reads only English.
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>, Arc<MemStore>) {
    let store = Arc::new(MemStore::new());
    let mut changes = sample_changes();
    changes.push(Change::SettingsReplaced(Settings {
        meeting_languages: vec![tag("tl")],
        meeting_languages_guessed: false,
        ..Settings::default()
    }));
    store.apply(changes).expect("seed the samples");
    let en_only = {
        let mut entry = llm_entry(EN_ONLY);
        entry.caps = LlmCaps {
            context_tokens: 8_192,
            languages: dory_core::domain::LangSet::Only(vec![tag("en")]),
        };
        entry.rank = 10;
        entry
    };
    let provider = |entry, reply: &'static str| {
        Arc::new(FakeLlmProvider::new(
            "fake",
            vec![entry],
            Arc::new(FakeLanguageModel::answering(move |system, user| {
                if system.contains("You translate") {
                    user.lines()
                        .map(|l| match l.split_once("] ") {
                            Some((n, text)) => format!("{n}] {reply}: {text}"),
                            None => l.to_owned(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    format!("## Key points\n\n- Written by {reply}.")
                }
            })),
        )) as Arc<dyn dory_core::ports::ModelProvider<dory_core::models::Llm>>
    };
    let dir = std::env::temp_dir().join(format!("dory-gpui-rerun-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dory = Dory::open(
        Ports {
            store: store.clone(),
            takes: Arc::new(MemTakes::in_dir(&dir)),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            trash: Arc::new(FakeTrash::new(&dir)),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: vec![
                provider(llm_entry(ANY), "any"),
                provider(llm_entry(BIG), "big"),
                provider(en_only, "english"),
            ],
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        common::options(),
    )
    .expect("open Dory");
    let (window, app) = open_with_prefs(cx, dory, past_first_run());
    (window, app, store)
}

fn select(app: &Entity<AppState>, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of(TITLE)), cx);
        app.set_tab(DetailTab::Summary, cx);
    });
    until(cx, "the summary models", |cx| {
        app.read_with(cx, |app, cx| {
            app.session()
                .is_some_and(|s| s.read(cx).rerun_models().summary.is_some())
        })
    });
}

fn labels<A>(items: &[Option<ModelItem<A>>]) -> Vec<Option<String>> {
    items
        .iter()
        .map(|item| item.as_ref().map(|i| i.label.clone()))
        .collect()
}

fn produced_by(store: &MemStore) -> String {
    store
        .summary(&id_of(TITLE))
        .unwrap()
        .unwrap()
        .produced_by
        .to_string()
}

#[gpui_kit::test]
fn re_summarize_with_lists_only_models_that_read_the_meeting_and_runs_the_pick(
    cx: &mut TestAppContext,
) {
    let (window, app, store) = open(cx);
    select(&app, cx);
    let state = app.read_with(cx, menus::menu_state);
    let offered = [
        Some(format!("Automatic ({BIG})")),
        None,
        Some(BIG.to_owned()),
        Some(ANY.to_owned()),
    ];
    assert_eq!(
        labels(&state.resummarize_with),
        offered,
        "Automatic first, then every model that reads Tagalog in Auto's order, and not {EN_ONLY}"
    );
    assert_eq!(
        state.resummarize_with[3].as_ref().unwrap().action,
        ResummarizeWith {
            llm: Some(ANY.parse().unwrap())
        }
    );
    assert_eq!(
        labels(&menus::translate_items(&state.resummarize_with)),
        offered,
        "Translate Summary with… offers the same models"
    );
    let built = menus::build(state);
    let recording = built.iter().find(|m| m.name == "Recording").unwrap();
    let native = recording
        .items
        .iter()
        .find_map(|item| match item {
            MenuItem::Submenu(sub) if sub.name == "Re-summarize with" => Some(&sub.items),
            _ => None,
        })
        .expect("Recording > Re-summarize with");
    assert!(
        matches!(&native[3], MenuItem::Action { name, disabled: false, .. } if name.as_ref() == ANY)
    );

    cx.dispatch_action(
        window,
        ResummarizeWith {
            llm: Some(ANY.parse().unwrap()),
        },
    );
    until(cx, "the picked summary", |_| produced_by(&store) == ANY);
    let job = store
        .jobs_for(&id_of(TITLE))
        .unwrap()
        .into_iter()
        .find(|j| j.goal == Goal::SUMMARIZE)
        .unwrap();
    assert_eq!(job.picks.llm, Some(ANY.parse().unwrap()));
    assert_eq!(job.state, JobState::Succeeded);
    assert_eq!(
        store.settings().unwrap().llm,
        Choice::Auto,
        "Settings never change"
    );
    assert!(
        store
            .summary(&id_of(TITLE))
            .unwrap()
            .unwrap()
            .markdown
            .contains("Written by any")
    );

    cx.dispatch_action(window, ResummarizeWith { llm: None });
    until(cx, "the Automatic summary", |_| produced_by(&store) == BIG);
}

#[gpui_kit::test]
fn translate_summary_with_opens_translate_for_that_model(cx: &mut TestAppContext) {
    let (window, app, store) = open(cx);
    select(&app, cx);
    cx.dispatch_action(
        window,
        TranslateWith {
            llm: Some(ANY.parse().unwrap()),
        },
    );
    cx.run_until_parked();
    let hint = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find(ElementId::from("language-hint".to_owned()))
                .map(|e| e.label().unwrap_or_default().to_owned())
        })
        .unwrap();
    assert_eq!(
        hint.as_deref(),
        Some(format!("With {ANY} · Return translates · Esc closes").as_str()),
        "the popover says which model translates"
    );

    cx.dispatch_action(
        window,
        TranslateSummaryTo {
            target: tag("fr"),
            llm: Some(ANY.parse().unwrap()),
        },
    );
    until(cx, "the translation", |_| {
        store
            .summary_translation(&id_of(TITLE))
            .unwrap()
            .is_some_and(|t| t.target == tag("fr"))
    });
    let translation = store.summary_translation(&id_of(TITLE)).unwrap().unwrap();
    assert_eq!(translation.produced_by.to_string(), ANY);
    assert!(
        translation.markdown.contains("any: "),
        "{}",
        translation.markdown
    );
}
