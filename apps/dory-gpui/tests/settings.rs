//! P6's Settings window, model rows, explainer, onboarding speech step, and the Set up summaries
//! sheet, driven through real windows over a real `Dory` with fake model providers. Probes and
//! downloads run on their own threads, so each wait polls until the window shows the state.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{
    ChannelLayout, Goal, LangSet, LangTag, LanguagePref, Ms, Priority, Processing, Recording,
    RecordingId, Settings, Timestamp,
};
use dory_core::models::{
    Availability, Choice, DiarizationSupport, LangSupport, Llm, ModelAbout, ModelEntry, ModelId,
    ProviderId, Stt, Unavailable, UnavailableReason,
};
use dory_core::ports::{
    CancelToken, Change, Conditions, EngineError, ModelProvider, Power, ProvisionError,
    ProvisionSink, Store as _, StoreRead as _, Takes as _, Thermal, Transcriber,
};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeMachine, FakeSttProvider, FakeTranscriber,
    FakeTrash, ManualClock, MemTakes, llm_entry, stt_entry,
};
use dory_gpui::app::AppState;
use dory_gpui::{fixtures, models_view, set_up_summaries, settings_window};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, ElementId, Entity, InputEvent as _, IntoElement,
    ParentElement as _, Render, Role, ScrollDelta, TestAppContext, Window, point, px,
    test::TestQueryExt as _, test::TestWindowExt as _,
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
    entry.caps.window = Some(Ms(30_000));
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
    /// More summary providers after Ollama's, such as a built-in model.
    more_llm: Vec<Arc<dyn ModelProvider<Llm>>>,
    machine: Arc<FakeMachine>,
    transcriber: Arc<FakeTranscriber>,
    dir: std::path::PathBuf,
    takes: MemTakes,
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
    let dir = std::env::temp_dir().join(format!(
        "dory-settings-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    Rig {
        more_llm: Vec::new(),
        store,
        machine: Arc::new(FakeMachine::default()),
        transcriber: Arc::new(FakeTranscriber::new()),
        takes: MemTakes::in_dir(&dir),
        dir,
        llm: Arc::new(FakeLlmProvider::new(
            "ollama",
            vec![gemma],
            Arc::new(FakeLanguageModel::new("summary")),
        )),
    }
}

fn open(rig: &Rig, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    open_with_speech(rig, Vec::new(), cx)
}

