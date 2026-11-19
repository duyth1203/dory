//! The window rig the integration tests share: a real `Dory` over the core's in-memory store,
//! seeded with the section 6 samples, driven through `TestAppContext`.

#![allow(dead_code)]

pub mod views;

use std::sync::Arc;

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{RecordingId, Timestamp};
use dory_core::ports::{Change, Store as _};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{FakeCapture, FakeTrash, ManualClock, MemTakes};
use dory_gpui::app::AppState;
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::prefs::Prefs;
use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size,
};

/// The sample whose summary has a translation into Spanish, for the checks that walk every view.
pub const TRANSLATED_SUMMARY: &str = "Customer interview: Ortega";

/// The samples as store changes: recordings, then their transcripts and summaries.
pub fn sample_changes() -> Vec<Change> {
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
            if sample.title == TRANSLATED_SUMMARY {
                changes.push(Change::SummaryTranslationWanted {
                    id: sample.id(),
                    target: dory_core::domain::LangTag::parse("es").unwrap(),
                });
                changes.push(Change::SummaryTranslated {
                    id: sample.id(),
                    translation: dory_core::domain::SummaryTranslation {
                        target: dory_core::domain::LangTag::parse("es").unwrap(),
                        produced_by: dory_core::models::ModelRef::parse("ollama/gemma4:e4b")
                            .unwrap(),
                        markdown: format!("## Puntos clave\n\n- {}\n", sample.title),
                    },
                });
            }
        }
    }
    changes
}

pub fn sample_store() -> Arc<MemStore> {
    let store = Arc::new(MemStore::new());
    store.apply(sample_changes()).expect("seed the samples");
    store
}

/// A fresh folder per test, so parallel tests never trash or restore each other's files.
fn scratch_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "dory-gpui-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create the scratch dir");
    dir
}

/// The store runs the same contract suite as SQLite, search included.
pub fn dory_on(store: Arc<MemStore>) -> Dory {
    dory_with_capture(store, FakeCapture::new())
}

/// As [`dory_on`], with the capture adapter a test sets up, such as one reporting permissions
/// macOS has not asked about yet.
pub fn dory_with_capture(store: Arc<MemStore>, capture: FakeCapture) -> Dory {
    dory_with(store, capture, Vec::new())
}

/// A speech model whose explainer says far more than any window can show, so a popover that
/// only fits short text fails the layout checks.
pub fn long_winded_model() -> Arc<dyn dory_core::ports::ModelProvider<dory_core::models::Stt>> {
    let mut entry = dory_core::testing::stt_entry(
        "sherpa/long-winded",
        dory_core::models::DiarizationSupport::None,
    );
    entry.display_name = "Long-winded speech model with a long name".into();
    entry.disk_bytes = Some(702_545_920);
    let sentence = |n: usize, what: &str| {
        format!(
            "{what} {n}: a sentence long enough to wrap across two lines of the explainer, as the real catalog's do"
        )
    };
    entry.about = dory_core::models::ModelAbout {
        summary: "A model whose explainer has more to say than fits on any screen, to check that the popover scrolls instead of running off the window.".into(),
        pros: (1..=8).map(|n| sentence(n, "Strength")).collect(),
        cons: (1..=8).map(|n| sentence(n, "Caveat")).collect(),
    };
    Arc::new(dory_core::testing::FakeSttProvider::new(
        "sherpa",
        vec![entry],
        Arc::new(dory_core::testing::FakeTranscriber::new()),
    ))
}

