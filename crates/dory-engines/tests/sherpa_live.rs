//! Runs every installed sherpa-onnx model on a real WAV.
//!
//! `DORY_SHERPA_MODELS=<dir with one folder per model> DORY_SHERPA_WAV=<16 kHz mono wav>
//! cargo test -p dory-engines --test sherpa_live -- --ignored --nocapture`
//!
//! `whisper_reports_the_language_it_heard` also takes `DORY_SHERPA_EXPECT_LANG`, the tag the clip is in.
//!
//! Set `DORY_SHERPA_LANG` to a tag such as `en` to pin the language instead of detecting it, and
//! `DORY_SHERPA_ONLY` to a model name to run just that one.

use std::path::PathBuf;
use std::time::Instant;

use dory_core::domain::{Channel, Fraction, LangTag, LanguagePref, Ms};
use dory_core::models::Availability;
use dory_core::ports::{
    Activity, CancelToken, Clip, EngineError, ModelProvider, RawSegment, SegmentSink,
    TranscribeRequest,
};
use dory_engines::Sherpa;

#[derive(Default)]
struct Collect(Vec<RawSegment>);

impl SegmentSink for Collect {
    fn progress(&mut self, _: Fraction) {}
    fn segment(&mut self, segment: RawSegment) {
        self.0.push(segment);
    }
}

#[test]
#[ignore = "needs DORY_SHERPA_MODELS and DORY_SHERPA_WAV"]
fn installed_models_transcribe_a_wav() {
    let models = PathBuf::from(std::env::var("DORY_SHERPA_MODELS").expect("DORY_SHERPA_MODELS"));
    let wav = PathBuf::from(std::env::var("DORY_SHERPA_WAV").expect("DORY_SHERPA_WAV"));
    let language = std::env::var("DORY_SHERPA_LANG").map_or(LanguagePref::Auto, |l| {
        LanguagePref::Pinned(LangTag::parse(&l).unwrap())
    });
    let sherpa = Sherpa::new(&models, Sherpa::RELEASES);
    let clip = Clip::new(
        wav,
        Channel::Mixed,
        Ms(0),
        Ms(0),
        Activity::Unmeasured,
        None,
    );
    let audio_secs = wav_secs(&clip.path);
    let only = std::env::var("DORY_SHERPA_ONLY").ok();
    let cancel = CancelToken::new();

    let mut ran = 0;
    for entry in sherpa.catalog() {
        if only.as_ref().is_some_and(|name| name != entry.id.name()) {
            continue;
        }
        if sherpa.availability(&entry.id) != Availability::Ready {
            println!("{}: not installed, skipped", entry.id);
            continue;
        }
        let loading = Instant::now();
        let engine = sherpa.open(&entry.id, &cancel).unwrap();
        let load_secs = loading.elapsed().as_secs_f32();
        let decoding = Instant::now();
        let mut sink = Collect::default();
        let request = TranscribeRequest {
            clip: &clip,
            language: language.clone(),
            diarize: false,
            hint: None,
            threads: 4,
        };
        match engine.transcribe(&request, &mut sink, &cancel) {
            Err(EngineError::UnsupportedLanguage(lang)) => {
                println!("{}: does not speak {lang}, skipped", entry.id);
                continue;
            }
            result => result.unwrap(),
        };
        let decode_secs = decoding.elapsed().as_secs_f32();
        println!(
            "{}: load {load_secs:.1} s, decode {decode_secs:.2} s for {audio_secs:.1} s of audio (RTFx {:.1})",
            entry.id,
            audio_secs / decode_secs
        );
        for s in &sink.0 {
            println!(
                "  [{:>6} - {:>6} ms] {}",
                s.range.start().0,
                s.range.end().0,
                s.text
            );
        }
        assert!(
            sink.0.iter().any(|s| !s.text.trim().is_empty()),
            "{} produced text",
            entry.id
        );
        ran += 1;
    }
    assert!(ran > 0, "no model is installed under {}", models.display());
}

#[test]
#[ignore = "needs DORY_SHERPA_MODELS with Whisper installed and DORY_SHERPA_WAV"]
fn whisper_reports_the_language_it_heard() {
    let models = PathBuf::from(std::env::var("DORY_SHERPA_MODELS").expect("DORY_SHERPA_MODELS"));
    let wav = PathBuf::from(std::env::var("DORY_SHERPA_WAV").expect("DORY_SHERPA_WAV"));
    let sherpa = Sherpa::new(&models, Sherpa::RELEASES);
    let clip = Clip::new(
        wav,
        Channel::Mixed,
        Ms(0),
        Ms(0),
        Activity::Unmeasured,
        None,
    );
    let cancel = CancelToken::new();
    let whisper = sherpa
        .catalog()
        .into_iter()
        .find(|e| {
            e.id.name().starts_with("whisper") && sherpa.availability(&e.id) == Availability::Ready
        })
        .expect("a Whisper model installed under DORY_SHERPA_MODELS");
    let engine = sherpa.open(&whisper.id, &cancel).unwrap();
    let mut sink = Collect::default();
    let request = TranscribeRequest {
        clip: &clip,
        language: LanguagePref::Auto,
        diarize: false,
        hint: None,
        threads: 4,
    };
    let outcome = engine.transcribe(&request, &mut sink, &cancel).unwrap();
    println!(
        "{}: heard {:?} in {} segments",
        whisper.id,
        outcome.detected_language,
        sink.0.len()
    );
    let heard = outcome.detected_language.expect("a language");
    if let Ok(want) = std::env::var("DORY_SHERPA_EXPECT_LANG") {
        assert_eq!(heard, LangTag::parse(&want).unwrap());
    }
}

/// Seconds of 16-bit mono audio, from the data chunk size.
fn wav_secs(path: &std::path::Path) -> f32 {
    let bytes = std::fs::read(path).unwrap();
    let data = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("a data chunk");
    let len = u32::from_le_bytes(bytes[data + 4..data + 8].try_into().unwrap());
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    len as f32 / 2.0 / rate as f32
}
