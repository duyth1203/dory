//! P6's Settings window, model rows, explainer, onboarding speech step, and the Set up summaries
//! sheet, driven through real windows over a real `Dory` with fake model providers. Probes and
//! downloads run on their own threads, so each wait polls until the window shows the state.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{LangSet, LangTag, Settings, Timestamp};
use dory_core::models::{
    Availability, Choice, DiarizationSupport, LangSupport, Llm, ModelAbout, ModelEntry, ModelId,
    ProviderId, Stt, Unavailable, UnavailableReason,
};
use dory_core::ports::{
    CancelToken, Change, EngineError, ModelProvider, ProvisionError, ProvisionSink, Store as _,
    StoreRead as _, Transcriber,
};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeSttProvider, FakeTranscriber, FakeTrash,
    ManualClock, MemTakes, llm_entry, stt_entry,
};
use dory_gpui::app::AppState;
use dory_gpui::{fixtures, models_view, set_up_summaries, settings_window};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _,
    Render, Role, ScrollDelta, TestAppContext, Window, point, px, test::TestQueryExt as _,
    test::TestWindowExt as _,
};

use common::{open_with, sample_store};

const PARAKEET: &str = "sherpa/parakeet";
const WHISPER: &str = "sherpa/whisper";
const HELD: &str = "held/big";
const GEMMA: &str = "ollama/gemma4:e4b";

fn tags(list: &[&str]) -> Vec<LangTag> {
    list.iter().map(|t| LangTag::parse(t).unwrap()).collect()
}

fn speech(id: &str, name: &str, languages: &[&str], recommended: bool) -> ModelEntry<Stt> {
    let mut entry = stt_entry(id, DiarizationSupport::None);
    entry.display_name = name.into();
    entry.caps.languages = LangSupport::Multilingual {
        auto_detect: true,
        tags: LangSet::Only(tags(languages)),
    };
    entry.recommended = recommended;
    entry.disk_bytes = Some(1_036_613_791);
    entry.about = ModelAbout {
        summary: format!("{name} turns speech into text."),
        pros: vec![format!("{name} is accurate")],
        cons: vec![format!("{name} is slow")],
    };
    entry
}

/// A provider whose one model downloads until the test releases it or the user cancels.
struct HeldProvider {
    release: AtomicBool,
    ready: AtomicBool,
}

impl ModelProvider<Stt> for HeldProvider {
    fn id(&self) -> ProviderId {
        ProviderId::parse("held").unwrap()
    }

    fn catalog(&self) -> Vec<ModelEntry<Stt>> {
        vec![speech(HELD, "Big model", &["en"], false)]
    }

    fn availability(&self, _: &ModelId<Stt>) -> Availability {
        if self.ready.load(Ordering::SeqCst) {
            Availability::Ready
        } else {
            Availability::NeedsProvision {
                bytes: Some(2_000_000_000),
            }
        }
    }

