//! The player's waveform draws the take's loudness, read off the main thread through the
//! `PlayerHandle`, and a flat floor until then.

mod common;

use std::sync::Arc;

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{ChannelLayout, RecordingId, Timestamp, Waveform};
use dory_core::ports::Takes as _;
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::{FakeCapture, FakeTrash, ManualClock, MemTakes};
use dory_gpui::app::AppState;
use dory_gpui::fixtures;
use dory_gpui::library::Selection;
use dory_gpui::player_view::PlayerView;
use gpui_kit::{
    AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size,
};

use common::{id_of, open_with, sample_store};

/// Twenty seconds, speech on the system channel for the first ten, then silence.
const SPOKEN: &str = "Design review";
/// A row with a take whose audio is not there.
const UNREADABLE: &str = "Weekly sync";

fn dory(takes: Arc<MemTakes>) -> Dory {
    Dory::open(
        Ports {
            store: sample_store(),
            takes,
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            trash: Arc::new(FakeTrash::new(std::env::temp_dir())),
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
    .expect("open Dory")
}

fn spoken_then_silent(takes: &MemTakes, id: &RecordingId) {
    const RATE: usize = 8_000;
    let mut writer = takes
        .begin(id, ChannelLayout::MicSystem, RATE as u32)
        .expect("begin");
    let speech: Vec<f32> = (0..RATE * 10)
        .flat_map(|i| [0., 0.4 * (i as f32 * 0.2).sin()])
        .collect();
    writer.write(&speech).expect("write the speech");
    writer
        .write(&vec![0.; RATE * 10 * 2])
        .expect("write the silence");
    writer.finish().expect("finish");
    takes.seal(id).expect("seal");
}

/// Selects `title` and opens a window of its own around the session's player.
fn player_for(title: &str, app: &Entity<AppState>, cx: &mut TestAppContext) -> Entity<PlayerView> {
    app.update(cx, |app, cx| app.select(Selection::One(id_of(title)), cx));
    let handle = app.read_with(cx, |app, cx| {
        app.session()
            .expect("a selection")
            .read(cx)
            .player()
            .clone()
    });
    let app = app.clone();
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(400.), px(56.)),
                })),
                ..Default::default()
            },
            cx,
            |_, cx| cx.new(|cx| PlayerView::new(handle, app, cx)),
        )
        .expect("open the player window")
        .1
    })
}

fn painted(view: &Entity<PlayerView>, cx: &mut TestAppContext) -> Vec<f32> {
    cx.run_until_parked();
    cx.update(|cx| cx.refresh_windows());
    cx.run_until_parked();
    view.read_with(cx, |view, _| view.bar_heights())
}

#[gpui_kit::test]
fn the_bars_rise_where_the_take_is_loud_and_rest_at_the_floor_where_it_is_silent(
    cx: &mut TestAppContext,
) {
    let takes = Arc::new(MemTakes::new());
    spoken_then_silent(&takes, &id_of(SPOKEN));
    let (_, app) = open_with(cx, dory(takes));

    let view = player_for(SPOKEN, &app, cx);
    let bars = painted(&view, cx);

    assert!(bars.len() > 50, "the width holds many bars: {}", bars.len());
    let (spoken, silent) = bars.split_at(bars.len() / 2);
    assert!(
        spoken.iter().all(|h| *h > 0.9),
        "the spoken half is tall: {spoken:?}"
    );
    assert!(
        silent[1..].iter().all(|h| *h == Waveform::FLOOR),
        "the silent half rests on the floor: {silent:?}"
    );
}

#[gpui_kit::test]
fn a_take_that_cannot_be_read_keeps_the_flat_placeholder(cx: &mut TestAppContext) {
    let (_, app) = open_with(cx, dory(Arc::new(MemTakes::new())));

    let view = player_for(UNREADABLE, &app, cx);
    let bars = painted(&view, cx);

    assert!(!bars.is_empty());
    assert!(
        bars.iter().all(|h| *h == Waveform::FLOOR),
        "no shape the audio does not have: {bars:?}"
    );
}
