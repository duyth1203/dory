//! Fake engines and capture for lanes and demos. The engines come from `dory_core::testing`.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dory_core::capture::FrameSink;
use dory_core::domain::{Channel, LangSet, LangTag};
use dory_core::models::{DiarizationSupport, LangSupport, Llm, Stt};
use dory_core::ports::ModelProvider;
use dory_core::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, CaptureHandle, CaptureSpec, InputDevice,
    Source,
};
use dory_core::testing::{
    FakeLanguageModel, FakeLlmProvider, FakeSttProvider, FakeTranscriber, Refusal, llm_entry,
    stt_entry,
};
use thiserror::Error;

use crate::{DATA_DIR_VAR, Providers};

pub const FAKE_STT: &str = "fake/stt";
pub const FAKE_LLM: &str = "fake/llm";
/// A summary model that reads only English, with a small window.
pub const FAKE_EN_ONLY_LLM: &str = "fake/en-only-llm";
/// A speech model that needs its one locale, English, chosen up front.
pub const FAKE_EN_LOCALE_STT: &str = "fake/en-locale-stt";

/// `reject-language` or `busy:<n>`: how `fake/en-only-llm` fails. Setting it also ranks that model
/// first, so `Auto` tries it before `fake/llm`.
pub const FAKE_LLM_VAR: &str = "DORY_FAKE_LLM";
/// Comma-separated tags the fake transcriber labels its segments with in turn, such as `en,tl`.
pub const FAKE_STT_LANGS_VAR: &str = "DORY_FAKE_STT_LANGS";
/// `limited` keeps only the two limited rows in the fake catalogs. `locale-first` ranks the
/// one-locale speech row above `fake/stt`, as Apple Speech ranks above Whisper.
pub const FAKE_ROWS_VAR: &str = "DORY_FAKE_ROWS";

/// Put in front of every line the fake engines translate.
pub const TRANSLATED: &str = "[fake translation] ";
const SUMMARY: &str = "## Fake summary\n- The fake engines wrote this.";
const EN_ONLY_SUMMARY: &str = "## English-only summary\n- The English-only fake model wrote this.";
const CHUNK: Duration = Duration::from_millis(100);
const TONE_AMPLITUDE: f32 = 0.25;

