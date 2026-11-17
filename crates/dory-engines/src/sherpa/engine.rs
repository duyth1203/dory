//! One loaded sherpa-onnx recognizer behind the `Transcriber` port.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use dory_core::domain::{Fraction, LangTag, LanguagePref, Ms, TimeRange};
use dory_core::ports::{
    CancelToken, EngineError, RawSegment, SegmentSink, TranscribeOutcome, TranscribeRequest,
    Transcriber,
};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineRecognizerResult,
    OfflineTransducerModelConfig, OfflineWhisperModelConfig, Wave,
};

use super::catalog::{Layout, Model};

/// A pause this long between words starts a new segment.
const GAP_SECS: f32 = 1.0;

pub(super) fn recognizer_config(
    layout: Layout,
    dir: &Path,
    recipe: Recipe,
) -> OfflineRecognizerConfig {
    let language = recipe.whisper_language;
    let path = |file: &str| Some(dir.join(file).to_string_lossy().into_owned());
    let mut config = OfflineRecognizerConfig::default();
    let model = &mut config.model_config;
    match layout {
        Layout::Parakeet => {
            model.transducer = OfflineTransducerModelConfig {
                encoder: path("encoder.int8.onnx"),
                decoder: path("decoder.int8.onnx"),
                joiner: path("joiner.int8.onnx"),
            };
            model.tokens = path("tokens.txt");
            model.model_type = Some("nemo_transducer".into());
        }
        Layout::WhisperTurbo => {
            model.whisper = OfflineWhisperModelConfig {
                encoder: path("turbo-encoder.int8.onnx"),
                decoder: path("turbo-decoder.int8.onnx"),
                language: language.map(str::to_owned),
                task: Some("transcribe".into()),
                tail_paddings: -1,
                ..Default::default()
            };
            model.tokens = path("turbo-tokens.txt");
        }
    }
    model.num_threads = i32::from(recipe.threads);
    model.provider = Some("cpu".into());
    config
}

/// What a recognizer is built for. onnxruntime sizes its thread pool when the recognizer is
/// built, and only Whisper takes the language at load time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Recipe {
    pub whisper_language: Option<&'static str>,
    pub threads: u8,
}

/// One built recognizer and its recipe. A request with another recipe drops it before building
/// the next, so two never sit in memory together.
pub(super) struct Built<R>(Option<(Recipe, R)>);

impl<R> Built<R> {
    pub(super) fn empty() -> Self {
        Self(None)
    }

    pub(super) fn get<E>(
        &mut self,
        recipe: Recipe,
        build: impl FnOnce(Recipe) -> Result<R, E>,
    ) -> Result<&mut R, E> {
        if self.0.as_ref().is_none_or(|(built, _)| *built != recipe) {
            self.0 = None;
            self.0 = Some((recipe, build(recipe)?));
        }
        Ok(&mut self.0.as_mut().expect("built above").1)
    }
}

/// Builds the recognizer on the first window, with that window's thread count, so a light first
/// window does not build twice.
pub(super) struct SherpaEngine {
    model: Model,
    dir: PathBuf,
    built: Mutex<Built<OfflineRecognizer>>,
}

impl SherpaEngine {
    pub(super) fn open(model: Model, dir: PathBuf) -> SherpaEngine {
        SherpaEngine {
            model,
            dir,
            built: Mutex::new(Built::empty()),
        }
    }
}

/// The code the engine takes for `wanted`, or `None` when the model does not speak it.
pub(super) fn engine_code(model: &Model, wanted: &LangTag) -> Option<&'static str> {
    model
        .languages
        .iter()
        .find(|(tag, _)| LangTag::parse(tag).is_ok_and(|tag| tag.covers(wanted)))
        .map(|(_, code)| *code)
}

fn load(model: &Model, dir: &Path, recipe: Recipe) -> Result<OfflineRecognizer, EngineError> {
    OfflineRecognizer::create(&recognizer_config(model.layout, dir, recipe)).ok_or_else(|| {
        EngineError::Other {
            message: format!("sherpa-onnx could not load {}", model.display_name),
            source: None,
        }
    })
}