/// Like `open`, with `more` speech providers after the two sherpa models and the held one.
fn open_with_speech(
    rig: &Rig,
    more: Vec<Arc<dyn ModelProvider<Stt>>>,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<AppState>) {
    let dir = &rig.dir;
    let stt = Arc::new(FakeSttProvider::new(
        "sherpa",
        vec![
            speech(PARAKEET, "Parakeet", &["en", "fr", "de"], true),
            speech(WHISPER, "Whisper", &["en", "tl", "fr", "de"], false),
        ],
        rig.transcriber.clone(),
    ));
    let held = Arc::new(HeldProvider {
        release: AtomicBool::new(false),
        ready: AtomicBool::new(false),
    });
    let dory = Dory::open(
        Ports {
            store: rig.store.clone(),
            takes: Arc::new(rig.takes.clone()),
            trash: Arc::new(FakeTrash::new(dir)),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: [stt as Arc<dyn ModelProvider<Stt>>, held]
                .into_iter()
                .chain(more)
                .collect(),
            llm: std::iter::once(rig.llm.clone() as Arc<dyn ModelProvider<Llm>>)
                .chain(rig.more_llm.iter().cloned())
                .collect(),
            diar: Vec::new(),
            machine: rig.machine.clone(),
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

fn description(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    render(window, cx);
    cx.update_window(window, |_, window, _| {
        window
            .try_find(ElementId::from(id.to_owned()))
            .and_then(|e| e.description().map(str::to_owned))
    })
    .unwrap()
}

fn value(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    render(window, cx);
    cx.update_window(window, |_, window, _| {
        window
            .try_find(ElementId::from(id.to_owned()))
            .and_then(|e| e.value().map(str::to_owned))
    })
    .unwrap()
}

fn checked(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<bool> {
    render(window, cx);
    cx.update_window(window, |_, window, _| {
        window
            .try_find(ElementId::from(id.to_owned()))
            .and_then(|e| e.checked())
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
        // Aim at the panel's left margin, clear of any control.
        cx.update_window(window, |_, window, cx| {
            let panel = window.find("settings-panel").bounds();
            let position = point(panel.left() + px(4.), panel.center().y);
            window.dispatch_event(
                gpui_kit::MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: window.modifiers(),
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                gpui_kit::ScrollEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                    modifiers: window.modifiers(),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
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
    pane: settings_window::Pane,
    cx: &mut TestAppContext,
) -> AnyWindowHandle {
    cx.update(|cx| settings_window::open(app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| *w != main)
        .expect("the Settings window opened");
    click(window, &pane.id(), cx);
    window
}

fn settings(rig: &Rig) -> Settings {
    rig.store.settings().unwrap()
}

#[gpui_kit::test]
fn rows_say_which_meeting_language_a_model_lacks(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    let row = format!("model-{PARAKEET}");
    until(cx, "the speech rows", |cx| has(window, &row, cx));
    let parakeet = description(window, &row, cx).unwrap();
    assert!(
        parakeet.contains("Doesn't support Tagalog"),
        "the row itself names the language: {parakeet}"
    );
    let whisper = description(window, &format!("model-{WHISPER}"), cx).unwrap();
    assert!(!whisper.contains("Doesn't support"), "{whisper}");
    click(window, &row, cx);
    render(window, cx);
    assert_eq!(
        settings(&rig).stt,
        Choice::Auto,
        "a model that cannot do the job cannot be chosen"
    );
    let auto = cx
        .update_window(window, |_, window, _| {
            let auto = window.find("choice-auto-speech");
            (auto.checked(), auto.description().map(str::to_owned))
        })
        .unwrap();
    assert_eq!(auto, (Some(true), Some("Uses Whisper.".to_owned())));
    assert_eq!(
        value(window, "meeting-languages", cx).as_deref(),
        Some("Tagalog, English")
    );
}

/// Apple Speech's shape: one row per language, each fixed to that language's locales.
fn apple_speech(rig: &Rig) -> Arc<dyn ModelProvider<Stt>> {
    let row = |language: &str, name: &str, locales: &[&str]| {
        let mut entry = stt_entry(
            &format!("apple/speech-{language}"),
            DiarizationSupport::None,
        );
        entry.display_name = format!("Apple Speech ({name})");
        entry.caps.languages = LangSupport::FixedLocale {
            tags: LangSet::Only(tags(locales)),
        };
        entry
    };
    let provider = FakeSttProvider::new(
        "apple",
        vec![
            row("de", "German", &["de-DE"]),
            row("en", "English", &["en-GB", "en-US"]),
            row("fr", "French", &["fr-FR"]),
        ],
        rig.transcriber.clone(),
    );
    provider.set_availability(
        &"apple/speech-de".parse().unwrap(),
        Availability::NeedsProvision { bytes: None },
    );
    Arc::new(provider)
}

#[gpui_kit::test]
fn apple_speech_shows_as_one_group_of_the_languages_that_fit(cx: &mut TestAppContext) {
    let rig = rig(&["en"]);
    let (main, app) = open_with_speech(&rig, vec![apple_speech(&rig)], cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    let english = "model-apple/speech-en";
    until(cx, "the Apple rows", |cx| has(window, english, cx));
    assert_eq!(
        label(window, &format!("heading-{english}"), cx).as_deref(),
        Some("Apple Speech")
    );
    assert_eq!(label(window, english, cx).as_deref(), Some("English"));
    assert!(description(window, english, cx).unwrap().contains("Ready"));
    for hidden in ["model-apple/speech-de", "model-apple/speech-fr"] {
        assert!(!has(window, hidden, cx), "{hidden} does not fit English");
    }
    assert!(has(window, &format!("model-{WHISPER}"), cx));
}

#[gpui_kit::test]
fn a_pinned_apple_language_stays_listed_when_it_no_longer_fits(cx: &mut TestAppContext) {
    let rig = rig(&["en"]);
    let mut pinned = settings(&rig);
    pinned.stt = Choice::Pinned("apple/speech-de".parse().unwrap());
    rig.store
        .apply(vec![Change::SettingsReplaced(pinned)])
        .unwrap();
    let (main, app) = open_with_speech(&rig, vec![apple_speech(&rig)], cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    let german = "model-apple/speech-de";
    until(cx, "the pinned row", |cx| has(window, german, cx));
    assert_eq!(
        label(window, &format!("heading-{german}"), cx).as_deref(),
        Some("Apple Speech"),
        "the first listed language carries the heading"
    );
    assert!(!has(window, "heading-model-apple/speech-en", cx));
    assert!(has(window, "model-apple/speech-en", cx));
    assert!(!has(window, "model-apple/speech-fr", cx));
}

#[gpui_kit::test]
fn dropping_tagalog_makes_parakeet_fit_again(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    until(cx, "the chips", |cx| has(window, "remove-language-tl", cx));
    assert_eq!(
        label(window, "remove-language-tl", cx).as_deref(),
        Some("Remove Tagalog")
    );
    click(window, "remove-language-tl", cx);
    until(cx, "the saved languages", |_| {
        settings(&rig).meeting_languages == tags(&["en"])
    });
    until(cx, "the refit row", |cx| {
        description(window, &format!("model-{PARAKEET}"), cx)
            .is_some_and(|l| !l.contains("Doesn't"))
    });
    assert_eq!(
        label(window, "remove-language-en", cx).as_deref(),
        Some("Remove English, unavailable"),
        "the last language stays"
    );
    assert_eq!(
        description(window, "remove-language-en", cx).as_deref(),
        Some("Keep at least one meeting language.")
    );
    click(window, "remove-language-en", cx);
    render(window, cx);
    assert_eq!(settings(&rig).meeting_languages, tags(&["en"]));
}

#[gpui_kit::test]
fn the_explainer_shows_the_catalog_text_and_opens_and_closes_by_keyboard(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
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
        ("about-disk", "On disk: 1.0 GB"),
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
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    let choose = format!("model-{WHISPER}");
    until(cx, "the Whisper radio", |cx| has(window, &choose, cx));
    click(window, &choose, cx);
    let whisper: ModelId<Stt> = WHISPER.parse().unwrap();
    until(cx, "the saved choice", |_| {
        settings(&rig).stt == Choice::Pinned(whisper.clone())
    });
    until(cx, "the row in use", |cx| {
        checked(window, &choose, cx) == Some(true)
    });
    assert_eq!(checked(window, "choice-auto-speech", cx), Some(false));
    click(window, "choice-auto-speech", cx);
    until(cx, "Automatic again", |_| {
        settings(&rig).stt == Choice::Auto
    });
}

#[gpui_kit::test]
fn a_download_shows_progress_and_cancel_stops_it(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    let download = format!("download-{HELD}");
    until(cx, "the Download button", |cx| has(window, &download, cx));
    assert_eq!(
        label(window, &download, cx).as_deref(),
        Some("Download Big model, 2.0 GB")
    );
    click(window, &download, cx);
    let row = format!("model-{HELD}");
    until(cx, "the progress", |cx| {
        description(window, &row, cx).is_some_and(|l| l.contains("Downloading, 25%"))
    });
    assert!(has(window, &format!("progress-{HELD}"), cx));
    click(window, &format!("cancel-{HELD}"), cx);
    until(cx, "Download again after Cancel", |cx| {
        has(window, &download, cx)
    });
    assert!(
        description(window, &row, cx)
            .unwrap()
            .contains("Not downloaded")
    );
}

#[gpui_kit::test]
fn the_summaries_sheet_starts_at_install_and_moves_on_when_ollama_is_found(
    cx: &mut TestAppContext,
) {
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
    until(cx, "step 1", |cx| has(main, "set-up-get-ollama", cx));
    assert_eq!(
        label(main, "set-up-step-2", cx).as_deref(),
        Some("Step 2, Download the summary model · Gemma 4 E4B · 9.5 GB"),
        "the catalog's model and size"
    );
    assert_eq!(
        label(main, "set-up-download", cx).as_deref(),
        Some("Download Model, available after Ollama is installed")
    );
    rig.llm.set_availability(&gemma, Availability::Ready);
    let models = cx.update(|cx| models_view::shared(cx)).expect("the models");
    models.update(cx, |m, cx| m.refresh(cx));
    until(cx, "ready once Ollama answers", |cx| {
        has(main, "set-up-ready", cx)
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
        label(main, "set-up-download", cx).as_deref() == Some("Download Model")
    });
    assert_eq!(
        label(main, "set-up-step-1", cx).as_deref(),
        Some("Step 1, Ollama is installed")
    );
    assert!(!has(main, "set-up-get-ollama", cx));
    click(main, "set-up-download", cx);
    until(cx, "ready after the download", |cx| {
        has(main, "set-up-ready", cx)
    });
    assert_eq!(rig.llm.provisions(), 1);
}

#[gpui_kit::test]
fn the_summaries_sheet_says_when_summaries_are_ready(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let (main, _) = open(&rig, cx);
    cx.update_window(main, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
    until(cx, "the ready state", |cx| has(main, "set-up-ready", cx));
    assert_eq!(
        label(main, "set-up-ready", cx).as_deref(),
        Some("Summaries are ready")
    );
    assert!(!has(main, "set-up-download", cx));
    assert!(!has(main, "set-up-subtitle", cx), "U3 has no subtitle");
    click(main, "set-up-done", cx);
    let open = cx
        .update_window(main, |_, window, cx| {
            dory_gpui::surface::has_dialog(window, cx)
        })
        .unwrap();
    assert!(!open, "Done closes it");
}

#[gpui_kit::test]
fn ollama_section_only_for_ollama_choices(cx: &mut TestAppContext) {
    let mut rig = rig(&["en"]);
    let gemma: ModelId<Llm> = GEMMA.parse().unwrap();
    rig.llm.set_availability(
        &gemma,
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::ServerUnreachable {
                endpoint: "http://127.0.0.1:11434".into(),
            },
            fix: None,
        }),
    );
    let mut system = llm_entry("system/summarizer");
    system.display_name = "Apple Intelligence".into();
    system.caps.languages = LangSet::Only(tags(&["en"]));
    system.rank = -1;
    system.built_in = true;
    rig.more_llm.push(Arc::new(FakeLlmProvider::new(
        "system",
        vec![system],
        Arc::new(FakeLanguageModel::new("summary")),
    )));
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Summaries, cx);
    until(cx, "the summary rows", |cx| {
        has(window, "model-system/summarizer", cx)
    });
    assert!(
        !has(window, "settings-ollama", cx),
        "Automatic uses the built-in model, so a stopped Ollama does not matter"
    );
    assert!(
        description(window, "model-system/summarizer", cx)
            .unwrap_or_default()
            .contains("Ready"),
        "the built-in row reads Ready"
    );
    click(window, &format!("model-{GEMMA}"), cx);
    until(cx, "the Ollama steps once Gemma is chosen", |cx| {
        has(window, "settings-ollama", cx)
    });
}

struct Onboarding(Entity<AppState>);

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::div()
            .child(models_view::speech_step(&self.0, window, cx))
            .child(dory_gpui::combobox::meeting_languages(&self.0, window, cx))
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
        Some("Whisper, 1.0 GB on disk")
    );
    assert_eq!(
        value(window, "meeting-languages", cx).as_deref(),
        Some("Tagalog, English")
    );
}

#[gpui_kit::test]
fn processing_saves_by_click_and_keyboard_and_speaks_each_effect(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    until(cx, "the Processing choices", |cx| {
        has(window, "processing-auto", cx)
    });
    for (id, name, effect) in [
        ("processing-auto", "Automatic", "Runs light on battery"),
        ("processing-fast", "Fast", "Uses up to 4 cores."),
        ("processing-light", "Light", "Uses 2 cores."),
    ] {
        assert_eq!(label(window, id, cx).as_deref(), Some(name));
        let spoken = description(window, id, cx).unwrap();
        assert!(spoken.contains(effect), "{id}: {spoken}");
    }
    assert_eq!(
        label(window, "processing", cx).as_deref(),
        Some("Processing"),
        "one radio group"
    );
    assert_eq!(
        checked(window, "processing-auto", cx),
        Some(true),
        "the default"
    );

    click(window, "processing-fast", cx);
    until(cx, "Fast saved", |_| {
        settings(&rig).processing == Processing::Fast
    });
    assert!(
        focused(window, "processing", cx),
        "the click puts focus on the group"
    );
    press(window, "down", cx);
    until(cx, "Light saved by Down", |_| {
        settings(&rig).processing == Processing::Light
    });
    until(cx, "Light shows as chosen", |cx| {
        checked(window, "processing-light", cx) == Some(true)
    });
    press(window, "down", cx);
    until(cx, "Down wraps to Automatic", |_| {
        settings(&rig).processing == Processing::Auto
    });
    press(window, "tab", cx);
    assert!(
        !focused(window, "processing", cx),
        "the group is one Tab stop"
    );
}

#[gpui_kit::test]
fn a_job_running_light_says_why_on_its_row(cx: &mut TestAppContext) {
    let rig = rig(&[]);
    rig.machine.set(Conditions {
        power: Power::Battery,
        low_power: false,
        thermal: Thermal::Nominal,
    });
    rig.transcriber.hold(true);
    let id = RecordingId::parse("on-battery").unwrap();
    let mut writer = rig.takes.begin(&id, ChannelLayout::MicOnly, 1_000).unwrap();
    writer.write(&vec![0.1; 90_000]).unwrap();
    writer.finish().unwrap();
    let take = rig.takes.seal(&id).unwrap();
    rig.store
        .apply(vec![Change::RecordingCreated(Recording {
            id: id.clone(),
            title: Some("Unplugged sync".into()),
            created_at: Timestamp(fixtures::now().timestamp_millis()),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicOnly,
            take: Some(take),
        })])
        .unwrap();
    rig.store
        .apply(vec![Change::JobEnqueued {
            id: id.clone(),
            goal: Goal::TRANSCRIBE,
            priority: Priority::Normal,
            at: Timestamp(fixtures::now().timestamp_millis()),
        }])
        .unwrap();
    let (main, _) = open(&rig, cx);
    let row = format!("row-{id}");
    // Events before the window subscribed are gone, so the next window is what it hears.
    rig.transcriber.hold_from(1);
    until(cx, "the light row", |cx| {
        label(main, &row, cx).is_some_and(|l| l.ends_with(", 33%, light"))
    });
    rig.transcriber.hold(false);
}

/// A library imported from the old app: one recording read in place from its WebM file.
struct Upgraded {
    original: std::path::PathBuf,
    trash: Arc<FakeTrash>,
    takes: Arc<MemTakes>,
    id: RecordingId,
}

fn upgraded(cx: &mut TestAppContext) -> (Upgraded, AnyWindowHandle, Entity<AppState>) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "dory-gpui-originals-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let old = dir.join("old-app").join("recordings");
    std::fs::create_dir_all(&old).unwrap();
    let original = old.join("meeting.webm");
    std::fs::write(&original, vec![7u8; 3_000_000]).unwrap();
    let takes = Arc::new(MemTakes::in_dir(dir.join("takes")));
    std::fs::create_dir_all(dir.join("trash")).unwrap();
    std::fs::create_dir_all(dir.join("takes")).unwrap();
    let trash = Arc::new(FakeTrash::new(dir.join("trash")));
    let id = RecordingId::parse("legacy-meeting").unwrap();
    let take = takes.link_legacy(&id, &original, Ms(60_000)).unwrap();
    let store = sample_store();
    store
        .apply(vec![Change::RecordingCreated(Recording {
            id: id.clone(),
            title: Some("Old meeting".into()),
            created_at: Timestamp(fixtures::now().timestamp_millis() - 86_400_000),
            language: LanguagePref::Auto,
            layout: ChannelLayout::Mixed,
            take: Some(take),
        })])
        .unwrap();
    let dory = Dory::open(
        Ports {
            store,
            takes: takes.clone(),
            trash: trash.clone(),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: Vec::new(),
            diar: Vec::new(),
            machine: Arc::new(FakeMachine::default()),
            guard: None,
        },
        Options::default(),
    )
    .unwrap();
    cx.executor().allow_parking();
    let (main, app) = open_with(cx, dory);
    (
        Upgraded {
            original,
            trash,
            takes,
            id,
        },
        main,
        app,
    )
}

#[gpui_kit::test]
fn storage_moves_the_original_webm_files_to_the_trash_and_undo_puts_them_back(
    cx: &mut TestAppContext,
) {
    let (rig, main, app) = upgraded(cx);
    let window = open_settings(&app, main, settings_window::Pane::Storage, cx);
    until(cx, "the originals are listed", |cx| {
        has(window, "trash-originals", cx)
    });
    let line = label(window, "originals-status", cx).unwrap();
    assert!(
        line.starts_with("1 original WebM file from the old Dory app, 3 MB."),
        "{line}"
    );
    assert!(rig.original.is_file(), "nothing moves until the user asks");

    click(window, "trash-originals", cx);
    until(cx, "the move", |cx| has(window, "undo-trash-originals", cx));
    assert!(!rig.original.exists(), "the WebM file left the old folder");
    assert_eq!(rig.trash.trashed_files().len(), 1, "it is in the Trash");
    assert!(
        rig.takes.legacy_original(&rig.id).unwrap().is_none(),
        "the recording now reads Dory's own copy"
    );
    assert_eq!(
        label(window, "originals-status", cx).as_deref(),
        Some("Moved 1 original WebM file to the Trash.")
    );

    click(window, "undo-trash-originals", cx);
    until(cx, "the undo", |cx| {
        !has(window, "undo-trash-originals", cx)
    });
    assert!(rig.original.is_file(), "Undo put the file back");
    assert!(rig.trash.trashed_files().is_empty());
    until(cx, "the section to hide", |cx| {
        !has(window, "settings-originals", cx)
    });
}

#[gpui_kit::test]
fn storage_offers_nothing_when_no_webm_files_are_left(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (main, app) = open_with(cx, common::dory_on(sample_store()));
    let window = open_settings(&app, main, settings_window::Pane::Storage, cx);
    for _ in 0..20 {
        render(window, cx);
    }
    assert!(!has(window, "settings-originals", cx));
    assert!(!has(window, "trash-originals", cx));
}

/// Every radio's circle and the top of its label, keyed by the radio's row.
fn radio_geometry(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<(String, f32, f32)> {
    render(window, cx);
    cx.update_window(window, |_, window, _| {
        let all = gpui_kit::base::test_support::snapshots(window);
        let mut out = Vec::new();
        for radio in all
            .iter()
            .filter(|e| e.role() == Some(Role::RadioButton) && e.visible())
        {
            let under = |name: &str| {
                all.iter().find(|e| {
                    e.path().starts_with(radio.path())
                        && matches!(e.path().last(), Some(ElementId::Name(n)) if n == name)
                })
            };
            let (Some(control), Some(text)) = (under("control"), under("label")) else {
                continue;
            };
            let id = format!("{:?}", radio.path().last());
            let x = f32::from(control.bounds().origin.x - radio.bounds().origin.x);
            let dy = f32::from(control.bounds().origin.y - text.bounds().origin.y);
            out.push((id, x, dy));
        }
        out
    })
    .unwrap()
}

#[gpui_kit::test]
fn every_radio_sits_at_the_same_place_in_recording_and_transcription(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Recording, cx);
    until(cx, "the microphones", |cx| {
        has(window, "microphone-automatic", cx)
    });
    let mut radios = radio_geometry(window, cx);
    click(window, &settings_window::Pane::Transcription.id(), cx);
    until(cx, "the speech rows", |cx| {
        has(window, &format!("model-{WHISPER}"), cx)
    });
    for _ in 0..10 {
        radios.extend(radio_geometry(window, cx));
        reveal(window, "processing-light", cx);
    }
    radios.sort_by(|a, b| a.0.cmp(&b.0));
    radios.dedup_by(|a, b| a.0 == b.0);
    let ids: Vec<&str> = radios.iter().map(|r| r.0.as_str()).collect();
    for want in [
        "microphone-automatic",
        "system-audio-separate",
        "choice-auto-speech",
        "processing-auto",
    ] {
        assert!(
            ids.iter().any(|id| id.contains(want)),
            "{want} missing: {ids:?}"
        );
    }
    let (_, x, dy) = radios[0].clone();
    assert_eq!(x, 14., "the circle sits 14 from the row's left edge");
    assert_eq!(dy, 1., "the circle is centered on the 18 pt first line");
    for (id, rx, rdy) in &radios {
        assert_eq!((*rx, *rdy), (x, dy), "{id} is out of line");
    }
}

fn focus_by_tab(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    for _ in 0..80 {
        if focused(window, id, cx) {
            return;
        }
        press(window, "tab", cx);
    }
    panic!("Tab never reached {id}");
}

fn chosen(rig: &Rig) -> Vec<String> {
    settings(rig)
        .meeting_languages
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[gpui_kit::test]
fn the_combobox_works_from_the_keyboard_alone(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    until(cx, "the field", |cx| has(window, "meeting-languages", cx));
    let field = cx
        .update_window(window, |_, window, _| {
            let f = window.find("meeting-languages");
            (f.role(), f.label().map(str::to_owned), f.expanded())
        })
        .unwrap();
    assert_eq!(
        field,
        (
            Some(Role::ComboBox),
            Some("Meeting languages".into()),
            Some(false)
        )
    );
    focus_by_tab(window, "meeting-languages", cx);
    press(window, "down", cx);
    assert!(
        has(window, "meeting-languages-popover", cx),
        "Down opens it"
    );
    assert!(
        focused(window, "language-search", cx),
        "the search has focus"
    );
    assert_eq!(
        cx.update_window(window, |_, window, _| window
            .find("language-options")
            .role())
            .unwrap(),
        Some(Role::ListBox)
    );

    cx.update_window(window, |_, window, cx| window.input("ge", cx))
        .unwrap();
    render(window, cx);
    assert_eq!(
        label(window, "language-match-count", cx).as_deref(),
        Some("4 matches")
    );
    for code in ["ka", "de", "nds", "gsw"] {
        assert!(
            has(window, &format!("language-option-{code}"), cx),
            "{code}"
        );
    }
    assert!(!has(window, "language-option-en", cx));
    let option = |code: &str, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            let o = window.find(format!("language-option-{code}"));
            (o.role(), o.selected(), o.description().map(str::to_owned))
        })
        .unwrap()
    };
    assert_eq!(
        option("de", cx),
        (
            Some(Role::ListBoxOption),
            Some(false),
            Some("Deutsch".into())
        )
    );
    press(window, "down", cx);
    press(window, "enter", cx);
    until(cx, "German added", |_| chosen(&rig) == ["tl", "en", "de"]);
    assert!(
        has(window, "meeting-languages-popover", cx),
        "Return keeps it open"
    );
    assert_eq!(
        label(window, "language-match-count", cx).as_deref(),
        Some(format!("{} matches", dory_gpui::languages::LANGUAGES.len()).as_str()),
        "the search clears"
    );
    assert_eq!(option("de", cx).1, Some(true), "German shows as chosen");

    cx.update_window(window, |_, window, cx| window.input("de", cx))
        .unwrap();
    render(window, cx);
    assert_eq!(
        label(window, "language-match-count", cx).as_deref(),
        Some("1 match")
    );
    press(window, "enter", cx);
    until(cx, "German removed by Return", |_| {
        chosen(&rig) == ["tl", "en"]
    });

    press(window, "backspace", cx);
    until(cx, "Delete removes the last chip", |_| {
        chosen(&rig) == ["tl"]
    });
    assert_eq!(
        label(window, "meeting-languages-status", cx).as_deref(),
        Some("English removed")
    );
    press(window, "backspace", cx);
    render(window, cx);
    assert_eq!(chosen(&rig), ["tl"], "the last chip stays");
    assert_eq!(
        label(window, "meeting-languages-status", cx).as_deref(),
        Some("Keep at least one meeting language.")
    );

    cx.update_window(window, |_, window, cx| window.input("xyz", cx))
        .unwrap();
    render(window, cx);
    assert_eq!(
        label(window, "language-no-match", cx).as_deref(),
        Some("No language matches “xyz”.")
    );
    press(window, "escape", cx);
    assert!(
        !has(window, "meeting-languages-popover", cx),
        "Esc closes it"
    );
    assert!(
        focused(window, "meeting-languages", cx),
        "focus returns to the field"
    );
    press(window, "down", cx);
    assert_eq!(
        label(window, "language-match-count", cx).as_deref(),
        Some(format!("{} matches", dory_gpui::languages::LANGUAGES.len()).as_str()),
        "it reopens with an empty search"
    );
    let rows = dory_gpui::languages::search("").rows;
    let shown = |code: &str, cx: &mut TestAppContext| {
        for _ in 0..3 {
            render(window, cx);
        }
        cx.update_window(window, |_, window, _| {
            window.find(format!("language-option-{code}")).visible()
        })
        .unwrap()
    };
    let (first, last) = (rows[0].code, rows[rows.len() - 1].code);
    assert!(!shown(last, cx), "the list is longer than it shows");
    press(window, "up", cx);
    assert!(
        shown(last, cx),
        "Up wraps to the last language and shows it"
    );
    press(window, "down", cx);
    assert!(shown(first, cx), "Down wraps to the first and scrolls back");
    press(window, "tab", cx);
    assert!(
        !has(window, "meeting-languages-popover", cx),
        "Tab closes it"
    );
    assert!(focused(window, "meeting-languages", cx));
}

/// Section 4.3: the popover sits 6 pt under the field and is as wide as it, so the chips stay
/// in view while the list is open.
#[gpui_kit::test]
fn the_languages_popover_opens_under_the_field(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    until(cx, "the field", |cx| has(window, "meeting-languages", cx));
    focus_by_tab(window, "meeting-languages", cx);
    press(window, "down", cx);
    until(cx, "the popover", |cx| {
        has(window, "meeting-languages-popover", cx)
    });
    let (field, popover) = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            (
                window.find("meeting-languages").bounds(),
                window.find("meeting-languages-popover").bounds(),
            )
        })
        .unwrap();
    let gap = f32::from(popover.top() - field.bottom());
    assert!(
        (gap - 6.).abs() < 0.5,
        "6 pt under the field, not {gap} (field {field:?}, popover {popover:?})"
    );
    assert!(
        f32::from((popover.left() - field.left()).abs()) < 0.5
            && f32::from((popover.size.width - field.size.width).abs()) < 0.5,
        "as wide as the field: field {field:?}, popover {popover:?}"
    );
}

#[gpui_kit::test]
fn language_follows_the_meeting_languages(cx: &mut TestAppContext) {
    let rig = rig(&["en", "de"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Transcription, cx);
    until(cx, "the Language row", |cx| {
        has(window, "setting-language", cx)
    });
    assert_eq!(
        label(window, "language-popup", cx).as_deref(),
        Some("Language, Automatic")
    );
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(
            Box::new(settings_window::ChooseLanguage {
                language: LanguagePref::Pinned(LangTag::parse("de").unwrap()),
            }),
            cx,
        )
    })
    .unwrap();
    until(cx, "German saved", |_| {
        settings(&rig).language == LanguagePref::Pinned(LangTag::parse("de").unwrap())
    });
    until(cx, "the popup shows German", |cx| {
        label(window, "language-popup", cx).as_deref() == Some("Language, German")
    });
    click(window, "remove-language-de", cx);
    until(cx, "Language back on Automatic", |_| {
        settings(&rig).language == LanguagePref::Auto
            && settings(&rig).meeting_languages == tags(&["en"])
    });
    assert_eq!(
        label(window, "language-note", cx).as_deref(),
        Some("German was removed from meeting languages, so Language is back to Automatic.")
    );
    until(cx, "the popup shows Automatic", |cx| {
        label(window, "language-popup", cx).as_deref() == Some("Language, Automatic")
    });
    let names: Vec<Option<String>> = dory_gpui::menus::retranscribe_choices(&tags(&["en", "de"]))
        .into_iter()
        .map(|c| c.map(|(name, _)| name))
        .collect();
    assert_eq!(
        names,
        [
            Some("Automatic".into()),
            None,
            Some("English".into()),
            Some("German".into())
        ],
        "Re-transcribe in… offers the same options"
    );
}

#[gpui_kit::test]
fn settings_reopens_on_the_pane_it_showed_last(cx: &mut TestAppContext) {
    let rig = rig(&["en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::Storage, cx);
    assert_eq!(
        app.read_with(cx, |app, _| app.prefs.settings_pane),
        settings_window::Pane::Storage,
        "saved in the UI prefs"
    );
    let size = cx
        .update_window(window, |_, window, _| window.viewport_size())
        .unwrap();
    assert_eq!(
        (f32::from(size.width), f32::from(size.height)),
        (settings_window::WIDTH, settings_window::HEIGHT)
    );
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| settings_window::open(&app, cx));
    cx.run_until_parked();
    let pane = cx.update(|cx| settings_window::current(cx).unwrap().read(cx).pane());
    assert_eq!(pane, settings_window::Pane::Storage);
    let reopened = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| *w != main)
        .unwrap();
    let selected = cx
        .update_window(reopened, |_, window, cx| {
            window.render_frame(cx);
            window.find("settings-pane-storage").selected()
        })
        .unwrap();
    assert_eq!(selected, Some(true));
}

#[gpui_kit::test]
fn search_jumps_to_the_setting_and_highlights_it(cx: &mut TestAppContext) {
    let rig = rig(&["en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::General, cx);
    click(window, "settings-search", cx);
    cx.update_window(window, |_, window, cx| window.input("process", cx))
        .unwrap();
    render(window, cx);
    assert!(has(window, "settings-hit-processing", cx));
    assert!(has(
        window,
        "settings-hit-process-recordings-automatically",
        cx
    ));
    assert!(
        !has(window, "settings-pane-storage", cx),
        "panes filter too"
    );
    click(window, "settings-hit-processing", cx);
    render(window, cx);
    let view = cx.update(|cx| settings_window::current(cx).unwrap());
    assert_eq!(
        view.read_with(cx, |v, _| (v.pane(), v.highlighted())),
        (settings_window::Pane::Transcription, Some("row-processing"))
    );
    render(window, cx);
    until(cx, "the row scrolled into view", |cx| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.find("processing-light").visible()
        })
        .unwrap()
    });
    cx.executor()
        .advance_clock(settings_window::HIGHLIGHT + Duration::from_millis(100));
    until(cx, "the highlight fades", |cx| {
        view.read_with(cx, |v, _| v.highlighted().is_none())
    });
}

/// Settings' search field is the main window's: a "Clear search" button at its end while it holds
/// text, which empties it, keeps focus in it, and brings every pane back.
#[gpui_kit::test]
fn the_clear_button_empties_the_settings_search(cx: &mut TestAppContext) {
    let rig = rig(&["en"]);
    let (main, app) = open(&rig, cx);
    let window = open_settings(&app, main, settings_window::Pane::General, cx);
    render(window, cx);
    assert!(
        !has(window, "settings-search-clear", cx),
        "no clear button while the field is empty"
    );
    click(window, "settings-search", cx);
    cx.update_window(window, |_, window, cx| window.input("process", cx))
        .unwrap();
    render(window, cx);
    assert!(!has(window, "settings-pane-storage", cx), "typing filters");
    let (label, role) = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let clear = window.find("settings-search-clear");
            (clear.label().map(str::to_owned), clear.role())
        })
        .unwrap();
    assert_eq!(
        (label.as_deref(), role),
        (Some("Clear search"), Some(gpui_kit::Role::Button))
    );
    click(window, "settings-search-clear", cx);
    render(window, cx);
    assert!(!has(window, "settings-search-clear", cx), "gone once empty");
    assert!(!has(window, "settings-hit-processing", cx), "no hits left");
    assert!(
        has(window, "settings-pane-storage", cx),
        "every pane is back"
    );
    let typing = cx
        .update_window(window, |_, window, cx| dory_gpui::a11y::typing(window, cx))
        .unwrap();
    assert!(typing, "focus stays in the field");
}

