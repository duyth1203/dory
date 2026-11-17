//! The Transcript tab and the player, driven through a real window over a real `Dory` (UX-3,
//! A11Y-3, A11Y-10).

use std::path::Path;
use std::sync::Arc;

use dory_core::app::{Dory, Options, Ports};
use dory_core::domain::{
    Channel, Ms, RecordingId, Revision, Segment, SpeakerId, TimeRange, Timestamp, Transcript,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store as _, Takes as _};
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{FakeCapture, ManualClock, MemTakes};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::library::Selection;
use dory_gpui::prefs::Prefs;
use dory_gpui::transcript_view::PAGED_TURNS;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Role, TestAppContext, Window, WindowBounds,
    WindowOptions, px, size, test::TestWindowExt as _,
};

/// Timed, with a take: Speaker 3, Me, Speaker 3.
const TIMED: &str = "Design review";
/// Imported from the old app: no times and no take.
const LEGACY: &str = "1:1 with Sam";
/// Longer than [`PAGED_TURNS`], so it virtualizes.
const LONG: &str = "Roadmap review";
const LONG_TURNS: usize = 150;

fn id_of(title: &str) -> RecordingId {
    SAMPLES.iter().find(|s| s.title == title).unwrap().id()
}

fn at(a: u64, b: u64) -> Option<TimeRange> {
    TimeRange::new(Ms(a), Ms(b))
}

fn segment(speaker: u16, channel: Channel, range: Option<TimeRange>, text: &str) -> Segment {
    Segment {
        range,
        speaker: SpeakerId(speaker),
        channel,
        text: text.to_owned(),
    }
}

fn transcript(segments: Vec<Segment>) -> Transcript {
    Transcript {
        revision: Revision(1),
        segments,
        detected_languages: Vec::new(),
        produced_by: ModelRef::parse("fake/sample").unwrap(),
    }
}

fn transcript_of(title: &str) -> Option<Transcript> {
    let segments = match title {
        TIMED => vec![
            segment(3, Channel::System, at(750_000, 760_000), "Export first."),
            segment(0, Channel::Mic, at(760_000, 770_000), "Agreed."),
            segment(3, Channel::System, at(780_000, 790_000), "Then waveforms."),
        ],
        LEGACY => vec![
            segment(1, Channel::System, None, "Sam wants a mentoring plan."),
            segment(0, Channel::Mic, None, "Let's draft one."),
        ],
        LONG => (0..LONG_TURNS)
            .map(|i| {
                let start = i as u64 * 10_000;
                segment(
                    1 + (i % 2) as u16,
                    Channel::System,
                    at(start, start + 9_000),
                    "Line.",
                )
            })
            .collect(),
        _ => return SAMPLES.iter().find(|s| s.title == title)?.transcript(),
    };
    Some(transcript(segments))
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>, Arc<FakePlayback>) {
    cx.update(gpui_kit::init);
    let store = Arc::new(MemStore::new());
    let mut changes = Vec::new();
    for sample in &SAMPLES {
        changes.push(Change::RecordingCreated(sample.recording()));
        if let Some(transcript) = transcript_of(sample.title) {
            changes.push(Change::TranscriptReplaced {
                id: sample.id(),
                transcript,
            });
        }
    }
    store.apply(changes).expect("seed the samples");
    let takes = Arc::new(MemTakes::new());
    for title in [TIMED, LONG] {
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
    let (window, app) = cx.update(|cx| {
        let mut state = None;
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1100.), px(900.)),
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
    (window, app, playback)
}

/// Selects `title` and shows its Transcript tab.
fn show(title: &str, app: &Entity<AppState>, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of(title)), cx);
        app.set_tab(DetailTab::Transcript, cx);
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

fn position(app: &Entity<AppState>, cx: &mut TestAppContext) -> (Ms, bool) {
    app.read_with(cx, |app, cx| {
        let player = app.session().unwrap().read(cx).player().read(cx);
        (player.position(), player.is_playing())
    })
}

