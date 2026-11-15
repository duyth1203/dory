//! Capture harness: runs `CoreAudioCapture` into a WAV file and reports what it saw.
//! It uses `dory-macos` directly only because no composition root crate exists yet to hand it a backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dory_core::capture::{FrameSource, frame_channel};
use dory_core::domain::{Channel, ChannelLayout};
use dory_core::ports::capture::{Access, CaptureBackend, CaptureHandle, CaptureSpec, Source};
use dory_macos::CoreAudioCapture;

use crate::args::CaptureArgs;

const RING_SECONDS: usize = 2;
const DRAIN_PERIOD: Duration = Duration::from_millis(20);
const PERMISSION_WAIT: Duration = Duration::from_secs(30);
const SELF_TONE_HZ: f64 = 440.0;

pub fn devices() -> Result<(), String> {
    let backend = CoreAudioCapture::new();
    println!("probe: {:?}", backend.probe());
    for d in backend.input_devices().map_err(|e| e.to_string())? {
        println!(
            "{} {:?} uid={}",
            if d.is_default { "*" } else { " " },
            d.name,
            d.uid
        );
    }
    Ok(())
}

pub fn run(args: &CaptureArgs) -> Result<(), String> {
    let backend = CoreAudioCapture::new();
    let caps = backend.probe();
    println!(
        "probe: microphone={:?} system_audio={:?} unsupported={:?}",
        caps.microphone, caps.system_audio, caps.unsupported
    );
    if args.layout.index_of(Channel::Mic).is_some() && caps.microphone == Access::Undetermined {
        println!(
            "requesting microphone access (waiting up to {}s for an answer)",
            PERMISSION_WAIT.as_secs()
        );
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(CoreAudioCapture::new().request_access(Source::Microphone));
        });
        match rx.recv_timeout(PERMISSION_WAIT) {
            Ok(answer) => println!("microphone access: {answer:?}"),
            Err(_) => {
                return Err("blocked: the microphone permission prompt was not answered".into());
            }
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let on_signal = Arc::clone(&stop);
    ctrlc::set_handler(move || on_signal.store(true, Ordering::Relaxed))
        .map_err(|e| e.to_string())?;

    let spec = CaptureSpec {
        layout: args.layout,
        rate_hz: args.rate_hz,
        mic: args.mic.clone(),
    };
    let (sink, source) = frame_channel(args.layout, args.rate_hz as usize * RING_SECONDS);
    let handle = backend
        .start_capture(&spec, sink)
        .map_err(|e| format!("start failed: {e}"))?;
    let running = Instant::now();
    let clock = handle.timeline_clock();
    let g = handle.graph();
    print_graph(&g);

    let writer = {
        let out = args.out.clone();
        let layout = args.layout;
        let rate = args.rate_hz;
        thread::Builder::new()
            .name("dory-cli-writer".into())
            .spawn(move || write_take(source, layout, rate, &out))
            .map_err(|e| e.to_string())?
    };

    let tone = if args.self_tone {
        let tone = crate::tone::Tone::play(SELF_TONE_HZ)?;
        println!("playing a {SELF_TONE_HZ} Hz tone from this process");
        Some(tone)
    } else {
        None
    };

    let started = Instant::now();
    let deadline = Duration::from_secs_f64(args.seconds);
    let mut rebuild_at = args.rebuild_at.map(Duration::from_secs_f64);
    while started.elapsed() < deadline && !stop.load(Ordering::Relaxed) {
        if rebuild_at.is_some_and(|at| started.elapsed() >= at) {
            rebuild_at = None;
            println!(
                "  requesting a graph rebuild at {:.2} s wall",
                started.elapsed().as_secs_f64()
            );
            handle.rebuild();
        }
        thread::sleep(Duration::from_millis(10));
    }
    let interrupted = stop.load(Ordering::Relaxed);
    let wall = started.elapsed();
    let last = handle.graph();
    if last.generation != g.generation {
        println!("graph at stop:");
        print_graph(&last);
    }
    drop(tone);
    let handle: Box<dyn CaptureHandle> = Box::new(handle);
    handle.stop();
    let ran = running.elapsed();
    let totals = writer
        .join()
        .map_err(|_| "writer thread panicked".to_string())??;

    let channels = args.layout.channel_count() as u64;
    println!(
        "stopped{} after {:.3} s wall; {:.3} s from start returning to stop returning",
        if interrupted { " by Ctrl-C" } else { "" },
        wall.as_secs_f64(),
        ran.as_secs_f64()
    );
    println!(
        "total frames {} ({:.3} s at {} Hz), padded frames {}, samples {}, issues dropped {}",
        totals.frames + totals.padded,
        (totals.frames + totals.padded) as f64 / args.rate_hz as f64,
        args.rate_hz,
        totals.padded,
        (totals.frames + totals.padded) * channels,
        totals.dropped_issues
    );
    println!(
        "host time from first frame to last: {:.4} s = {:.0} frames; skipped input filled with {} frames",
        clock.seconds(),
        clock.seconds() * args.rate_hz as f64,
        clock.skipped_frames()
    );
    println!("wrote {}", args.out.display());
    Ok(())
}