/// UX-31: a wheel anywhere over Settings content moves the page, so no inner area keeps it.
#[gpui_kit::test]
fn settings_content_has_one_scroll_area(cx: &mut TestAppContext) {
    let rig = rig(&["tl", "en", "de", "fr", "es"]);
    let (main, app) = open(&rig, cx);
    cx.update(|cx| dory_gpui::theme::set_display(|d| d.text_scale = 2.0, cx));
    let window = open_settings(&app, main, settings_window::Pane::General, cx);
    cx.update_window(window, |_, window, _| {
        window.resize(gpui_kit::size(
            px(settings_window::WIDTH),
            px(settings_window::MIN_HEIGHT),
        ))
    })
    .unwrap();
    let mut probed = 0;
    for pane in settings_window::Pane::ALL {
        click(window, &pane.id(), cx);
        until(cx, "the pane", |cx| {
            label(window, "settings-panel", cx).as_deref() == Some(pane.title())
        });
        for _ in 0..20 {
            render(window, cx);
        }
        let (panel, points) = cx
            .update_window(window, |_, window, _| {
                let panel = window.find("settings-panel");
                let path = panel.path().to_vec();
                let points: Vec<_> = gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .filter(|e| e.path().starts_with(&path) && e.path() != path && e.visible())
                    .map(|e| e.bounds().center())
                    .filter(|c| panel.bounds().contains(c))
                    .collect();
                (panel.bounds(), points)
            })
            .unwrap();
        let top = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                window.render_frame(cx);
                let path = window.find("settings-panel").path().to_vec();
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .filter(|e| e.path().starts_with(&path) && e.path() != path)
                    .map(|e| f32::from(e.bounds().origin.y))
                    .fold(f32::INFINITY, f32::min)
            })
            .unwrap()
        };
        let wheel = |at: gpui_kit::Point<gpui_kit::Pixels>, dy: f32, cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                window.dispatch_event(
                    gpui_kit::MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: window.modifiers(),
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    gpui_kit::ScrollEvent {
                        position: at,
                        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                        modifiers: window.modifiers(),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
            render(window, cx);
        };
        let margin = point(panel.left() + px(4.), panel.center().y);
        wheel(margin, 5_000., cx);
        let before = top(cx);
        wheel(margin, -60., cx);
        if top(cx) >= before {
            continue;
        }
        for at in points {
            wheel(margin, 5_000., cx);
            let before = top(cx);
            wheel(at, -60., cx);
            let after = top(cx);
            probed += 1;
            assert!(
                after < before,
                "{}: a wheel at {at:?} did not move the page",
                pane.title()
            );
        }
    }
    assert!(probed > 30, "only {probed} points probed");
}