fn labels(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<String> {
    app.read_with(cx, |app, cx| {
        let detail = app.session().unwrap().read(cx).detail().unwrap();
        detail.turns.iter().map(|t| t.label.to_string()).collect()
    })
}

#[gpui_kit::test]
fn turns_read_as_speaker_time_then_text(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, _| {
        let list = window.find("transcript");
        assert_eq!(list.role(), Some(Role::List));
        assert_eq!(list.label(), Some("Transcript"));
        let first = window.find("turn-0");
        assert_eq!(first.role(), Some(Role::ListItem));
        assert_eq!(
            first.label(),
            Some("Speaker 3, 12 minutes 30 seconds, Export first.")
        );
        assert_eq!(
            window.find("turn-1").label(),
            Some("Me, 12 minutes 40 seconds, Agreed.")
        );
        let time = window.find("turn-0-time");
        assert_eq!(time.role(), Some(Role::Button));
        assert_eq!(time.label(), Some("Play from 12:30"));
        assert_eq!(
            window.find("turn-0-speaker").label(),
            Some("Rename Speaker 3")
        );
    });
}

/// Handoff section 4: each turn has a 30 pt initial circle, however tall its text runs.
#[gpui_kit::test]
fn the_speaker_initial_is_a_30_pt_circle(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, _| {
        let turn = window.find("turn-0").bounds();
        let avatar = window.find("turn-0-avatar").bounds();
        assert!(
            turn.size.height > px(30.),
            "the turn is taller than the circle: {turn:?}"
        );
        assert_eq!(
            (avatar.size.width, avatar.size.height),
            (px(30.), px(30.)),
            "a circle, not a pill the height of the turn"
        );
    });
}

#[gpui_kit::test]
fn a_timestamp_plays_from_its_turn_and_the_turn_shows_playing(cx: &mut TestAppContext) {
    let (window, app, playback) = open(cx);
    show(TIMED, &app, cx);
    assert!(playback.opened().is_empty(), "selecting opens no audio");
    with_window(window, cx, |window, cx| window.click("turn-2-time", cx));
    assert_eq!(position(&app, cx), (Ms(780_000), true));
    with_window(window, cx, |window, _| {
        assert_eq!(window.find("turn-2").description(), Some("Playing"));
        assert_eq!(window.find("turn-0").description(), None);
    });

    playback.advance(Ms(15_000));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    assert_eq!(position(&app, cx).0, Ms(795_000));
    with_window(window, cx, |window, _| {
        assert_eq!(
            window.find("turn-2").description(),
            None,
            "the playhead left the turn's range"
        );
    });
}

#[gpui_kit::test]
fn arrows_move_the_current_turn_and_return_plays_it(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, cx| {
        window.click("turn-0", cx);
        window.render_frame(cx);
        assert_eq!(window.find("transcript").focused(), Some(true));
        window.press("down", cx);
        window.press("down", cx);
        window.press("down", cx);
        window.press("up", cx);
        window.press("enter", cx);
    });
    assert_eq!(position(&app, cx), (Ms(760_000), true), "Me's turn");
}

#[gpui_kit::test]
fn renaming_a_speaker_relabels_every_one_of_their_turns(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, cx| {
        window.click("turn-2-speaker", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("turn-2-rename-hint").label(),
            Some("Renames Speaker 3 in all 2 turns. Return to save, Escape to cancel.")
        );
        assert_eq!(window.find("turn-2-rename").role(), Some(Role::TextInput));
        window.input("Alex", cx);
        window.press("enter", cx);
    });
    assert_eq!(labels(&app, cx), ["Alex", "Me", "Alex"]);
    with_window(window, cx, |window, _| {
        assert!(
            window.try_find("turn-2-rename").is_none(),
            "the field closed"
        );
        assert_eq!(window.find("transcript").focused(), Some(true));
        assert_eq!(window.find("turn-0-speaker").label(), Some("Rename Alex"));
    });
}

#[gpui_kit::test]
fn escape_cancels_a_rename_and_shift_return_starts_one(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, cx| {
        window.click("turn-1", cx);
        window.press("shift-enter", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("turn-1-rename-hint").label(),
            Some("Renames Me in its one turn. Return to save, Escape to cancel.")
        );
        window.input("Nolan", cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find("turn-1-rename").is_none());
    });
    assert_eq!(labels(&app, cx), ["Speaker 3", "Me", "Speaker 3"]);
}