impl Transcriber for SherpaEngine {
    fn transcribe(
        &self,
        req: &TranscribeRequest<'_>,
        sink: &mut dyn SegmentSink,
        cancel: &CancelToken,
    ) -> Result<TranscribeOutcome, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let code = match &req.language {
            LanguagePref::Auto => None,
            LanguagePref::Pinned(tag) => Some(
                engine_code(&self.model, tag)
                    .ok_or_else(|| EngineError::UnsupportedLanguage(tag.clone()))?,
            ),
        };
        let path = req.clip.path.to_str().ok_or_else(|| {
            EngineError::InputRejected(format!("{} is not UTF-8", req.clip.path.display()))
        })?;
        let wave = Wave::read(path).ok_or_else(|| {
            EngineError::InputRejected(format!("cannot read {path} as a mono WAV"))
        })?;
        let rate = wave.sample_rate();
        if rate <= 0 {
            return Err(EngineError::InputRejected(format!(
                "{path} has no sample rate"
            )));
        }
        let clip_len = Ms(wave.samples().len() as u64 * 1000 / rate as u64);
        sink.progress(Fraction::new(0.0));

        let recipe = Recipe {
            whisper_language: code.filter(|_| self.model.layout == Layout::WhisperTurbo),
            threads: req.threads.max(1),
        };
        let mut built = self.built.lock().unwrap_or_else(PoisonError::into_inner);
        let recognizer = built.get(recipe, |r| load(&self.model, &self.dir, r))?;
        let stream = recognizer.create_stream();
        stream.accept_waveform(rate, wave.samples());
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        recognizer.decode(&stream);
        let result = stream.get_result().ok_or_else(|| EngineError::Other {
            message: "sherpa-onnx returned no result".into(),
            source: None,
        })?;
        drop(built);

        for (range, text) in segments(&result, clip_len) {
            sink.segment(RawSegment {
                range,
                text,
                speaker: None,
                language: None,
            });
        }
        sink.progress(Fraction::new(1.0));
        Ok(TranscribeOutcome {
            detected_language: detected_language(self.model.layout, &result),
        })
    }
}

/// The language the model heard in the window. Whisper reports one for every window. Parakeet
/// reports none, and an empty or unparseable code is none too.
pub(super) fn detected_language(
    layout: Layout,
    result: &OfflineRecognizerResult,
) -> Option<LangTag> {
    match layout {
        Layout::WhisperTurbo => LangTag::parse(result.lang.trim()).ok(),
        Layout::Parakeet => None,
    }
}

/// Splits at sentence punctuation and long pauses when the model gave token times. Otherwise the
/// whole clip is one segment.
pub(super) fn segments(result: &OfflineRecognizerResult, clip_len: Ms) -> Vec<(TimeRange, String)> {
    let whole = || {
        let text = result.text.trim();
        let range = TimeRange::new(Ms(0), clip_len).expect("0 is never after the clip end");
        (!text.is_empty())
            .then(|| (range, text.to_owned()))
            .into_iter()
            .collect()
    };
    let tokens = &result.tokens;
    let Some(starts) = result
        .timestamps
        .as_deref()
        .filter(|t| !tokens.is_empty() && t.len() == tokens.len())
    else {
        return whole();
    };
    let durations = result
        .durations
        .as_deref()
        .filter(|d| d.len() == tokens.len());
    let word_start = |t: &str| t.starts_with(['▁', ' ']);
    let ms = |secs: f32| Ms((secs.max(0.0) * 1000.0).round() as u64).min(clip_len);

    let mut out = Vec::new();
    let mut text = String::new();
    let mut start = 0.0;
    let mut end = 0.0_f32;
    let mut flush = |text: &mut String, start: f32, end: f32| {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let (a, b) = (ms(start), ms(end));
            if let Some(range) = TimeRange::new(a, b.max(a)) {
                out.push((range, trimmed.to_owned()));
            }
        }
        text.clear();
    };
    for (i, token) in tokens.iter().enumerate() {
        let at = starts[i];
        if !text.is_empty() && word_start(token) && at - end >= GAP_SECS {
            flush(&mut text, start, end);
        }
        if text.is_empty() {
            start = at;
        }
        text.push_str(&token.replace('▁', " "));
        end = match durations {
            Some(d) => at + d[i],
            None => starts.get(i + 1).copied().unwrap_or(at),
        }
        .max(at);
        let sentence_end = token.trim_end().ends_with(['.', '?', '!']);
        if sentence_end && tokens.get(i + 1).is_none_or(|next| word_start(next)) {
            flush(&mut text, start, end);
        }
    }
    flush(&mut text, start, end);
    out
}
