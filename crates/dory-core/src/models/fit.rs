use std::fmt;

use crate::domain::{LangSet, LangTag, LanguagePref, Ms};
use crate::ports::{Diarizer, LanguageModel, Transcriber};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KindTag {
    Stt,
    Llm,
    Diar,
}

impl KindTag {
    pub fn as_str(self) -> &'static str {
        match self {
            KindTag::Stt => "speech",
            KindTag::Llm => "summary",
            KindTag::Diar => "speakers",
        }
    }
}

impl fmt::Display for KindTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A kind of model. The bounds exist so generic types over a kind can derive their traits.
pub trait ModelKind:
    'static + Send + Sync + Copy + fmt::Debug + Eq + Ord + std::hash::Hash
{
    const TAG: KindTag;
    /// Whether the user may turn this kind off. Speech to text cannot be off.
    const OPTIONAL: bool;
    /// What `ModelProvider::open` returns.
    type Engine: ?Sized + Send + Sync;
    /// What an entry declares about itself, known before it loads.
    type Caps: Clone + fmt::Debug + Send + Sync;
    /// What a job needs from the model.
    type Needs: fmt::Debug + Send + Sync;
    /// The only place a capability is compared with a need.
    fn fit(caps: &Self::Caps, needs: &Self::Needs) -> Fit;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stt {}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Llm {}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Diar {}

/// How well a model suits a job. `No` rules it out. `Degraded` runs, with a known limitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Yes,
    Degraded(Mismatch),
    No(Mismatch),
}

