//! Set Up Summaries with a model built into the system beside Ollama's Gemma, through a real
//! window over a real `Dory` with fake providers. The built-in row's id is deliberately not
//! Apple's, so the dialog can only find it by `ModelEntry::built_in`.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{LangSet, LangTag, Settings, Timestamp};
use dory_core::models::{Availability, Llm, ModelId, Unavailable, UnavailableReason};
use dory_core::ports::{Change, ModelProvider, Store as _, StoreRead as _};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeMachine, FakeTrash, ManualClock, MemTakes,
    llm_entry,
};
use dory_gpui::{fixtures, models_view, set_up_summaries};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, TestAppContext, test::TestWindowExt as _,
};

use common::{open_with, sample_store};

const GEMMA: &str = "ollama/gemma4:e4b";
const SYSTEM: &str = "system/summarizer";
const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Siri-Settings.extension";

fn tags(list: &[&str]) -> Vec<LangTag> {
    list.iter().map(|t| LangTag::parse(t).unwrap()).collect()
}

struct Rig {
    ollama: Arc<FakeLlmProvider>,
    system: Arc<FakeLlmProvider>,
}

impl Rig {
    fn gemma(&self, availability: Availability) {
        let id: ModelId<Llm> = GEMMA.parse().unwrap();
        self.ollama.set_availability(&id, availability);
    }

    fn system(&self, availability: Availability) {
        let id: ModelId<Llm> = SYSTEM.parse().unwrap();
        self.system.set_availability(&id, availability);
    }
}

fn unavailable(reason: UnavailableReason) -> Availability {
    Availability::Unavailable(Unavailable { reason, fix: None })
}

/// Gemma through a stopped Ollama, and a built-in model for English and German that is Ready.
fn open(meeting: &[&str], cx: &mut TestAppContext) -> (Rig, AnyWindowHandle) {
    open_reading(meeting, &["en", "de"], cx)
}

/// As `open`, with a built-in model that names `languages`, none when its languages were never
/// read.
fn open_reading(
    meeting: &[&str],
    languages: &[&str],
    cx: &mut TestAppContext,
) -> (Rig, AnyWindowHandle) {
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
    gemma.rank = 1;
    let mut system = llm_entry(SYSTEM);
    system.display_name = "Apple Intelligence".into();
    system.caps.languages = LangSet::Only(tags(languages));
    system.rank = -1;
    system.built_in = true;
    let model = Arc::new(FakeLanguageModel::new("summary"));
    let rig = Rig {
        ollama: Arc::new(FakeLlmProvider::new("ollama", vec![gemma], model.clone())),
        system: Arc::new(FakeLlmProvider::new("system", vec![system], model)),
    };
    rig.gemma(unavailable(UnavailableReason::ServerUnreachable {
        endpoint: "http://127.0.0.1:11434".into(),
    }));
    rig.system(Availability::Ready);
    let dir = std::env::temp_dir().join(format!(
        "dory-set-up-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dory = Dory::open(
        Ports {
            store,
            takes: Arc::new(MemTakes::in_dir(&dir)),
            trash: Arc::new(FakeTrash::new(&dir)),
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: vec![
                rig.ollama.clone() as Arc<dyn ModelProvider<Llm>>,
                rig.system.clone(),
            ],
            diar: Vec::new(),
            machine: Arc::new(FakeMachine::default()),
            guard: None,
        },
        Options::default(),
    )
    .unwrap();
    cx.executor().allow_parking();
    let (main, _) = open_with(cx, dory);
    (rig, main)
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

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(ElementId::from(id.to_owned()))
            .map(|e| e.label().unwrap_or_default().to_owned())
    })
    .unwrap()
}

fn has(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    label(window, id, cx).is_some()
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.click(id.to_owned(), cx))
        .unwrap();
    cx.run_until_parked();
}