fn print_graph(g: &dory_macos::CaptureGraph) {
    println!(
        "graph {}: aggregate id={} uid={}",
        g.generation, g.aggregate_id, g.aggregate_uid
    );
    println!("tap: id={:?} format={:?}", g.tap_id, g.tap_format);
    println!(
        "clock device: {:?}, mic device: {:?}",
        g.clock_device, g.mic_device
    );
    println!("tap excludes process object {:?}", g.excluded_process);
    println!(
        "aggregate inputs: buffers {:?}, mic reads {} ch, system reads {} ch, device {} Hz -> output {} Hz",
        g.input_buffers, g.mic_channels, g.system_channels, g.device_rate_hz, g.output_rate_hz
    );
}

struct Totals {
    frames: u64,
    padded: u64,
    dropped_issues: u64,
}

/// Drains the ring into a 16-bit WAV until the sink is dropped, printing per-second peaks.
fn write_take(
    mut source: FrameSource,
    layout: ChannelLayout,
    rate: u32,
    out: &std::path::Path,
) -> Result<Totals, String> {
    let channels = layout.channel_count();
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav =
        hound::WavWriter::create(out, spec).map_err(|e| format!("{}: {e}", out.display()))?;
    let names: Vec<&str> = layout
        .channels()
        .iter()
        .map(|c| match c {
            Channel::Mic => "mic",
            Channel::System => "system",
            Channel::Mixed => "mixed",
        })
        .collect();
    let mut totals = Totals {
        frames: 0,
        padded: 0,
        dropped_issues: 0,
    };
    let mut buf = Vec::with_capacity(rate as usize * channels);
    let mut peaks = vec![0.0f32; channels];
    let mut in_second = 0u64;
    let mut second = 0u64;
    loop {
        let finished = source.is_finished();
        buf.clear();
        let drained = source.drain(&mut buf);
        totals.frames += drained.frames;
        totals.padded += drained.padded_frames;
        if drained.padded_frames > 0 {
            println!(
                "  ring overflow: padded {} frames with silence",
                drained.padded_frames
            );
        }
        for issue in source.issues() {
            println!(
                "  issue at {:.2} s: {issue:?}",
                (totals.frames + totals.padded) as f64 / rate as f64
            );
        }
        for frame in buf.chunks_exact(channels) {
            for (c, s) in frame.iter().enumerate() {
                peaks[c] = peaks[c].max(s.abs());
                wav.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .map_err(|e| e.to_string())?;
            }
            in_second += 1;
            if in_second == rate as u64 {
                second += 1;
                let levels: Vec<String> = names
                    .iter()
                    .zip(&peaks)
                    .map(|(n, p)| format!("{n} {}", dbfs(*p)))
                    .collect();
                println!("  {second:>3}s peak {}", levels.join(", "));
                wav.flush().map_err(|e| e.to_string())?;
                peaks.iter_mut().for_each(|p| *p = 0.0);
                in_second = 0;
            }
        }
        if finished {
            break;
        }
        thread::sleep(DRAIN_PERIOD);
    }
    totals.dropped_issues = source.dropped_issue_count();
    wav.finalize().map_err(|e| e.to_string())?;
    Ok(totals)
}

fn dbfs(peak: f32) -> String {
    if peak <= 0.0 {
        "-inf dBFS".into()
    } else {
        format!("{:6.1} dBFS", 20.0 * peak.log10())
    }
}