impl Fit {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Fit::Yes => 0,
            Fit::Degraded(_) => 1,
            Fit::No(_) => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Mismatch {
    /// The language is Auto but the model needs one chosen up front.
    #[error("needs a language chosen, it cannot detect one")]
    AutoLanguageUnsupported,
    #[error("does not support this language")]
    LanguageUnsupported,
    /// A model that reads only some languages cannot be trusted with a meeting whose languages
    /// are unknown, because it may drop the parts it cannot read.
    #[error("needs your meeting languages set in Settings")]
    LanguagesUnknown,
    /// The model runs in one language chosen up front, and the meeting uses two or more.
    #[error("handles one language per meeting")]
    OneLanguagePerMeeting,
    #[error("cannot tell speakers apart")]
    NoDiarization,
    #[error("input is longer than the model accepts")]
    InputTooLong,
    /// A run sent the model real speech and it recognized no word, as Parakeet does on a language
    /// it was not trained on. Only a finished run can tell, so no fit rule returns it.
    #[error("heard speech but recognized no words")]
    NothingRecognized,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LangSupport {
    /// Handles many languages. `auto_detect` means it can run with `LanguagePref::Auto`.
    Multilingual { auto_detect: bool, tags: LangSet },
    /// Needs exactly one locale chosen before the run, like Apple's SpeechTranscriber.
    FixedLocale { tags: LangSet },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiarizationSupport {
    None,
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SttCaps {
    pub languages: LangSupport,
    pub diarization: DiarizationSupport,
    /// Engines that refuse long input. The core does not split for them, so a longer take is
    /// `Degraded`.
    pub max_input: Option<Ms>,
    /// The longest window the engine takes in one call. The core cuts each clip into windows at
    /// silence up to this length and checkpoints after each. `None` runs the whole clip in one call,
    /// for engines whose speaker ids only hold within one call.
    pub window: Option<Ms>,
    /// Asked for them, the engine returns one segment per sentence with its own time, so the core
    /// can see which speech a window left without text.
    pub sentence_times: bool,
    /// Audio the engine hears before each window, for an engine that transcribes the start of a
    /// window better once it has heard what came before. The core sends it with the window, never
    /// from before the clip, and keeps only the sentences centered inside the window, so
    /// checkpoints and resume stay per window. Zero sends each window alone.
    pub lead_in: Ms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiarizationNeed {
    Required,
    NotNeeded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SttNeeds {
    pub language: LanguagePref,
    /// With `LanguagePref::Auto`, the languages the meeting may use. A model must support all of
    /// them, so a meeting that mixes Tagalog and English never lands on an English-only model.
    /// Empty means no constraint.
    pub expected: Vec<LangTag>,
    pub diarization: DiarizationNeed,
    pub input: Ms,
}

impl ModelKind for Stt {
    const TAG: KindTag = KindTag::Stt;
    const OPTIONAL: bool = false;
    type Engine = dyn Transcriber;
    type Caps = SttCaps;
    type Needs = SttNeeds;

    fn fit(caps: &SttCaps, needs: &SttNeeds) -> Fit {
        // A fixed-locale model runs on `Auto` only in a meeting of one language, pinned for it.
        let (can_auto, tags) = match &caps.languages {
            LangSupport::Multilingual { auto_detect, tags } => (*auto_detect, tags),
            LangSupport::FixedLocale { tags } => (one_language(&needs.expected).is_some(), tags),
        };
        let fixed = matches!(caps.languages, LangSupport::FixedLocale { .. });
        match &needs.language {
            LanguagePref::Auto if !can_auto && fixed && !needs.expected.is_empty() => {
                return Fit::No(Mismatch::OneLanguagePerMeeting);
            }
            LanguagePref::Auto if !can_auto => return Fit::No(Mismatch::AutoLanguageUnsupported),
            LanguagePref::Auto if needs.expected.iter().any(|lang| !tags.supports(lang)) => {
                return Fit::No(Mismatch::LanguageUnsupported);
            }
            LanguagePref::Pinned(lang) if !tags.supports(lang) => {
                return Fit::No(Mismatch::LanguageUnsupported);
            }
            _ => {}
        }
        if needs.diarization == DiarizationNeed::Required
            && caps.diarization == DiarizationSupport::None
        {
            return Fit::Degraded(Mismatch::NoDiarization);
        }
        if caps.max_input.is_some_and(|max| needs.input > max) {
            return Fit::Degraded(Mismatch::InputTooLong);
        }
        Fit::Yes
    }
}

/// The language a speech model runs in. A fixed-locale model on `Auto` runs in the meeting's one
/// language, which is how it fits `Auto` at all. Every other model runs in `needs.language`.
pub fn run_language(caps: &SttCaps, needs: &SttNeeds) -> LanguagePref {
    match (&caps.languages, &needs.language) {
        (LangSupport::FixedLocale { .. }, LanguagePref::Auto) => {
            one_language(&needs.expected).map_or(LanguagePref::Auto, LanguagePref::Pinned)
        }
        _ => needs.language.clone(),
    }
}

/// The one language every expected tag shares: the tag itself when they are all the same, else
/// the bare primary language. `None` when the list is empty or names two languages.
fn one_language(expected: &[LangTag]) -> Option<LangTag> {
    let first = expected.first()?;
    if expected.iter().all(|tag| tag == first) {
        return Some(first.clone());
    }
    if expected.iter().all(|tag| tag.primary() == first.primary()) {
        return LangTag::parse(first.primary()).ok();
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmCaps {
    /// Token window. Summarization sizes its chunks from this.
    pub context_tokens: u32,
    pub languages: LangSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmNeeds {
    /// The meeting's main language, when known. The prompt asks for the summary in it.
    pub main: Option<LangTag>,
    /// Every language the meeting may use, each once. A model must read all of them. Empty means
    /// nothing is known, and only a model for any language fits.
    pub all: Vec<LangTag>,
}

impl LlmNeeds {
    /// `all` is `main`, the recording's pinned language, the meeting languages from Settings, and
    /// the languages the transcript detected, in that order, each once.
    pub fn new(
        main: Option<LangTag>,
        pinned: &LanguagePref,
        meeting: &[LangTag],
        detected: &[LangTag],
    ) -> Self {
        let pinned = match pinned {
            LanguagePref::Pinned(tag) => Some(tag),
            LanguagePref::Auto => None,
        };
        let mut all: Vec<LangTag> = Vec::new();
        for tag in main.iter().chain(pinned).chain(meeting).chain(detected) {
            if !all.contains(tag) {
                all.push(tag.clone());
            }
        }
        Self { main, all }
    }
}

impl ModelKind for Llm {
    const TAG: KindTag = KindTag::Llm;
    const OPTIONAL: bool = true;
    type Engine = dyn LanguageModel;
    type Caps = LlmCaps;
    type Needs = LlmNeeds;

    /// A model for any language fits every meeting. A model that reads only some languages needs
    /// the meeting's languages known, because with none set it may summarize the half it reads
    /// and drop the rest without refusing.
    fn fit(caps: &LlmCaps, needs: &LlmNeeds) -> Fit {
        match &caps.languages {
            LangSet::Any => Fit::Yes,
            LangSet::Only(_) if needs.all.is_empty() => Fit::No(Mismatch::LanguagesUnknown),
            LangSet::Only(_) if needs.all.iter().all(|lang| caps.languages.supports(lang)) => {
                Fit::Yes
            }
            LangSet::Only(_) => Fit::No(Mismatch::LanguageUnsupported),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiarCaps {
    pub max_speakers: Option<u8>,
}

/// A job cannot say how many speakers a meeting holds before diarization has run, so `fit` has
/// nothing to compare `DiarCaps::max_speakers` against and every diarizer fits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiarNeeds;

impl ModelKind for Diar {
    const TAG: KindTag = KindTag::Diar;
    const OPTIONAL: bool = true;
    type Engine = dyn Diarizer;
    type Caps = DiarCaps;
    type Needs = DiarNeeds;

    fn fit(_: &DiarCaps, _: &DiarNeeds) -> Fit {
        Fit::Yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn only(tags: &[&str]) -> LangSet {
        LangSet::Only(tags.iter().map(|t| tag(t)).collect())
    }

    fn pinned(s: &str) -> LanguagePref {
        LanguagePref::Pinned(tag(s))
    }

    fn needs(language: LanguagePref, diarization: DiarizationNeed, input: u64) -> SttNeeds {
        SttNeeds {
            language,
            expected: Vec::new(),
            diarization,
            input: Ms(input),
        }
    }

    fn caps(
        languages: LangSupport,
        diarization: DiarizationSupport,
        max_input: Option<u64>,
    ) -> SttCaps {
        SttCaps {
            languages,
            diarization,
            max_input: max_input.map(Ms),
            window: None,
            sentence_times: false,
            lead_in: Ms(0),
        }
    }

    fn whisper() -> LangSupport {
        LangSupport::Multilingual {
            auto_detect: true,
            tags: only(&["en", "fr", "vi"]),
        }
    }

    #[test]
    fn auto_needs_every_meeting_language() {
        let european = LangSupport::Multilingual {
            auto_detect: true,
            tags: only(&["en", "fr", "de"]),
        };
        let with = |expected: &[&str]| SttNeeds {
            expected: expected.iter().map(|t| tag(t)).collect(),
            ..needs(LanguagePref::Auto, DiarizationNeed::NotNeeded, 1)
        };
        let fit = |expected: &[&str]| {
            Stt::fit(
                &caps(european.clone(), DiarizationSupport::None, None),
                &with(expected),
            )
        };
        assert_eq!(fit(&[]), Fit::Yes, "no meeting languages set");
        assert_eq!(fit(&["en"]), Fit::Yes);
        assert_eq!(
            fit(&["tl", "en"]),
            Fit::No(Mismatch::LanguageUnsupported),
            "Tagalog and English meetings rule out a model without Tagalog"
        );
        let pinned_english = SttNeeds {
            expected: vec![tag("tl")],
            ..needs(pinned("en"), DiarizationNeed::NotNeeded, 1)
        };
        assert_eq!(
            Stt::fit(
                &caps(european, DiarizationSupport::None, None),
                &pinned_english
            ),
            Fit::Yes,
            "a recording pinned to one language ignores the meeting languages"
        );
    }

    #[test]
    fn stt_fit_table() {
        use DiarizationNeed::{NotNeeded, Required};
        use DiarizationSupport::{Native, None as NoDiar};
        use Fit::{Degraded, No, Yes};
        let apple = LangSupport::FixedLocale {
            tags: only(&["en-US", "fr-FR"]),
        };
        let no_auto = LangSupport::Multilingual {
            auto_detect: false,
            tags: LangSet::Any,
        };
        let cases = [
            (
                "auto on a detecting model",
                caps(whisper(), NoDiar, None),
                needs(LanguagePref::Auto, NotNeeded, 1),
                Yes,
            ),
            (
                "pinned and supported",
                caps(whisper(), NoDiar, None),
                needs(pinned("fr"), NotNeeded, 1),
                Yes,
            ),
            (
                "regional pin on bare tag",
                caps(whisper(), NoDiar, None),
                needs(pinned("en-GB"), NotNeeded, 1),
                Yes,
            ),
            (
                "pinned but unsupported",
                caps(whisper(), NoDiar, None),
                needs(pinned("de"), NotNeeded, 1),
                No(Mismatch::LanguageUnsupported),
            ),
            (
                "fixed locale with auto",
                caps(apple.clone(), NoDiar, None),
                needs(LanguagePref::Auto, NotNeeded, 1),
                No(Mismatch::AutoLanguageUnsupported),
            ),
            (
                "fixed locale with its locale",
                caps(apple.clone(), NoDiar, None),
                needs(pinned("en-US"), NotNeeded, 1),
                Yes,
            ),
            (
                "fixed locale with another region",
                caps(apple, NoDiar, None),
                needs(pinned("en-GB"), NotNeeded, 1),
                No(Mismatch::LanguageUnsupported),
            ),
            (
                "multilingual without auto detect",
                caps(no_auto, NoDiar, None),
                needs(LanguagePref::Auto, NotNeeded, 1),
                No(Mismatch::AutoLanguageUnsupported),
            ),
            (
                "diarization required, none offered",
                caps(whisper(), NoDiar, None),
                needs(LanguagePref::Auto, Required, 1),
                Degraded(Mismatch::NoDiarization),
            ),
            (
                "diarization required, native",
                caps(whisper(), Native, None),
                needs(LanguagePref::Auto, Required, 1),
                Yes,
            ),
            (
                "input longer than max",
                caps(whisper(), NoDiar, Some(1000)),
                needs(LanguagePref::Auto, NotNeeded, 1001),
                Degraded(Mismatch::InputTooLong),
            ),
            (
                "input equal to max",
                caps(whisper(), NoDiar, Some(1000)),
                needs(LanguagePref::Auto, NotNeeded, 1000),
                Yes,
            ),
            (
                "hard mismatch beats degradation",
                caps(whisper(), NoDiar, Some(10)),
                needs(pinned("de"), Required, 99),
                No(Mismatch::LanguageUnsupported),
            ),
            (
                "diarization reported before length",
                caps(whisper(), NoDiar, Some(10)),
                needs(LanguagePref::Auto, Required, 99),
                Degraded(Mismatch::NoDiarization),
            ),
        ];
        for (name, caps, needs, want) in cases {
            assert_eq!(Stt::fit(&caps, &needs), want, "{name}");
        }
    }

    fn llm_caps(languages: LangSet) -> LlmCaps {
        LlmCaps {
            context_tokens: 4096,
            languages,
        }
    }

    fn llm_needs(all: &[&str]) -> LlmNeeds {
        LlmNeeds {
            main: all.first().map(|t| tag(t)),
            all: all.iter().map(|t| tag(t)).collect(),
        }
    }

    #[test]
    fn an_llm_must_read_every_known_language() {
        let english = llm_caps(only(&["en"]));
        assert_eq!(Llm::fit(&english, &llm_needs(&["en"])), Fit::Yes);
        assert_eq!(Llm::fit(&english, &llm_needs(&["en-US"])), Fit::Yes);
        assert_eq!(
            Llm::fit(&english, &llm_needs(&["en", "tl"])),
            Fit::No(Mismatch::LanguageUnsupported),
            "a Taglish meeting rules out an English-only model even when English comes first"
        );
        assert_eq!(
            Llm::fit(&english, &llm_needs(&["vi"])),
            Fit::No(Mismatch::LanguageUnsupported)
        );
        assert_eq!(
            Llm::fit(&llm_caps(LangSet::Any), &llm_needs(&["en", "tl", "vi"])),
            Fit::Yes,
            "a model for any language fits every meeting"
        );
    }

    #[test]
    fn a_limited_llm_needs_known_languages() {
        assert_eq!(
            Llm::fit(&llm_caps(only(&["en"])), &llm_needs(&[])),
            Fit::No(Mismatch::LanguagesUnknown)
        );
        assert_eq!(
            Llm::fit(&llm_caps(LangSet::Any), &llm_needs(&[])),
            Fit::Yes,
            "a model for any language fits when nothing is known"
        );
    }

    #[test]
    fn llm_needs_gather_every_language_once() {
        let needs = LlmNeeds::new(
            Some(tag("en")),
            &pinned("fr"),
            &[tag("en"), tag("tl")],
            &[tag("en"), tag("vi")],
        );
        assert_eq!(needs.main, Some(tag("en")));
        assert_eq!(needs.all, vec![tag("en"), tag("fr"), tag("tl"), tag("vi")]);
        assert_eq!(
            LlmNeeds::new(None, &LanguagePref::Auto, &[], &[]).all,
            Vec::<LangTag>::new()
        );
    }

    fn fixed_english() -> SttCaps {
        caps(
            LangSupport::FixedLocale {
                tags: only(&["en-US", "en-GB"]),
            },
            DiarizationSupport::None,
            None,
        )
    }

    fn expecting(expected: &[&str]) -> SttNeeds {
        SttNeeds {
            expected: expected.iter().map(|t| tag(t)).collect(),
            ..needs(LanguagePref::Auto, DiarizationNeed::NotNeeded, 1)
        }
    }

    #[test]
    fn a_fixed_locale_row_fits_auto_for_one_language() {
        let fit = |expected: &[&str]| Stt::fit(&fixed_english(), &expecting(expected));
        assert_eq!(fit(&["en"]), Fit::Yes);
        assert_eq!(fit(&["en-GB"]), Fit::Yes);
        assert_eq!(
            fit(&["en-US", "en-GB"]),
            Fit::Yes,
            "two regions of one language"
        );
        assert_eq!(
            fit(&["fr"]),
            Fit::No(Mismatch::LanguageUnsupported),
            "one language the row does not cover"
        );
        assert_eq!(
            fit(&[]),
            Fit::No(Mismatch::AutoLanguageUnsupported),
            "no meeting languages set"
        );
    }

    #[test]
    fn a_fixed_locale_row_never_fits_two_languages() {
        let any_locale = caps(
            LangSupport::FixedLocale { tags: LangSet::Any },
            DiarizationSupport::None,
            None,
        );
        for caps in [fixed_english(), any_locale] {
            assert_eq!(
                Stt::fit(&caps, &expecting(&["en", "tl"])),
                Fit::No(Mismatch::OneLanguagePerMeeting),
                "{caps:?}"
            );
        }
    }

    #[test]
    fn run_language_pins_the_one_language() {
        let run = |expected: &[&str]| run_language(&fixed_english(), &expecting(expected));
        assert_eq!(run(&["en"]), pinned("en"));
        assert_eq!(run(&["en-GB"]), pinned("en-GB"));
        assert_eq!(run(&["en-US", "en-GB"]), pinned("en"));
        let detecting = caps(whisper(), DiarizationSupport::None, None);
        assert_eq!(
            run_language(&detecting, &expecting(&["en"])),
            LanguagePref::Auto,
            "a model that detects the language runs on Auto"
        );
        let pinned_french = SttNeeds {
            expected: vec![tag("en")],
            ..needs(pinned("fr"), DiarizationNeed::NotNeeded, 1)
        };
        assert_eq!(run_language(&fixed_english(), &pinned_french), pinned("fr"));
    }

    #[test]
    fn diar_fit_does_not_look_at_the_speaker_limit() {
        for max_speakers in [None, Some(1), Some(2), Some(u8::MAX)] {
            assert_eq!(
                Diar::fit(&DiarCaps { max_speakers }, &DiarNeeds),
                Fit::Yes,
                "{max_speakers:?}"
            );
        }
    }

    #[test]
    fn only_speech_to_text_is_required() {
        assert_eq!(
            [Stt::OPTIONAL, Llm::OPTIONAL, Diar::OPTIONAL],
            [false, true, true]
        );
    }
}