    fn provision(
        &self,
        _: &ModelId<Stt>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError> {
        progress.step("Downloading");
        progress.bytes(25, Some(100));
        loop {
            if cancel.is_cancelled() {
                return Err(ProvisionError::Cancelled);
            }
            if self.release.load(Ordering::SeqCst) {
                self.ready.store(true, Ordering::SeqCst);
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn open(
        &self,
        model: &ModelId<Stt>,
        _: &CancelToken,
    ) -> Result<Arc<dyn Transcriber>, EngineError> {
        Err(EngineError::ModelMissing(model.to_string()))
    }
}

struct Rig {
    store: Arc<MemStore>,
    llm: Arc<FakeLlmProvider>,
}

fn rig(meeting: &[&str]) -> Rig {
    let store = sample_store();
    let mut settings: Settings = store.settings().unwrap();
    settings.meeting_languages = tags(meeting);
    store
        .apply(vec![Change::SettingsReplaced(settings)])
        .unwrap();
    let mut gemma = llm_entry(GEMMA);
    gemma.display_name = "Gemma 4 E4B".into();
    gemma.disk_bytes = Some(9_461_576_343);
    gemma.recommended = true;
    Rig {
        store,
        llm: Arc::new(FakeLlmProvider::new(
            "ollama",
            vec![gemma],
            Arc::new(FakeLanguageModel::new("summary")),
        )),
    }
}

fn open(rig: &Rig, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    let dir = std::env::temp_dir().join(format!(
        "dory-settings-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let stt = Arc::new(FakeSttProvider::new(
        "sherpa",
        vec![
            speech(PARAKEET, "Parakeet", &["en", "fr", "de"], true),
            speech(WHISPER, "Whisper", &["en", "tl", "fr", "de"], false),
        ],
        Arc::new(FakeTranscriber::new()),
    ));
    let held = Arc::new(HeldProvider {
        release: AtomicBool::new(false),
        ready: AtomicBool::new(false),
    });
    let dory = Dory::open(
        Ports {
            store: rig.store.clone(),
            takes: Arc::new(MemTakes::in_dir(&dir)),
            trash: Arc::new(FakeTrash::new(&dir)),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: vec![stt, held],
            llm: vec![rig.llm.clone()],
            diar: Vec::new(),
            guard: None,
        },
        Options::default(),
    )
    .unwrap();
    cx.executor().allow_parking();
    open_with(cx, dory)
}

fn until(cx: &mut TestAppContext, what: &str, mut ok: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn render(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    render(window, cx);
    cx.update_window(window, |_, window, _| {
        window
            .try_find(ElementId::from(id.to_owned()))
            .map(|e| e.label().unwrap_or_default().to_owned())
    })
    .unwrap()
}

fn has(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    label(window, id, cx).is_some()
}

/// Scrolls the Settings panel until `id` is on screen, as a user would before clicking it.
fn reveal(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    for _ in 0..20 {
        render(window, cx);
        let (visible, scrollable) = cx
            .update_window(window, |_, window, _| {
                (
                    window.find(ElementId::from(id.to_owned())).visible(),
                    window.try_find("settings-panel").is_some(),
                )
            })
            .unwrap();
        if visible || !scrollable {
            return;
        }
        cx.update_window(window, |_, window, cx| {
            window.scroll(
                "settings-panel",
                ScrollDelta::Pixels(point(px(0.), px(-200.))),
                cx,
            )
        })
        .unwrap();
    }
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    reveal(window, id, cx);
    cx.update_window(window, |_, window, cx| window.click(id.to_owned(), cx))
        .unwrap();
    render(window, cx);
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .unwrap();
    render(window, cx);
}

fn focused(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    render(window, cx);
    cx.update_window(window, |_, window, _| {
        window.find(ElementId::from(id.to_owned())).focused() == Some(true)
    })
    .unwrap()
}

fn open_settings(
    app: &Entity<AppState>,
    main: AnyWindowHandle,
    tab: settings_window::Tab,
    cx: &mut TestAppContext,
) -> AnyWindowHandle {
    cx.update(|cx| settings_window::open(app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| *w != main)
        .expect("the Settings window opened");
    click(window, &tab.id(), cx);
    window
}

fn settings(rig: &Rig) -> Settings {
    rig.store.settings().unwrap()
}

#[gpui_kit::test]
fn rows_say_which_meeting_language_a_model_lacks(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Tab::Transcription, cx);
    let row = format!("model-{PARAKEET}");
    until(cx, "the speech rows", |cx| has(window, &row, cx));
    let parakeet = label(window, &row, cx).unwrap();
    assert!(
        parakeet.contains("Doesn't support Tagalog"),
        "the row itself names the language: {parakeet}"
    );
    let whisper = label(window, &format!("model-{WHISPER}"), cx).unwrap();
    assert!(!whisper.contains("Doesn't support"), "{whisper}");
    assert!(
        !has(window, &format!("choose-{PARAKEET}"), cx),
        "a model that cannot do the job is not offered"
    );
    assert!(has(window, &format!("choose-{WHISPER}"), cx));
    let auto = cx
        .update_window(window, |_, window, _| {
            let auto = window.find("choice-auto-speech");
            (auto.checked(), auto.description().map(str::to_owned))
        })
        .unwrap();
    assert_eq!(auto, (Some(true), Some("Uses Whisper.".to_owned())));
    assert_eq!(
        label(window, "meeting-languages-summary", cx).as_deref(),
        Some("Meetings are in Tagalog and English.")
    );
}

#[gpui_kit::test]
fn dropping_tagalog_makes_parakeet_fit_again(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Tab::Transcription, cx);
    until(cx, "the checkboxes", |cx| {
        has(window, "meeting-language-tl", cx)
    });
    click(window, "meeting-language-tl", cx);
    until(cx, "the saved languages", |_| {
        settings(&rig).meeting_languages == tags(&["en"])
    });
    until(cx, "the refit row", |cx| {
        label(window, &format!("model-{PARAKEET}"), cx).is_some_and(|l| !l.contains("Doesn't"))
    });
    assert!(focused(window, "meeting-language-tl", cx));
    press(window, "space", cx);
    until(cx, "Space checks it once", |_| {
        settings(&rig).meeting_languages == tags(&["en", "tl"])
    });
}

#[gpui_kit::test]
fn the_explainer_shows_the_catalog_text_and_opens_and_closes_by_keyboard(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Tab::Transcription, cx);
    let info = format!("about-{WHISPER}");
    let panel = format!("about-panel-{WHISPER}");
    until(cx, "the info button", |cx| has(window, &info, cx));
    assert_eq!(label(window, &info, cx).as_deref(), Some("About Whisper"));
    assert!(!has(window, &panel, cx));

    click(window, &info, cx);
    let opened = cx
        .update_window(window, |_, window, _| {
            let panel = window.find(ElementId::from(panel.clone()));
            let items: Vec<String> = window
                .find_all_by_role(Role::ListItem)
                .iter()
                .filter_map(|e| e.label().map(str::to_owned))
                .collect();
            (
                panel.role(),
                panel.label().map(str::to_owned),
                window.find(ElementId::from(info.clone())).expanded(),
                items,
            )
        })
        .unwrap();
    assert_eq!(opened.0, Some(Role::Group));
    assert_eq!(opened.1.as_deref(), Some("Whisper"));
    assert_eq!(opened.2, Some(true));
    assert!(opened.3.contains(&"Whisper is accurate".to_owned()));
    assert!(opened.3.contains(&"Whisper is slow".to_owned()));
    for (id, want) in [
        ("about-summary", "Whisper turns speech into text."),
        ("about-good", "Good at"),
        ("about-watch", "Watch out for"),
        ("about-size", "Download size: 1.0 GB"),
        (
            "about-languages",
            "Languages: Tagalog, English, French, and German",
        ),
    ] {
        assert_eq!(
            label(window, &format!("{id}-{WHISPER}"), cx).as_deref(),
            Some(want)
        );
    }
    assert!(
        focused(window, &info, cx),
        "a click focuses the info button"
    );
    press(window, "escape", cx);
    assert!(!has(window, &panel, cx), "Escape closes it");
    assert!(focused(window, &info, cx), "focus stays on the info button");
    press(window, "space", cx);
    assert!(has(window, &panel, cx), "Space opens it");
    press(window, "space", cx);
    assert!(!has(window, &panel, cx), "Space closes it again");
    press(window, "enter", cx);
    assert!(has(window, &panel, cx), "Return opens it");
}

#[gpui_kit::test]
fn choose_saves_the_pinned_model(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Tab::Transcription, cx);
    let choose = format!("choose-{WHISPER}");
    until(cx, "the Choose button", |cx| has(window, &choose, cx));
    click(window, &choose, cx);
    let whisper: ModelId<Stt> = WHISPER.parse().unwrap();
    until(cx, "the saved choice", |_| {
        settings(&rig).stt == Choice::Pinned(whisper.clone())
    });
    until(cx, "the row in use", |cx| {
        label(window, &format!("model-{WHISPER}"), cx).is_some_and(|l| l.ends_with("in use"))
    });
    assert!(!has(window, &choose, cx));
    click(window, "choice-auto-speech", cx);
    until(cx, "Automatic again", |_| {
        settings(&rig).stt == Choice::Auto
    });
}

#[gpui_kit::test]
fn a_download_shows_progress_and_cancel_stops_it(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Tab::Transcription, cx);
    let download = format!("download-{HELD}");
    until(cx, "the Download button", |cx| has(window, &download, cx));
    assert_eq!(
        label(window, &download, cx).as_deref(),
        Some("Download Big model, 2.0 GB")
    );
    click(window, &download, cx);
    let row = format!("model-{HELD}");
    until(cx, "the progress", |cx| {
        label(window, &row, cx).is_some_and(|l| l.contains("Downloading, 25%"))
    });
    assert!(has(window, &format!("progress-{HELD}"), cx));
    click(window, &format!("cancel-{HELD}"), cx);
    until(cx, "Download again after Cancel", |cx| {
        has(window, &download, cx)
    });
    assert!(label(window, &row, cx).unwrap().contains("Not downloaded"));
}

#[gpui_kit::test]
fn the_summaries_sheet_shows_the_steps_while_ollama_is_not_running(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let gemma: ModelId<Llm> = GEMMA.parse().unwrap();
    rig.llm.set_availability(
        &gemma,
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::ServerUnreachable {
                endpoint: "http://127.0.0.1:11434".into(),
            },
            fix: Some("Ollama is not running".into()),
        }),
    );
    let (main, _) = open(&rig, cx);
    cx.update_window(main, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
    until(cx, "the steps", |cx| has(main, "set-up-check-again", cx));
    for command in set_up_summaries::COMMANDS {
        assert!(has(main, &format!("copy-{command}"), cx));
    }
    assert!(!has(main, "set-up-download", cx));
    rig.llm.set_availability(&gemma, Availability::Ready);
    click(main, "set-up-check-again", cx);
    until(cx, "ready after Check again", |cx| {
        label(main, "set-up-status", cx).is_some_and(|l| l.starts_with("Summaries are ready"))
    });
}

