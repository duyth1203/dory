//! Live Apple Speech checks against the test library's takes. They need English speech assets
//! installed and a copy of the library, so they are ignored by default:
//!
//! ```sh
//! cp -cR "$HOME/Library/Application Support/dory-dev/live-data" /tmp/dory-ai-p3-live
//! DORY_APPLE_LIVE=1 cargo test -p dory-macos --features apple-intelligence --test speech_live \
//!   -- --ignored --nocapture
//! ```
//!
//! `DORY_SPEECH_LIBRARY` points at another copy. The source library is never opened.
#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dory_core::domain::{Channel, Fraction, LangTag, LanguagePref, Ms};
use dory_core::models::{Availability, ModelId, Stt};
use dory_core::ports::{
    Activity, CancelToken, Clip, EngineError, ModelProvider, RawSegment, SegmentSink,
    TranscribeRequest,
};
use dory_macos::AppleSpeech;

const WEEKLY: &str = "2026-11-16T041159-0000";
const WEEKLY_MS: u64 = 42_592;
const TWO_HOURS: &str = "2026-11-16T041200-0001";
const TWO_HOURS_MS: u64 = 7_905_589;

fn live() -> bool {
    let on = std::env::var("DORY_APPLE_LIVE").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set DORY_APPLE_LIVE=1 to call Apple Speech");
    }
    on
}

fn clip(take: &str, duration: u64) -> Clip {
    let library = std::env::var_os("DORY_SPEECH_LIBRARY")
        .map_or_else(|| PathBuf::from("/tmp/dory-ai-p3-live"), PathBuf::from);
    let path = library.join("takes").join(format!("{take}.flac"));
    assert!(
        path.exists(),
        "copy the test library first: {}",
        path.display()
    );
    Clip::new(
        path,
        Channel::Mixed,
        Ms(duration),
        Ms(0),
        Activity::Unmeasured,
        None,
    )
}

fn english() -> ModelId<Stt> {
    "apple/speech-en".parse().unwrap()
}

fn request(clip: &Clip) -> TranscribeRequest<'_> {
    TranscribeRequest {
        clip,
        language: LanguagePref::Pinned(LangTag::parse("en").unwrap()),
        diarize: false,
        hint: None,
        threads: 1,
        sentence_times: false,
    }
}

#[derive(Default)]
struct Collect {
    segments: Vec<RawSegment>,
    progress: Vec<f32>,
    first: Option<mpsc::Sender<()>>,
}

impl SegmentSink for Collect {
    fn progress(&mut self, fraction: Fraction) {
        self.progress.push(fraction.get());
    }

    fn segment(&mut self, segment: RawSegment) {
        if let Some(first) = self.first.take() {
            let _ = first.send(());
        }
        self.segments.push(segment);
    }
}

#[test]
#[ignore = "calls Apple Speech; run with DORY_APPLE_LIVE=1 and --ignored"]
fn the_weekly_sync_transcribes() {
    if !live() {
        return;
    }
    let started = Instant::now();
    let speech = AppleSpeech::load();
    let load_ms = started.elapsed().as_millis();
    let catalog = speech.catalog();
    let row = catalog
        .iter()
        .find(|e| e.id == english())
        .expect("an English row");
    assert_eq!(row.display_name, "Apple Speech (English)");

    let started = Instant::now();
    let availability = speech.availability(&english());
    let availability_ms = started.elapsed().as_millis();
    assert_eq!(availability, Availability::Ready);

    let cancel = CancelToken::new();
    let started = Instant::now();
    let engine = speech.open(&english(), &cancel).expect("open");
    let open_ms = started.elapsed().as_millis();

    let clip = clip(WEEKLY, WEEKLY_MS);
    let mut sink = Collect::default();
    let started = Instant::now();
    engine
        .transcribe(&request(&clip), &mut sink, &cancel)
        .expect("transcribe");
    let transcribe_ms = started.elapsed().as_millis();

    let segments = &sink.segments;
    let sample = &segments[segments.len() / 2];
    let tail = segments.last().expect("a segment");
    eprintln!(
        "rows={} load_ms={load_ms} availability_ms={availability_ms} open_ms={open_ms} \
         transcribe_ms={transcribe_ms} segments={} sample=[{}..{}] {:?} last=[{}..{}] {:?}",
        catalog.len(),
        segments.len(),
        sample.range.start().0,
        sample.range.end().0,
        sample.text,
        tail.range.start().0,
        tail.range.end().0,
        tail.text
    );
    assert!(
        (14..=18).contains(&segments.len()),
        "{} segments",
        segments.len()
    );
    let mut last_start = 0;
    for segment in segments {
        assert!(segment.range.end().0 <= WEEKLY_MS, "{segment:?}");
        assert!(segment.range.start().0 >= last_start, "in time order");
        last_start = segment.range.start().0;
        assert!(!segment.text.is_empty());
        assert_eq!(segment.language, Some(LangTag::parse("en").unwrap()));
    }
    let text: Vec<_> = segments.iter().map(|s| s.text.as_str()).collect();
    let text = text.join(" ");
    assert!(text.contains("weekly sync"), "{text}");
    assert!(text.contains("Maria"), "{text}");
    assert_eq!(sink.progress.first(), Some(&0.0));
    assert_eq!(sink.progress.last(), Some(&1.0));
}

#[test]
#[ignore = "calls Apple Speech; run with DORY_APPLE_LIVE=1 and --ignored"]
fn cancel_stops_within_two_seconds() {
    if !live() {
        return;
    }
    let speech = AppleSpeech::load();
    let token = CancelToken::new();
    let engine = speech.open(&english(), &token).expect("open");
    let (first, saw_segment) = mpsc::channel();
    let worker = {
        let token = token.clone();
        thread::spawn(move || {
            let clip = clip(TWO_HOURS, TWO_HOURS_MS);
            let mut sink = Collect {
                first: Some(first),
                ..Collect::default()
            };
            let result = engine.transcribe(&request(&clip), &mut sink, &token);
            (result, Instant::now(), sink.segments.len())
        })
    };
    saw_segment
        .recv_timeout(Duration::from_secs(60))
        .expect("Apple started transcribing");
    thread::sleep(Duration::from_secs(2));
    let cancelled_at = Instant::now();
    token.cancel();
    let (result, returned_at, segments) = worker.join().expect("worker");
    let latency = returned_at.duration_since(cancelled_at);
    eprintln!(
        "cancel_latency_ms={} segments_before_cancel={segments}",
        latency.as_millis()
    );
    assert!(
        matches!(result, Err(EngineError::Cancelled)),
        "{:?}",
        result.err()
    );
    assert!(latency < Duration::from_secs(2), "took {latency:?}");
}