fn status(window: AnyWindowHandle, cx: &mut TestAppContext) -> String {
    label(window, "assistant-connected", cx).unwrap_or_default()
}

#[gpui_kit::test]
fn the_assistants_pane_connects_claude_code_and_manages_it(cx: &mut TestAppContext) {
    use dory_core::domain::{ClientKey, Exposure, Writes};
    let rig = rig(&["en"]);
    let (main, app) = open(&rig, cx);
    let helper = rig.dir.join("dory-mcp");
    std::fs::write(&helper, b"").unwrap();
    app.update(cx, |app, _| app.set_assistant_helper(Some(helper.clone())));
    let window = open_settings(&app, main, settings_window::Pane::Assistants, cx);

    until(cx, "the access switch", |cx| {
        checked(window, "setting-assistants-enabled", cx) == Some(false)
    });
    assert!(
        !has(window, "connect-copy", cx),
        "nothing to connect while off"
    );
    click(window, "setting-assistants-enabled", cx);
    until(cx, "access to turn on", |_| {
        rig.store.assistant_access().unwrap().enabled
    });
    until(cx, "the connect buttons", |cx| {
        has(window, "connect-copy", cx)
    });

    click(window, "connect-copy", cx);
    until(cx, "the missing level to be named", |cx| {
        status(window, cx) == "Choose what it may read first."
    });
    assert!(rig.store.assistant_access().unwrap().clients.is_empty());

    click(window, "assistant-level-summaries", cx);
    click(window, "connect-copy", cx);
    until(cx, "the command to be copied", |cx| {
        status(window, cx).starts_with("Copied.")
    });
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        copied.starts_with("claude mcp add --env DORY_MCP_KEY=dk_"),
        "{copied}"
    );
    assert!(copied.contains(" dory -- "), "{copied}");
    assert!(copied.contains(&helper.display().to_string()), "{copied}");
    let key = |text: &str| {
        text.split_whitespace()
            .find_map(|w| w.strip_prefix("DORY_MCP_KEY="))
            .and_then(ClientKey::parse)
            .unwrap()
    };
    let first = key(&copied);
    let client = rig.store.assistant_client(&first).unwrap().unwrap();
    assert_eq!(
        (client.label.as_str(), client.exposure, client.writes),
        ("Claude Code", Exposure::Summaries, Writes::Never)
    );
    assert!(has(window, "assistant-Claude Code", cx));

    click(window, "connect-copy", cx);
    until(cx, "the key to be replaced", |cx| {
        status(window, cx).contains("old key no longer works")
    });
    assert_eq!(rig.store.assistant_client(&first).unwrap(), None);
    assert_eq!(rig.store.assistant_access().unwrap().clients.len(), 1);

    click(window, "assistant-edits-Claude Code", cx);
    until(cx, "edits to be allowed", |_| {
        rig.store.assistant_access().unwrap().clients[0].writes == Writes::Ask
    });

    click(window, "assistant-remove-Claude Code", cx);
    until(cx, "the assistant to go", |_| {
        rig.store.assistant_access().unwrap().clients.is_empty()
    });
    until(cx, "the empty list", |cx| {
        has(window, "setting-assistants-none", cx)
    });
}

