//! `record`: drives the recorder through the facade and reports from its events.
//!
//! Keys on stdin, one per line: Enter stops, `p` pauses or resumes, `u` undoes a stop during the
//! grace window. Ctrl-C stops too. A closed stdin stops and keeps stopping: an undo read before it
//! cannot resume a recording that nobody can stop anymore.

use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use dory::Config;
use dory_core::app::{Dory, Event, RecorderStatus, StartOptions, Subscription};
use dory_core::domain::{Channel, Ms, TakeContainer};
use dory_core::ports::capture::{Access, Source};

use crate::args::RecordArgs;
use crate::view;
use crate::wait::block_on;

const TICK: Duration = Duration::from_millis(20);
const LEVEL_EVERY: Duration = Duration::from_secs(1);
const PERMISSION_WAIT: Duration = Duration::from_secs(30);

const FAKE_TITLE: &str = "Fake tone";

enum Key {
    Stop,
    Pause,
    Undo,
    /// Stdin closed. Nothing more can be typed, so the recording must end.
    Closed,
    Unknown(String),
}

pub fn run(config: &Config, args: &RecordArgs) -> Result<(), String> {
    let dory = match args.fake_capture {
        Some(_) => {
            crate::allow_fakes("--fake-capture")?;
            dory::open_with_fake_capture(config)
        }
        None => dory::open(config),
    }
    .map_err(crate::chain)?;
    crate::report_recovery(dory.recovery());
    if args.fake_capture.is_none() {
        check_access(&dory, args)?;
    }

    let events = dory.events();
    let (keys_tx, keys) = mpsc::channel();
    watch_stdin(keys_tx.clone());
    let on_interrupt = keys_tx;
    ctrlc::set_handler(move || {
        let _ = on_interrupt.send(Key::Stop);
    })
    .map_err(|e| e.to_string())?;

    let recorder = dory.recorder();
    recorder
        .start(StartOptions {
            title: args.fake_capture.map(|_| FAKE_TITLE.to_owned()),
            layout: args.layout,
            mic: args.mic.clone(),
            ..StartOptions::default()
        })
        .map_err(crate::chain)?;
    println!("recording to {}", config.data_dir.display());
    println!("Enter stops, p pauses or resumes, u undoes a stop");

    let stop_at = args.fake_capture.map(|s| Ms(u64::from(s) * 1000));
    let names = channel_names(args);
    // The loudest peak per channel since the last line, and the latest recorded time.
    let mut peaks = vec![0.0f32; names.len()];
    let mut recorded_at = Ms(0);
    // A stop sent but not yet seen as Grace, so a key, a closed stdin, and the fake capture's
    // timer behind it send only one.
    let mut stop_sent = false;
    let mut stdin_closed = false;
    let mut auto_stopped = false;
    let mut next_levels = Instant::now() + LEVEL_EVERY;
    let mut errors = 0;
    let mut active = false;
    let finished = loop {
        // Status first: an Idle read means everything it announced is already queued below.
        let status = recorder.status();
        active |= status != RecorderStatus::Idle;
        let capturing = matches!(
            status,
            RecorderStatus::Recording { .. } | RecorderStatus::Paused { .. }
        );
        if matches!(status, RecorderStatus::Grace { .. }) {
            stop_sent = false;
        }
        let mut finished = None;
        while let Some(event) = events.try_recv() {
            match event {
                Event::Finished { id, reason } => finished = Some((id, reason)),
                Event::Levels {
                    peaks: now,
                    recorded,
                } => {
                    for (peak, now) in peaks.iter_mut().zip(now) {
                        *peak = peak.max(now);
                    }
                    recorded_at = recorded;
                }
                Event::Recorder(status) => announce(&status),
                Event::CaptureIssue(issue) => println!("  capture issue: {issue:?}"),
                Event::TimeWarning { remaining } => {
                    println!("  recording ends by itself in {}", view::clock(remaining));
                }
                Event::Rejected(input) => println!("  not now: {input:?} does not apply here"),
                Event::Error { context, message } => {
                    errors += 1;
                    eprintln!("  error {context}: {message}");
                }
                _ => {}
            }
        }
        if let Some(done) = finished {
            break done;
        }
        if status == RecorderStatus::Idle && (active || errors > 0) {
            return Err("the recording did not start or ended without a take".into());
        }

        while let Ok(key) = keys.try_recv() {
            match (key, &status) {
                (Key::Closed, _) => stdin_closed = true,
                (Key::Stop, _) if stop_sent => {}
                (Key::Stop, _) if capturing => {
                    stop_sent = true;
                    let _ = recorder.stop();
                }
                (Key::Stop, _) => println!("  already stopped, u undoes it"),
                (Key::Pause, _) if stop_sent => {}
                (Key::Pause, RecorderStatus::Recording { .. }) => {
                    let _ = recorder.pause();
                }
                (Key::Pause, RecorderStatus::Paused { .. }) => {
                    let _ = recorder.resume();
                }
                (Key::Pause, _) => println!("  p only works while recording or paused"),
                (Key::Undo, _) => {
                    stop_sent = false;
                    let _ = recorder.undo_stop();
                }
                (Key::Unknown(text), _) => {
                    println!("  unknown key {text:?}: Enter, p, or u");
                }
            }
        }
        let timer_due = !auto_stopped
            && matches!((stop_at, &status), (Some(at), RecorderStatus::Recording { recorded, .. }) if *recorded >= at);
        auto_stopped |= timer_due;
        // Checked against every fresh status, not only when the key arrives: an undo queued just
        // before the stdin closed resumes after the status above was read.
        if (stdin_closed || timer_due) && capturing && !stop_sent {
            stop_sent = true;
            let _ = recorder.stop();
        }

        if matches!(status, RecorderStatus::Recording { .. }) && Instant::now() >= next_levels {
            next_levels = Instant::now() + LEVEL_EVERY;
            println!(
                "  {:>5}  peak {}",
                view::clock(recorded_at),
                level_text(&names, &peaks)
            );
            peaks.fill(0.0);
        }
        std::thread::sleep(TICK);
    };

    let (id, reason) = finished;
    println!("recorded {id} ({reason:?})");
    // Shutting down waits for the take to seal, so the row below is final, and every error the
    // seal reported is already queued.
    dory.shutdown();
    errors += report_errors(&events);
    // The row says whether the seal landed, even if its error event was lost.
    match dory.library().recording(&id).map_err(crate::chain)? {
        Some(recording) => match recording.take {
            Some(take) if take.container != TakeContainer::Wav => println!(
                "sealed {:?} {} Hz {:.1} s",
                take.container,
                take.rate_hz,
                take.duration.0 as f64 / 1000.0
            ),
            _ => {
                errors += 1;
                eprintln!("the audio is not sealed, the next start finishes it");
            }
        },
        None => return Err(format!("{id} is not in the library")),
    }
    match errors {
        0 => Ok(()),
        n => Err(format!(
            "{n} error(s) while recording {id}, see above. The next start repairs what it can."
        )),
    }
}

