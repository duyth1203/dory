//! Drives `AvPlayback` on the real audio engine with a silent take, so it makes no sound but needs
//! an output device. Run with `cargo test -p dory-macos --test playback -- --ignored`.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dory_core::domain::{ChannelLayout, Ms};
use dory_core::ports::{PlayState, Playback, PlaybackSource};
use dory_macos::AvPlayback;

const RATE: u32 = 48_000;

fn silent_take(name: &str, seconds: u32, channels: u16) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("dory-playback-{}-{name}.wav", std::process::id()));
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for _ in 0..RATE * seconds * u32::from(channels) {
        writer.write_sample(0.0f32).unwrap();
    }
    writer.finalize().unwrap();
    path
}

fn source(path: &Path, layout: ChannelLayout, seconds: u64) -> PlaybackSource {
    PlaybackSource {
        path: path.to_path_buf(),
        layout,
        duration: Ms(seconds * 1000),
    }
}

fn wait_for(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "needs an audio output device"]
fn plays_pauses_seeks_ends_and_replays() {
    let path = silent_take("flow", 3, 2);
    let player = AvPlayback::new()
        .open(&source(&path, ChannelLayout::MicSystem, 3))
        .unwrap();
    assert_eq!(player.status().state, PlayState::Paused);
    assert_eq!(player.status().position, Ms(0));

    player.play().unwrap();
    wait_for("500 ms of playback", Duration::from_secs(3), || {
        player.status().position >= Ms(500)
    });
    player.pause();
    let frozen = player.status();
    assert_eq!(frozen.state, PlayState::Paused);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        player.status().position,
        frozen.position,
        "pause holds the position"
    );

    player.seek(Ms(2_000)).unwrap();
    assert_eq!(player.status().state, PlayState::Paused);
    assert_eq!(player.status().position, Ms(2_000));
    player.play().unwrap();
    wait_for("the end", Duration::from_secs(4), || {
        player.status().state == PlayState::Ended
    });
    assert_eq!(player.status().position, Ms(3_000));

    player.play().unwrap();
    wait_for("a replay from the start", Duration::from_secs(2), || {
        let status = player.status();
        status.state == PlayState::Playing && status.position < Ms(1_000)
    });
    player.seek(Ms(9_999_000)).unwrap();
    assert_eq!(player.status().state, PlayState::Ended);
    let _ = std::fs::remove_file(path);
}

#[test]
#[ignore = "needs an audio output device"]
fn seeking_and_dropping_while_playing_never_hangs() {
    let path = silent_take("storm", 30, 2);
    let (done, finished) = std::sync::mpsc::channel();
    let worker_path = path.clone();
    std::thread::spawn(move || {
        for round in 0..6u64 {
            let player = AvPlayback::new()
                .open(&source(&worker_path, ChannelLayout::MicSystem, 30))
                .unwrap();
            player.play().unwrap();
            for step in 0..10 {
                player.seek(Ms((round * 3 + step) * 700)).unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(player);
        }
        let _ = done.send(());
    });
    finished
        .recv_timeout(Duration::from_secs(60))
        .expect("seek and drop under load deadlocked");
    let _ = std::fs::remove_file(path);
}

#[test]
#[ignore = "needs an audio output device"]
fn refuses_a_take_whose_channels_do_not_match_the_layout_and_a_missing_file() {
    let mono = silent_take("mono", 1, 1);
    assert!(
        AvPlayback::new()
            .open(&source(&mono, ChannelLayout::MicSystem, 1))
            .is_err()
    );
    assert!(
        AvPlayback::new()
            .open(&source(&mono, ChannelLayout::Mixed, 1))
            .is_ok()
    );
    let missing = PathBuf::from("/nonexistent/dory.flac");
    let error = AvPlayback::new()
        .open(&source(&missing, ChannelLayout::Mixed, 1))
        .err()
        .expect("a missing file fails");
    assert!(error.to_string().contains("cannot be played"));
    let _ = std::fs::remove_file(mono);
}