#[gpui_kit::test]
fn connect_says_so_when_no_helper_ships_with_the_app(cx: &mut TestAppContext) {
    let rig = rig(&["en"]);
    let (main, app) = open(&rig, cx);
    app.update(cx, |app, _| app.set_assistant_helper(None));
    rig.store
        .apply(vec![Change::Assistant(
            dory_core::domain::AssistantChange::Enabled(true),
        )])
        .unwrap();
    let window = open_settings(&app, main, settings_window::Pane::Assistants, cx);
    until(cx, "the connect buttons", |cx| {
        has(window, "connect-copy", cx)
    });
    click(window, "assistant-level-titles", cx);
    click(window, "connect-copy", cx);
    until(cx, "the missing helper to be named", |cx| {
        status(window, cx).contains("no dory-mcp helper")
    });
    assert!(rig.store.assistant_access().unwrap().clients.is_empty());
}

#[gpui_kit::test]
fn the_assistant_app_menu_picks_codex_and_copies_its_config(cx: &mut TestAppContext) {
    use dory_gpui::app::Target;
    use dory_gpui::settings_window::ChooseAssistantApp;
    let rig = rig(&["en"]);
    let (main, app) = open(&rig, cx);
    let helper = rig.dir.join("dory-mcp");
    std::fs::write(&helper, b"").unwrap();
    app.update(cx, |app, _| app.set_assistant_helper(Some(helper.clone())));
    rig.store
        .apply(vec![Change::Assistant(
            dory_core::domain::AssistantChange::Enabled(true),
        )])
        .unwrap();
    let window = open_settings(&app, main, settings_window::Pane::Assistants, cx);
    until(cx, "the app menu", |cx| {
        label(window, "assistant-app", cx).as_deref() == Some("Assistant app, Claude Code")
    });
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(
            Box::new(ChooseAssistantApp {
                target: Target::Codex,
            }),
            cx,
        )
    })
    .unwrap();
    until(cx, "Codex to be chosen", |cx| {
        label(window, "assistant-app", cx).as_deref() == Some("Assistant app, Codex")
    });
    click(window, "assistant-level-transcripts", cx);
    assert!(
        has(window, "copy-icon-copy", cx),
        "a copy icon before copying"
    );
    click(window, "connect-copy", cx);
    until(cx, "the Codex config to be copied", |cx| {
        status(window, cx).contains("~/.codex/config.toml")
    });
    until(cx, "the check mark", |cx| {
        has(window, "copy-icon-check", cx)
    });
    assert_eq!(
        label(window, "connect-copy", cx).as_deref(),
        Some("Copied the setup for Codex")
    );
    cx.executor()
        .advance_clock(dory_gpui::app::COPIED_FOR + Duration::from_millis(50));
    until(cx, "the copy icon to come back", |cx| {
        has(window, "copy-icon-copy", cx) && !has(window, "copy-icon-check", cx)
    });
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        copied.starts_with("[mcp_servers.dory]\ncommand = \""),
        "{copied}"
    );
    assert!(copied.contains("env = { DORY_MCP_KEY = \"dk_"), "{copied}");
    let clients = rig.store.assistant_access().unwrap().clients;
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].label, "Codex");
}