pub fn dory_with(
    store: Arc<MemStore>,
    capture: FakeCapture,
    stt: Vec<Arc<dyn dory_core::ports::ModelProvider<dory_core::models::Stt>>>,
) -> Dory {
    let dir = scratch_dir();
    Dory::open(
        Ports {
            store,
            takes: Arc::new(MemTakes::in_dir(&dir)),
            capture: Arc::new(capture),
            playback: Arc::new(FakePlayback::new()),
            trash: Arc::new(FakeTrash::new(&dir)),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt,
            llm: Vec::new(),
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        options(),
    )
    .expect("open Dory on the samples")
}

/// As [`dory_on`], with `model` as the one summary model, so a test can hold a job mid-stage.
pub fn dory_with_llm(
    store: Arc<MemStore>,
    model: Arc<dory_core::testing::FakeLanguageModel>,
) -> Dory {
    let dir = scratch_dir();
    Dory::open(
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
            llm: vec![Arc::new(dory_core::testing::FakeLlmProvider::new(
                "fake",
                vec![dory_core::testing::llm_entry("fake/llm")],
                model,
            ))],
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        options(),
    )
    .expect("open Dory with a summary model")
}

/// Dory's options for a UI test: probes answer on the asking thread, since GPUI's test scheduler
/// fails a test that is woken from a thread it does not run.
pub fn options() -> Options {
    Options {
        probes: dory_core::Probes::Caller,
        ..Options::default()
    }
}

/// The samples, past first run.
pub fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    open_with_prefs(cx, dory_on(sample_store()), past_first_run())
}

pub fn past_first_run() -> Prefs {
    Prefs {
        onboarding: Some(dory_gpui::onboarding::OnboardingStep::Done),
        ..Prefs::default()
    }
}

pub fn open_with(cx: &mut TestAppContext, dory: Dory) -> (AnyWindowHandle, Entity<AppState>) {
    open_with_prefs(cx, dory, past_first_run())
}

/// No UI prefs file yet, as on a first launch.
pub fn open_fresh(cx: &mut TestAppContext, dory: Dory) -> (AnyWindowHandle, Entity<AppState>) {
    open_with_prefs(cx, dory, Prefs::default())
}

pub fn open_with_prefs(
    cx: &mut TestAppContext,
    dory: Dory,
    prefs: Prefs,
) -> (AnyWindowHandle, Entity<AppState>) {
    cx.update(gpui_kit::init);
    let opened = cx.update(|cx| {
        let mut state = None;
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(prefs.window_size.0), px(prefs.window_size.1)),
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
        (window, state.expect("the app state"))
    });
    cx.run_until_parked();
    opened
}

pub fn id_of(title: &str) -> RecordingId {
    SAMPLES.iter().find(|s| s.title == title).unwrap().id()
}

/// A `Dory` whose fake device sends 20 steps of silence at 8 kHz, so a take records for real; the
/// manual clock holds the Undo window open.
pub fn recordable(store: Arc<MemStore>) -> Dory {
    let dir = scratch_dir();
    let capture = Arc::new(FakeCapture::new());
    capture.push_script(
        (0..20)
            .map(|_| dory_core::testing::Step::Silence { frames: 800 })
            .collect(),
    );
    Dory::open(
        Ports {
            store,
            takes: Arc::new(MemTakes::in_dir(&dir)),
            trash: Arc::new(FakeTrash::new(&dir)),
            capture,
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: Vec::new(),
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        Options {
            limits: dory_core::recorder::Limits {
                rate_hz: std::num::NonZeroU32::new(8_000).unwrap(),
                ..dory_core::recorder::Limits::default()
            },
            clock_poll: std::time::Duration::from_millis(2),
            level_every: std::time::Duration::from_millis(10),
            ..options()
        },
    )
    .expect("open a recordable Dory")
}

/// The app as `main` starts it: the state, the app-wide menus and Quit, then the main window.
pub fn launch(
    cx: &mut TestAppContext,
    dory: Dory,
    prefs: Prefs,
) -> (AnyWindowHandle, Entity<AppState>) {
    cx.update(gpui_kit::init);
    let opened = cx.update(|cx| {
        dory_gpui::menus::install(cx);
        let app = dory_gpui::new_state(dory, prefs, cx);
        dory_gpui::menus::watch(&app, cx);
        dory_gpui::quit::install(&app, cx);
        let window = dory_gpui::open_main_window(cx).expect("open the main window");
        (window, app)
    });
    cx.run_until_parked();
    opened
}

/// Polls until `ok`, for state that real threads (the recorder, the pipeline) bring about.
pub fn until(cx: &mut TestAppContext, what: &str, mut ok: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