/// What the fake summary model answers a translation request with: every numbered line, marked.
fn translate(user: &str) -> String {
    user.lines()
        .map(|line| match line.split_once("] ") {
            Some((n, text)) => format!("{n}] {TRANSLATED}{text}"),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fake summary model that writes `summary` and translates what it is asked to.
fn summarizer(summary: &'static str) -> FakeLanguageModel {
    FakeLanguageModel::answering(move |system, user| {
        if system.contains("You translate") {
            translate(user)
        } else {
            summary.to_owned()
        }
    })
}

/// What the `DORY_FAKE_*` variables ask of the fake engines. The default changes nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Knobs {
    /// How `fake/en-only-llm` refuses, and for how many calls.
    llm: Option<(Refusal, usize)>,
    stt_languages: Vec<LangTag>,
    limited: bool,
    locale_first: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum KnobError {
    #[error("{0} changes the fake engines, so it needs {DATA_DIR_VAR} set to a throwaway folder")]
    NeedsDataDir(&'static str),
    #[error("{FAKE_LLM_VAR} takes reject-language or busy:<n>, not {0:?}")]
    Llm(String),
    #[error("{FAKE_STT_LANGS_VAR} takes comma-separated language tags, not {0:?}")]
    Languages(String),
    #[error("{FAKE_ROWS_VAR} takes limited or locale-first, not {0:?}")]
    Rows(String),
}

/// Reads the knobs from the environment. Like `DORY_MACHINE`, they never reach the default
/// library.
pub(crate) fn knobs_from_env() -> Result<Knobs, KnobError> {
    let data_dir_set = std::env::var_os(DATA_DIR_VAR).is_some_and(|d| !d.is_empty());
    knobs(|name| std::env::var(name).ok(), data_dir_set)
}

pub(crate) fn knobs(
    var: impl Fn(&str) -> Option<String>,
    data_dir_set: bool,
) -> Result<Knobs, KnobError> {
    let mut knobs = Knobs::default();
    for name in [FAKE_LLM_VAR, FAKE_STT_LANGS_VAR, FAKE_ROWS_VAR] {
        let Some(value) = var(name) else { continue };
        if !data_dir_set {
            return Err(KnobError::NeedsDataDir(name));
        }
        let value = value.trim();
        match name {
            FAKE_LLM_VAR => {
                knobs.llm = Some(match value.split_once(':') {
                    None if value == "reject-language" => (Refusal::Language("und"), usize::MAX),
                    Some(("busy", n)) => (
                        Refusal::Busy,
                        n.parse().map_err(|_| KnobError::Llm(value.into()))?,
                    ),
                    _ => return Err(KnobError::Llm(value.into())),
                });
            }
            FAKE_STT_LANGS_VAR => {
                knobs.stt_languages = value
                    .split(',')
                    .map(|t| LangTag::parse(t.trim()))
                    .collect::<Result<_, _>>()
                    .map_err(|_| KnobError::Languages(value.into()))?;
            }
            _ if value == "limited" => knobs.limited = true,
            _ if value == "locale-first" => knobs.locale_first = true,
            _ => return Err(KnobError::Rows(value.into())),
        }
    }
    Ok(knobs)
}

pub(crate) fn providers(knobs: &Knobs) -> Providers {
    let mic = [
        (0, 900, 0, "Hello from the fake microphone."),
        (2_000, 2_800, 0, "Wrapping up."),
    ];
    let system = [(1_000, 1_900, 1, "Hello from the fake system audio.")];
    let mixed = [
        (0, 900, 1, "Hello from the fake mixed audio."),
        (1_000, 1_900, 2, "Second fake speaker."),
    ];
    let transcriber = Arc::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &mic)
            .with_segments(Channel::System, &system)
            .with_segments(Channel::Mixed, &mixed)
            .detecting("en")
            .speaking(&knobs.stt_languages),
    );
    let english = || LangSet::Only(vec![LangTag::parse("en").expect("a valid tag")]);
    let mut en_locale = stt_entry(FAKE_EN_LOCALE_STT, DiarizationSupport::None);
    en_locale.caps.languages = LangSupport::FixedLocale { tags: english() };
    en_locale.rank = if knobs.locale_first { 20 } else { -1 };
    let mut en_only = llm_entry(FAKE_EN_ONLY_LLM);
    en_only.caps.context_tokens = 4096;
    en_only.caps.languages = english();
    // The limited rows rank below the full ones, and `fake/en-only-llm` rises only when a knob asks,
    // so `Auto` picks what it picked before.
    en_only.rank = if knobs.llm.is_some() { 1 } else { -1 };
    let mut stt_entries = vec![stt_entry(FAKE_STT, DiarizationSupport::Native), en_locale];
    let mut llm_entries = vec![llm_entry(FAKE_LLM)];
    if knobs.limited {
        stt_entries.remove(0);
        llm_entries.clear();
    }
    let stt: Arc<dyn ModelProvider<Stt>> =
        Arc::new(FakeSttProvider::new("fake", stt_entries, transcriber));
    let llm: Arc<dyn ModelProvider<Llm>> = Arc::new(FakeLlmProvider::new(
        "fake",
        llm_entries,
        Arc::new(summarizer(SUMMARY)),
    ));
    let en_only_model = match knobs.llm {
        Some((refusal, calls)) => summarizer(EN_ONLY_SUMMARY).refusing(refusal, calls),
        None => summarizer(EN_ONLY_SUMMARY),
    };
    let en_only: Arc<dyn ModelProvider<Llm>> = Arc::new(FakeLlmProvider::new(
        "fake",
        vec![en_only],
        Arc::new(en_only_model),
    ));
    Providers {
        stt: vec![stt],
        llm: vec![llm, en_only],
    }
}

/// A capture backend that plays a tone on every channel in real time until it is stopped. It
/// stands in for `dory_core::testing::FakeCapture`, whose script thread ignores `stop`, so a
/// pause, an undo, or a stop mid-clip would block until the script ended.
pub(crate) struct ToneCapture {
    rate_hz: u32,
}

impl ToneCapture {
    pub fn new(rate_hz: u32) -> Self {
        Self { rate_hz }
    }
}

impl CaptureBackend for ToneCapture {
    fn probe(&self) -> CaptureCaps {
        CaptureCaps {
            microphone: Access::Granted,
            system_audio: Access::Granted,
            unsupported: None,
        }
    }

    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        Ok(vec![InputDevice {
            uid: "fake-mic".into(),
            name: "Fake Microphone".into(),
            is_default: true,
        }])
    }

    fn request_access(&self, _: Source) -> Result<Access, CaptureError> {
        Ok(Access::Granted)
    }

    fn start(
        &self,
        spec: &CaptureSpec,
        mut sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let channels = spec.layout.channel_count();
        let rate_hz = self.rate_hz;
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let chunk_frames = u64::from(rate_hz / 10);
            let mut frames = 0u64;
            let mut next = Instant::now();
            loop {
                let mut samples = Vec::with_capacity(chunk_frames as usize * channels);
                for frame in frames..frames + chunk_frames {
                    let at = frame as f32 / rate_hz as f32;
                    for channel in 0..channels {
                        let hz = 440.0 + 110.0 * channel as f32;
                        samples.push(TONE_AMPLITUDE * (std::f32::consts::TAU * hz * at).sin());
                    }
                }
                sink.push(&samples);
                frames += chunk_frames;
                next += CHUNK;
                match stopped.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Ok(Box::new(ToneHandle { stop, thread }))
    }
}

struct ToneHandle {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl CaptureHandle for ToneHandle {
    fn stop(self: Box<Self>) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::LanguagePref;
    use dory_core::models::{
        Availability, Choice, DiarizationNeed, LlmNeeds, ModelKind, ModelStatus, Resolution,
        SttNeeds, resolve,
    };

    use super::*;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs.to_vec();
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    #[test]
    fn fake_knobs_need_a_data_folder_and_name_their_values() {
        assert_eq!(knobs(vars(&[]), false), Ok(Knobs::default()));
        assert_eq!(
            knobs(
                vars(&[
                    (FAKE_LLM_VAR, "busy:2"),
                    (FAKE_STT_LANGS_VAR, "en, tl"),
                    (FAKE_ROWS_VAR, "limited"),
                ]),
                true
            ),
            Ok(Knobs {
                llm: Some((Refusal::Busy, 2)),
                stt_languages: vec![tag("en"), tag("tl")],
                limited: true,
                locale_first: false,
            })
        );
        assert_eq!(
            knobs(vars(&[(FAKE_LLM_VAR, "reject-language")]), true)
                .unwrap()
                .llm,
            Some((Refusal::Language("und"), usize::MAX))
        );
        assert_eq!(
            knobs(vars(&[(FAKE_ROWS_VAR, "limited")]), false),
            Err(KnobError::NeedsDataDir(FAKE_ROWS_VAR))
        );
        for (name, value) in [
            (FAKE_LLM_VAR, "busy:many"),
            (FAKE_LLM_VAR, "slow"),
            (FAKE_STT_LANGS_VAR, "en,!"),
            (FAKE_ROWS_VAR, "all"),
            (FAKE_ROWS_VAR, "locale"),
        ] {
            assert!(
                knobs(vars(&[(name, value)]), true).is_err(),
                "{name}={value}"
            );
        }
    }

    fn statuses<K: ModelKind>(providers: &[Arc<dyn ModelProvider<K>>]) -> Vec<ModelStatus<K>> {
        providers
            .iter()
            .flat_map(|p| p.catalog())
            .map(|entry| ModelStatus {
                entry,
                availability: Availability::Ready,
            })
            .collect()
    }

    fn auto_picks(knobs: &Knobs, meeting: &[&str]) -> (String, String) {
        let providers = providers(knobs);
        let meeting: Vec<LangTag> = meeting.iter().map(|t| tag(t)).collect();
        let stt_needs = SttNeeds {
            language: LanguagePref::Auto,
            expected: meeting.clone(),
            expected_guessed: false,
            diarization: DiarizationNeed::Required,
            input: dory_core::domain::Ms(1_000),
        };
        let llm_needs = LlmNeeds::new(None, &LanguagePref::Auto, &meeting, &[]);
        let stt = match resolve(&Choice::Auto, &statuses(&providers.stt), &stt_needs) {
            Resolution::Use(id) => id.to_string(),
            other => format!("{other:?}"),
        };
        let llm = match resolve(&Choice::Auto, &statuses(&providers.llm), &llm_needs) {
            Resolution::Use(id) => id.to_string(),
            other => format!("{other:?}"),
        };
        (stt, llm)
    }

    #[test]
    fn the_limited_rows_change_auto_only_when_a_knob_asks() {
        let pick = |stt: &str, llm: &str| (stt.to_owned(), llm.to_owned());
        assert_eq!(
            auto_picks(&Knobs::default(), &["en"]),
            pick(FAKE_STT, FAKE_LLM)
        );
        assert_eq!(auto_picks(&Knobs::default(), &[]), pick(FAKE_STT, FAKE_LLM));
        let ranked = Knobs {
            llm: Some((Refusal::Language("und"), usize::MAX)),
            ..Knobs::default()
        };
        assert_eq!(auto_picks(&ranked, &["en"]).1, FAKE_EN_ONLY_LLM);
        let limited = Knobs {
            limited: true,
            ..Knobs::default()
        };
        assert_eq!(
            auto_picks(&limited, &["en"]),
            pick(FAKE_EN_LOCALE_STT, FAKE_EN_ONLY_LLM)
        );
    }

    #[test]
    fn the_locale_first_row_wins_auto_only_on_a_language_the_user_chose() {
        let knobs = Knobs {
            locale_first: true,
            ..Knobs::default()
        };
        let providers = providers(&knobs);
        let pick = |guessed: bool| {
            let needs = SttNeeds {
                language: LanguagePref::Auto,
                expected: vec![tag("en")],
                expected_guessed: guessed,
                diarization: DiarizationNeed::NotNeeded,
                input: dory_core::domain::Ms(1_000),
            };
            match resolve(&Choice::Auto, &statuses(&providers.stt), &needs) {
                Resolution::Use(id) => id.to_string(),
                other => format!("{other:?}"),
            }
        };
        assert_eq!(pick(false), FAKE_EN_LOCALE_STT);
        assert_eq!(pick(true), FAKE_STT, "a guess does not hand it the meeting");
        assert!(knobs_from(&[(FAKE_ROWS_VAR, "locale-first")]).locale_first);
    }

    fn knobs_from(pairs: &[(&'static str, &'static str)]) -> Knobs {
        knobs(vars(pairs), true).unwrap()
    }
}