/// Prints the errors still queued and counts them.
fn report_errors(events: &Subscription) -> usize {
    let mut errors = 0;
    while let Some(event) = events.try_recv() {
        match event {
            Event::Error { context, message } => {
                errors += 1;
                eprintln!("  error {context}: {message}");
            }
            Event::Resync => eprintln!("  some events were lost, errors may be missing"),
            _ => {}
        }
    }
    errors
}

fn announce(status: &RecorderStatus) {
    match status {
        RecorderStatus::Idle => {}
        RecorderStatus::Recording { id, .. } => println!("recording {id}"),
        RecorderStatus::Paused { recorded, .. } => {
            println!("paused at {}, p resumes", view::clock(*recorded));
        }
        RecorderStatus::Grace { recorded, .. } => {
            println!(
                "stopped at {}, u within 5 s undoes it",
                view::clock(*recorded)
            );
        }
    }
}

fn channel_names(args: &RecordArgs) -> Vec<&'static str> {
    args.layout
        .channels()
        .iter()
        .map(|c| match c {
            Channel::Mic => "mic",
            Channel::System => "system",
            Channel::Mixed => "mixed",
        })
        .collect()
}

fn level_text(names: &[&str], peaks: &[f32]) -> String {
    names
        .iter()
        .zip(peaks)
        .map(|(name, peak)| format!("{name} {}", view::dbfs(*peak)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Refuses early when capture cannot work, and asks for the microphone if macOS has not been asked.
fn check_access(dory: &Dory, args: &RecordArgs) -> Result<(), String> {
    let caps = block_on(dory.recorder().probe()).map_err(crate::chain)?;
    println!(
        "probe: microphone={:?} system_audio={:?}",
        caps.microphone, caps.system_audio
    );
    if let Some(why) = caps.unsupported {
        return Err(format!("capture is not supported here: {why}"));
    }
    if args.layout.index_of(Channel::Mic).is_none() {
        return Ok(());
    }
    let mut access = caps.microphone;
    if access == Access::Undetermined {
        println!(
            "requesting microphone access (waiting up to {} s for an answer)",
            PERMISSION_WAIT.as_secs()
        );
        let asked = dory.recorder().request_access(Source::Microphone);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(block_on(asked));
        });
        access = match rx.recv_timeout(PERMISSION_WAIT) {
            Ok(answer) => answer.map_err(crate::chain)?,
            Err(_) => return Err("the microphone permission prompt was not answered".into()),
        };
    }
    match access {
        Access::Denied => Err(
            "microphone access is denied, allow it in System Settings > Privacy & Security".into(),
        ),
        _ => Ok(()),
    }
}

fn watch_stdin(keys: Sender<Key>) {
    let _ = std::thread::Builder::new()
        .name("dory-cli-stdin".into())
        .spawn(move || read_keys(std::io::stdin().lines().map_while(Result::ok), &keys));
}

fn read_keys(lines: impl Iterator<Item = String>, keys: &Sender<Key>) {
    for line in lines {
        let key = match line.trim() {
            "" => Key::Stop,
            "p" => Key::Pause,
            "u" => Key::Undo,
            other => Key::Unknown(other.to_owned()),
        };
        if keys.send(key).is_err() {
            return;
        }
    }
    let _ = keys.send(Key::Closed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of(input: &str) -> Vec<String> {
        let (tx, rx) = mpsc::channel();
        read_keys(input.lines().map(String::from), &tx);
        drop(tx);
        rx.iter()
            .map(|k| match k {
                Key::Stop => "stop".into(),
                Key::Pause => "pause".into(),
                Key::Undo => "undo".into(),
                Key::Closed => "closed".into(),
                Key::Unknown(t) => format!("unknown {t}"),
            })
            .collect()
    }

    #[test]
    fn keys_map_to_commands_and_a_closed_stdin_stops() {
        assert_eq!(
            keys_of("p\n  p \nu\nx\n\n"),
            ["pause", "pause", "undo", "unknown x", "stop", "closed"]
        );
        assert_eq!(keys_of(""), ["closed"]);
    }
}