#[gpui_kit::test]
fn legacy_turns_have_no_timestamp_and_do_not_seek(cx: &mut TestAppContext) {
    let (window, app, playback) = open(cx);
    show(LEGACY, &app, cx);
    with_window(window, cx, |window, cx| {
        let turn = window.find("turn-0");
        assert_eq!(turn.label(), Some("Speaker 1, Sam wants a mentoring plan."));
        assert_eq!(
            turn.description(),
            Some("No timestamp, so it cannot play from here")
        );
        assert!(window.try_find("turn-0-time").is_none());
        window.click("turn-0", cx);
        window.press("enter", cx);
    });
    assert!(playback.opened().is_empty());
    assert_eq!(position(&app, cx), (Ms(0), false));
}

#[gpui_kit::test]
fn a_long_transcript_lays_out_only_the_turns_in_view(cx: &mut TestAppContext) {
    const { assert!(LONG_TURNS > PAGED_TURNS) };
    let (window, app, _) = open(cx);
    show(LONG, &app, cx);
    with_window(window, cx, |window, cx| {
        assert!(window.try_find("turn-0").is_some());
        assert!(
            window
                .try_find(format!("turn-{}", LONG_TURNS - 1))
                .is_none()
        );
        window.click("turn-0", cx);
        for _ in 0..LONG_TURNS {
            window.press("down", cx);
        }
        window.render_frame(cx);
        // The detail pane owns page scrolling, so check against the list's own viewport.
        let list = window.find("transcript").bounds();
        let last = window.find(format!("turn-{}", LONG_TURNS - 1));
        assert_eq!(last.role(), Some(Role::ListItem));
        let shown = last.bounds();
        assert!(
            shown.top() >= list.top() && shown.bottom() <= list.bottom(),
            "Down scrolls the current turn into the list: {shown:?} in {list:?}"
        );
        assert!(
            window.try_find("turn-0").is_none(),
            "the first turn scrolled away"
        );
    });
}

#[gpui_kit::test]
fn the_waveform_is_the_slider_and_the_buttons_move_the_playhead(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, cx| {
        let slider = window.find("player");
        assert_eq!(slider.role(), Some(Role::Slider));
        assert_eq!(slider.label(), Some("Playback position"));
        assert_eq!(slider.value(), Some("0 seconds of 48 minutes"));
        assert_eq!(window.find("player-play").label(), Some("Play"));
        assert_eq!(window.find("player-time").label(), Some("0:00 / 48:00"));
        window.click("player-forward", cx);
        window.click("player-forward", cx);
        window.click("player-back", cx);
    });
    assert_eq!(position(&app, cx), (Ms(5_000), false));
    with_window(window, cx, |window, cx| {
        window.click("player", cx);
        window.press("pageup", cx);
        window.press("space", cx);
    });
    let (at, playing) = position(&app, cx);
    assert!(playing, "Space plays");
    assert!(
        (24 * 60_000 + 50_000..=24 * 60_000 + 70_000).contains(&at.0),
        "a click at the middle seeks to the middle, then Page Up adds a minute: {at:?}"
    );
    with_window(window, cx, |window, _| {
        assert_eq!(window.find("player-play").label(), Some("Pause"));
    });
}

#[gpui_kit::test]
fn the_player_says_why_audio_will_not_open(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(LEGACY, &app, cx);
    with_window(window, cx, |window, cx| {
        assert!(window.try_find("player-unavailable").is_none());
        window.click("player-play", cx);
    });
    with_window(window, cx, |window, _| {
        let why = window.find("player-unavailable");
        assert_eq!(why.role(), Some(Role::Status));
        assert_eq!(why.label(), Some("This recording has no audio."));
    });
}

/// Paused, the turn under the playhead keeps its place in the list but no longer says "Playing".
#[gpui_kit::test]
fn a_paused_turn_does_not_say_playing(cx: &mut TestAppContext) {
    let (window, app, _) = open(cx);
    show(TIMED, &app, cx);
    with_window(window, cx, |window, cx| window.click("turn-2-time", cx));
    assert_eq!(position(&app, cx), (Ms(780_000), true));
    with_window(window, cx, |window, _| {
        assert_eq!(window.find("turn-2").description(), Some("Playing"));
    });

    app.update(cx, |app, cx| {
        let player = app.session().unwrap().read(cx).player().clone();
        player.update(cx, |player, cx| player.toggle(cx));
    });
    cx.run_until_parked();
    assert_eq!(position(&app, cx), (Ms(780_000), false), "paused");
    with_window(window, cx, |window, _| {
        assert_eq!(
            window.find("turn-2").description(),
            None,
            "a paused playhead does not say Playing"
        );
    });
}