/// Every label in the window's frame, for wording that must not appear at all.
fn labels(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<String> {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::base::test_support::snapshots(window)
            .iter()
            .filter_map(|e| e.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}

fn open_sheet(main: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(main, |_, window, cx| set_up_summaries::sheet(window, cx))
        .unwrap();
}

fn refresh(cx: &mut TestAppContext) {
    let models = cx.update(|cx| models_view::shared(cx)).expect("the models");
    models.update(cx, |m, cx| m.refresh(cx));
}

fn system_offer(cx: &mut TestAppContext) -> Option<set_up_summaries::System> {
    let models = cx.update(|cx| models_view::shared(cx)).expect("the models");
    models.read_with(cx, |m, _| set_up_summaries::system(m))
}

#[gpui_kit::test]
fn apple_ready_needs_no_setup(cx: &mut TestAppContext) {
    let (rig, main) = open(&["en"], cx);
    open_sheet(main, cx);
    until(cx, "the System path", |cx| has(main, "set-up-system", cx));
    assert_eq!(
        label(main, "set-up-system", cx).as_deref(),
        Some("Summaries use Apple Intelligence on this Mac. Nothing to install.")
    );
    assert!(!has(main, "set-up-get-ollama", cx), "no install step");
    assert_eq!(
        label(main, "set-up-use-gemma", cx).as_deref(),
        Some("Use Gemma for long meetings")
    );
    click(main, "set-up-use-gemma", cx);
    assert_eq!(
        label(main, "set-up-step-1", cx).as_deref(),
        Some("Step 1, Install Ollama"),
        "Gemma's path is today's two Ollama steps"
    );
    rig.gemma(Availability::NeedsProvision {
        bytes: Some(9_461_576_343),
    });
    refresh(cx);
    until(cx, "the Download step", |cx| {
        label(main, "set-up-download", cx).as_deref() == Some("Download Model")
    });
}

#[gpui_kit::test]
fn apple_off_offers_both_paths(cx: &mut TestAppContext) {
    let (rig, main) = open(&["en"], cx);
    rig.system(unavailable(UnavailableReason::TurnedOff {
        detail: "Apple Intelligence is turned off".into(),
        settings_url: SETTINGS_URL.into(),
    }));
    open_sheet(main, cx);
    until(cx, "Turn on", |cx| has(main, "set-up-open-settings", cx));
    assert_eq!(
        label(main, "set-up-system", cx).as_deref(),
        Some("Step 1, Turn on Apple Intelligence")
    );
    assert_eq!(
        label(main, "set-up-open-settings", cx).as_deref(),
        Some("Open System Settings…")
    );
    assert_eq!(
        label(main, "set-up-use-ollama", cx).as_deref(),
        Some("Use Ollama Instead"),
        "the Ollama path beside it"
    );
    click(main, "set-up-open-settings", cx);
    assert_eq!(cx.opened_url().as_deref(), Some(SETTINGS_URL));
    rig.system(Availability::Ready);
    refresh(cx);
    until(cx, "the System path once it is on", |cx| {
        label(main, "set-up-system", cx).is_some_and(|l| l.starts_with("Summaries use"))
    });
}

#[gpui_kit::test]
fn ineligible_shows_the_ollama_flow(cx: &mut TestAppContext) {
    let (rig, main) = open(&["en"], cx);
    rig.system(unavailable(UnavailableReason::NotEligible {
        detail: "This Mac can't run Apple Intelligence".into(),
    }));
    open_sheet(main, cx);
    until(cx, "step 1", |cx| has(main, "set-up-get-ollama", cx));
    assert_eq!(
        label(main, "set-up-step-1", cx).as_deref(),
        Some("Step 1, Install Ollama")
    );
    let apple: Vec<String> = labels(main, cx)
        .into_iter()
        .filter(|l| l.contains("Apple"))
        .collect();
    assert!(apple.is_empty(), "{apple:?}");
    assert_eq!(system_offer(cx), None, "no Apple sentence either");
}

#[gpui_kit::test]
fn preparing_says_so_and_offers_no_dead_button(cx: &mut TestAppContext) {
    let (rig, main) = open(&["en"], cx);
    rig.system(unavailable(UnavailableReason::Preparing {
        detail: "Apple is getting the model ready".into(),
    }));
    open_sheet(main, cx);
    until(cx, "preparing", |cx| has(main, "set-up-system", cx));
    assert_eq!(
        label(main, "set-up-system", cx).as_deref(),
        Some("Apple Intelligence is getting ready")
    );
    assert!(!has(main, "set-up-open-settings", cx));
    assert!(has(main, "set-up-use-ollama", cx));
}

#[gpui_kit::test]
fn unknown_languages_offer_the_language_picker(cx: &mut TestAppContext) {
    let (_rig, main) = open(&[], cx);
    open_sheet(main, cx);
    until(cx, "the languages step", |cx| {
        label(main, "set-up-system", cx).as_deref() == Some("Step 1, Choose your meeting languages")
    });
    let picker: Vec<String> = labels(main, cx)
        .into_iter()
        .filter(|l| l.contains("language"))
        .collect();
    assert!(
        picker.iter().any(|l| l.contains("Meeting languages")),
        "the meeting-languages picker is in the dialog: {picker:?}"
    );
    assert!(has(main, "set-up-use-ollama", cx));
    let models = cx.update(|cx| models_view::shared(cx)).unwrap();
    models.update(cx, |m, cx| {
        m.toggle_language(LangTag::parse("en").unwrap(), cx);
    });
    until(cx, "the System path once English is chosen", |cx| {
        label(main, "set-up-system", cx).as_deref()
            == Some("Summaries use Apple Intelligence on this Mac. Nothing to install.")
    });
}

#[gpui_kit::test]
fn an_unsupported_language_is_named_over_the_ollama_flow(cx: &mut TestAppContext) {
    let (_rig, main) = open(&["en", "tl"], cx);
    open_sheet(main, cx);
    until(cx, "step 1", |cx| has(main, "set-up-get-ollama", cx));
    let note = set_up_summaries::unsupported_note(system_offer(cx).as_ref());
    assert_eq!(
        note.as_deref(),
        Some("Apple Intelligence doesn't support Tagalog.")
    );
}

#[gpui_kit::test]
fn a_language_apple_lacks_beats_turning_it_on(cx: &mut TestAppContext) {
    let (rig, main) = open(&["en", "tl"], cx);
    rig.system(unavailable(UnavailableReason::TurnedOff {
        detail: "Apple Intelligence is turned off".into(),
        settings_url: SETTINGS_URL.into(),
    }));
    open_sheet(main, cx);
    until(cx, "step 1", |cx| has(main, "set-up-get-ollama", cx));
    assert!(
        !has(main, "set-up-open-settings", cx),
        "turning on Apple Intelligence would not summarize Tagalog"
    );
    let note = set_up_summaries::unsupported_note(system_offer(cx).as_ref());
    assert_eq!(
        note.as_deref(),
        Some("Apple Intelligence doesn't support Tagalog.")
    );
}

#[gpui_kit::test]
fn apple_off_with_languages_unread_still_offers_turning_it_on(cx: &mut TestAppContext) {
    let (rig, main) = open_reading(&["en"], &[], cx);
    rig.system(unavailable(UnavailableReason::TurnedOff {
        detail: "Apple Intelligence is turned off".into(),
        settings_url: SETTINGS_URL.into(),
    }));
    open_sheet(main, cx);
    until(cx, "Turn on", |cx| has(main, "set-up-open-settings", cx));
    assert_eq!(
        label(main, "set-up-system", cx).as_deref(),
        Some("Step 1, Turn on Apple Intelligence")
    );
}
