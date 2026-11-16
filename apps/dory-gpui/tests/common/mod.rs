//! The window rig the integration tests share: a real `Dory` over the core's in-memory store,
//! seeded with the section 6 samples, driven through `TestAppContext`.

#![allow(dead_code)]

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
            llm: Vec::new(),
            diar: Vec::new(),
            guard: None,
        },
        Options::default(),
    )
    .expect("open Dory on the samples")
}

pub fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    open_with(cx, dory_on(sample_store()))
}

pub fn open_with(cx: &mut TestAppContext, dory: Dory) -> (AnyWindowHandle, Entity<AppState>) {
    cx.update(gpui_kit::init);
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

pub fn id_of(title: &str) -> RecordingId {
    SAMPLES.iter().find(|s| s.title == title).unwrap().id()
}