#[gpui_kit::test]
fn the_summaries_sheet_downloads_a_missing_model_in_one_click(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let gemma: ModelId<Llm> = GEMMA.parse().unwrap();
    rig.llm.set_availability(
        &gemma,
        Availability::NeedsProvision {
            bytes: Some(9_461_576_343),
        },
    );
    let (main, _) = open(&rig, cx);
    cx.update_window(main, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
    until(cx, "the Download button", |cx| {
        has(main, "set-up-download", cx)
    });
    assert_eq!(
        label(main, "set-up-download", cx).as_deref(),
        Some("Download Gemma 4 E4B, 9.5 GB")
    );
    assert!(!has(main, "set-up-check-again", cx));
    click(main, "set-up-download", cx);
    until(cx, "ready after the download", |cx| {
        label(main, "set-up-status", cx).is_some_and(|l| l.starts_with("Summaries are ready"))
    });
    assert_eq!(rig.llm.provisions(), 1);
}

#[gpui_kit::test]
fn the_summaries_sheet_says_when_summaries_are_ready(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let (main, _) = open(&rig, cx);
    cx.update_window(main, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
    until(cx, "the ready line", |cx| {
        label(main, "set-up-status", cx).as_deref()
            == Some("Summaries are ready. Dory writes them with Gemma 4 E4B through Ollama.")
    });
    assert!(!has(main, "set-up-download", cx));
    assert!(!has(main, "set-up-check-again", cx));
}

struct Onboarding(Entity<AppState>);

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::div()
            .child(models_view::speech_step(&self.0, window, cx))
            .child(models_view::meeting_languages_picker(&self.0, window, cx))
    }
}

#[gpui_kit::test]
fn the_speech_step_offers_the_model_auto_picks_for_tagalog(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (_, app) = open(&rig, cx);
    let window = cx.update(|cx| {
        let app = app.clone();
        gpui_kit::open_window(Default::default(), cx, move |_, cx| {
            cx.new(|_| Onboarding(app))
        })
        .unwrap()
        .0
    });
    until(cx, "the picked model", |cx| {
        has(window, "speech-step-model", cx)
    });
    assert_eq!(
        label(window, "speech-step-model", cx).as_deref(),
        Some("Whisper, 1.0 GB")
    );
    assert_eq!(
        label(window, "meeting-languages-summary", cx).as_deref(),
        Some("Meetings are in Tagalog and English.")
    );
}
