//! Safe Rust over Apple's Foundation Models: what the Mac offers, one blocking completion, and
//! [`FoundationModels`], the summary provider over them.

use std::ffi::CString;
use std::sync::{Arc, Mutex, PoisonError};

use dory_core::domain::{LangSet, LangTag};
use dory_core::models::{
    self, Llm, LlmCaps, ModelAbout, ModelEntry, ModelId, ProviderId, Unavailable, UnavailableReason,
};
use dory_core::ports::{
    CancelToken, CompletionRequest, EngineError, LanguageModel, ModelProvider, ProvisionError,
    ProvisionSink, TextSink,
};

use super::ffi;

/// What `SystemLanguageModel.default` reports on this Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub availability: Availability,
    /// Apple's context window in tokens, or 0 when it reported none.
    pub context_size: u32,
    /// BCP 47 tags such as `en`, `en-GB`, and `zh-Hant`, read whatever the availability. The
    /// SDK puts no availability condition on `supportedLanguages`, but a Mac that cannot run the
    /// model may report none.
    pub languages: Vec<String>,
}

/// Whether `SystemLanguageModel.default` can generate on this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Ready to generate.
    Available,
    /// Apple Intelligence is turned off in System Settings.
    NotEnabled,
    /// This Mac cannot run Apple Intelligence.
    NotEligible,
    /// Apple is still downloading or preparing the model.
    NotReady,
    /// macOS is older than 26.4, which `contextSize` and `tokenCount` need.
    OsTooOld,
    /// A reason a later macOS added, which this build does not know.
    Unknown,
}

/// Why a completion failed. One kind per `LanguageModelSession.GenerationError` case, plus ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    ExceededContextWindowSize,
    AssetsUnavailable,
    GuardrailViolation,
    UnsupportedGuide,
    UnsupportedLanguageOrLocale,
    DecodingFailure,
    RateLimited,
    ConcurrentRequests,
    Refusal,
    /// A `GenerationError` case a later macOS added.
    OtherGenerationError,
    /// The cancel token fired, or `on_piece` returned false.
    Cancelled,
    /// Any other error, including text with a NUL byte that cannot cross the C ABI.
    Other,
}

/// A failed completion: the kind, and Apple's message for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for Failure {}

/// Reads Apple's availability, context window, and languages. Takes about 80 ms, so keep it off a
/// UI thread.
pub fn info() -> Info {
    let raw = ffi::fm_info();
    info_of(raw.state, raw.context_size, &raw.languages_csv)
}

/// Generates a reply in a fresh session and returns its full text. Each new piece of text goes to
/// `on_piece` on this thread, and returning false stops the completion as `Cancelled`. Blocks
/// until Apple finishes, `cancel` fires, or `on_piece` stops it. Completions run one at a time
/// across the process.
pub fn complete(
    system: &str,
    user: &str,
    max_out: u32,
    temperature: f32,
    mut on_piece: impl FnMut(&str) -> bool,
    cancel: &CancelToken,
) -> Result<String, Failure> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    let (Ok(system), Ok(user)) = (CString::new(system), CString::new(user)) else {
        return Err(Failure {
            kind: FailureKind::Other,
            message: "the prompt contains a NUL byte".to_owned(),
        });
    };
    let task = Arc::new(ffi::Task::new());
    let hook = Arc::clone(&task);
    cancel.on_cancel(move || hook.cancel());

    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let raw = ffi::fm_complete(&task, &system, &user, max_out, temperature, &mut on_piece);
    match failure_kind(raw.code) {
        None => Ok(raw.full),
        Some(kind) => Err(Failure {
            kind,
            message: raw.error,
        }),
    }
}

/// Apple's own count runs 37 to 378 ms a call and adds 8 tokens, so Dory estimates instead:
/// `ceil(utf8_bytes / 2.7)`, which stayed above Apple's count on every recorded transcript.
pub fn count_tokens(text: &str) -> usize {
    (text.len() * 10).div_ceil(27)
}

/// Apple's window on macOS 26.4 and 26.5, assumed when the Mac cannot report its own.
const APPLE_CONTEXT: u32 = 4096;
/// Below Gemma 4 E4B's 1 and Gemma 4 12B's 0, so a Ready Gemma wins and Apple stands in for it.
const RANK: i16 = -1;
/// System Settings, Apple Intelligence & Siri. The pane is `com.apple.Siri-Settings.extension`
/// (`SiriPreferenceExtension.appex`, which allows this URL scheme), and macOS 26 titles it Apple
/// Intelligence & Siri on a Mac that supports Apple Intelligence.
pub const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Siri-Settings.extension";

/// `ModelProvider<Llm>` over Apple's on-device model, one row: `apple/foundation-model`.
pub struct FoundationModels {
    caps: LlmCaps,
    /// The state a lane pretends, reported instead of probing.
    pretend: Option<Availability>,
}

impl FoundationModels {
    /// Reads Apple's context window and languages once, here (13 ms on the M3 Pro, 80 ms in the
    /// spike), so `catalog` does no IO. The registry keeps the catalog for the session, so caps
    /// read while Apple named no language stay empty until Dory restarts. `availability` probes
    /// on every call, so a model Apple finishes preparing later turns Ready, or asks for that
    /// restart when its languages were never read.
    pub fn from_this_mac() -> Self {
        Self {
            caps: caps(&info()),
            pretend: None,
        }
    }

    /// Reports `state` from every probe and never calls Apple's availability, for lanes. Its
    /// languages are never read, as on a Mac where Apple names none.
    pub fn pretending(state: Availability) -> Self {
        Self {
            caps: caps(&Info {
                availability: state,
                context_size: 0,
                languages: Vec::new(),
            }),
            pretend: Some(state),
        }
    }

    /// True when Apple named no language as the row was built, so it claims none.
    fn languages_unread(&self) -> bool {
        self.caps.languages == LangSet::Only(Vec::new())
    }
}

fn model_id() -> ModelId<Llm> {
    ModelId::new(provider_id(), "foundation-model").expect("a valid model id")
}

fn provider_id() -> ProviderId {
    ProviderId::parse("apple").expect("a valid provider id")
}

/// What the row can read: Apple's languages, never `LangSet::Any`. Apple drops what it cannot
/// read without refusing, so with no language named the row claims none and fits no meeting.
fn caps(info: &Info) -> LlmCaps {
    LlmCaps {
        context_tokens: match info.context_size {
            0 => APPLE_CONTEXT,
            size => size,
        },
        languages: LangSet::Only(
            info.languages
                .iter()
                .filter_map(|tag| LangTag::parse(tag).ok())
                .collect(),
        ),
    }
}

/// Apple turned Ready after the row was built with no languages, so it claims none until Dory
/// reads them again.
fn restart() -> models::Availability {
    models::Availability::Unavailable(Unavailable {
        reason: UnavailableReason::Preparing {
            detail: "Restart Dory to finish setting up Apple Intelligence".into(),
        },
        fix: None,
    })
}

/// Appendix E of `docs/plans/platform-ai.md`, with the sentences the user reads.
fn availability_of(state: &Availability) -> models::Availability {
    let (reason, fix) = match state {
        Availability::Available => return models::Availability::Ready,
        Availability::OsTooOld => (
            UnavailableReason::OsTooOld {
                required: "macOS 26.4".into(),
            },
            None,
        ),
        Availability::NotEnabled => (
            UnavailableReason::TurnedOff {
                detail: "Apple Intelligence is turned off".into(),
                settings_url: SETTINGS_URL.into(),
            },
            Some("Turn it on in System Settings, Apple Intelligence & Siri".into()),
        ),
        Availability::NotEligible => (
            UnavailableReason::NotEligible {
                detail: "This Mac can't run Apple Intelligence".into(),
            },
            None,
        ),
        Availability::NotReady => (
            UnavailableReason::Preparing {
                detail: "Apple is getting the model ready".into(),
            },
            None,
        ),
        Availability::Unknown => (
            UnavailableReason::NotEligible {
                detail: "Apple Intelligence isn't available on this Mac right now".into(),
            },
            None,
        ),
    };
    models::Availability::Unavailable(Unavailable { reason, fix })
}

/// The BCP 47 tag for a language Apple refused without naming it.
const UNDETERMINED: &str = "und";

/// Appendix E of `docs/plans/platform-ai.md`. Apple's own message follows each sentence, for logs.
fn engine_error(failure: Failure) -> EngineError {
    let said = |sentence: &str| match failure.message.trim() {
        "" => sentence.to_owned(),
        apple => format!("{sentence}: {apple}"),
    };
    match failure.kind {
        FailureKind::Cancelled => EngineError::Cancelled,
        FailureKind::RateLimited => EngineError::Busy(said(
            "Apple Intelligence is limiting how often Dory can ask",
        )),
        FailureKind::ConcurrentRequests => {
            EngineError::Busy(said("Apple Intelligence is busy with another request"))
        }
        FailureKind::UnsupportedLanguageOrLocale => EngineError::UnsupportedLanguage(
            LangTag::parse(UNDETERMINED).expect("a valid language tag"),
        ),
        FailureKind::ExceededContextWindowSize => EngineError::InputRejected(said(
            "the text is longer than Apple Intelligence can read at once",
        )),
        FailureKind::GuardrailViolation => {
            EngineError::InputRejected(said("Apple Intelligence's guardrails blocked this text"))
        }
        FailureKind::Refusal => {
            EngineError::InputRejected(said("Apple Intelligence declined to summarize this text"))
        }
        FailureKind::DecodingFailure => {
            EngineError::InputRejected(said("Apple Intelligence wrote a reply it could not decode"))
        }
        FailureKind::UnsupportedGuide => EngineError::InputRejected(said(
            "Apple Intelligence cannot write in the requested format",
        )),
        FailureKind::AssetsUnavailable => EngineError::ModelMissing(said(
            "Apple Intelligence's model is not on this Mac right now",
        )),
        FailureKind::OtherGenerationError | FailureKind::Other => EngineError::Other {
            message: said("Apple Intelligence failed"),
            source: None,
        },
    }
}

impl ModelProvider<Llm> for FoundationModels {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn catalog(&self) -> Vec<ModelEntry<Llm>> {
        vec![ModelEntry {
            id: model_id(),
            display_name: "Apple Intelligence".into(),
            about: ModelAbout {
                summary: "Apple's model, built into macOS, writes the summary on this Mac.".into(),
                pros: vec!["No download, private, and fast on short meetings".into()],
                cons: vec![
                    "23 languages and no Tagalog".into(),
                    "Meetings longer than a few minutes are summarized in parts and can lose \
                     action items"
                        .into(),
                ],
            },
            caps: self.caps.clone(),
            recommended: false,
            rank: RANK,
            disk_bytes: None,
            built_in: true,
        }]
    }

    fn availability(&self, model: &ModelId<Llm>) -> models::Availability {
        if *model != model_id() {
            return models::Availability::Unavailable(Unavailable {
                reason: UnavailableReason::NotEligible {
                    detail: format!("{model} is not an Apple model"),
                },
                fix: None,
            });
        }
        let state = self.pretend.unwrap_or_else(|| info().availability);
        match availability_of(&state) {
            models::Availability::Ready if self.languages_unread() => restart(),
            other => other,
        }
    }

    fn provision(
        &self,
        _: &ModelId<Llm>,
        _: &mut dyn ProvisionSink,
        _: &CancelToken,
    ) -> Result<(), ProvisionError> {
        Err(ProvisionError::NotProvisionable(
            "macOS downloads Apple Intelligence's model itself once it is turned on in System \
             Settings, Apple Intelligence & Siri"
                .into(),
        ))
    }

    fn open(
        &self,
        model: &ModelId<Llm>,
        cancel: &CancelToken,
    ) -> Result<Arc<dyn LanguageModel>, EngineError> {
        if *model != model_id() {
            return Err(EngineError::ModelMissing(model.to_string()));
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        Ok(Arc::new(FoundationModel))
    }
}

/// Holds nothing: each completion runs in a fresh Apple session.
struct FoundationModel;

impl LanguageModel for FoundationModel {
    fn count_tokens(&self, text: &str) -> u32 {
        u32::try_from(count_tokens(text)).unwrap_or(u32::MAX)
    }

    fn complete(
        &self,
        req: &CompletionRequest<'_>,
        sink: &mut dyn TextSink,
        cancel: &CancelToken,
    ) -> Result<String, EngineError> {
        complete(
            req.system,
            req.user,
            req.max_output_tokens,
            req.temperature,
            |piece| {
                sink.text(piece);
                true
            },
            cancel,
        )
        .map_err(engine_error)
    }
}

fn info_of(state: i32, context_size: u32, languages_csv: &str) -> Info {
    let availability = match state {
        ffi::FM_AVAILABLE => Availability::Available,
        ffi::FM_NOT_ENABLED => Availability::NotEnabled,
        ffi::FM_NOT_ELIGIBLE => Availability::NotEligible,
        ffi::FM_NOT_READY => Availability::NotReady,
        ffi::FM_OS_TOO_OLD => Availability::OsTooOld,
        _ => Availability::Unknown,
    };
    Info {
        availability,
        context_size,
        languages: languages_csv
            .split(',')
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect(),
    }
}

fn failure_kind(code: i32) -> Option<FailureKind> {
    Some(match code {
        ffi::OK => return None,
        ffi::CANCELLED => FailureKind::Cancelled,
        ffi::EXCEEDED_CONTEXT_WINDOW_SIZE => FailureKind::ExceededContextWindowSize,
        ffi::ASSETS_UNAVAILABLE => FailureKind::AssetsUnavailable,
        ffi::GUARDRAIL_VIOLATION => FailureKind::GuardrailViolation,
        ffi::UNSUPPORTED_GUIDE => FailureKind::UnsupportedGuide,
        ffi::UNSUPPORTED_LANGUAGE_OR_LOCALE => FailureKind::UnsupportedLanguageOrLocale,
        ffi::DECODING_FAILURE => FailureKind::DecodingFailure,
        ffi::RATE_LIMITED => FailureKind::RateLimited,
        ffi::CONCURRENT_REQUESTS => FailureKind::ConcurrentRequests,
        ffi::REFUSAL => FailureKind::Refusal,
        ffi::OTHER_GENERATION_ERROR => FailureKind::OtherGenerationError,
        ffi::INVALID_ARGUMENT => FailureKind::Other,
        _ => FailureKind::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dory_core::models::{Fit, LlmNeeds, ModelKind as _};

    #[test]
    fn the_estimate_never_undercounts() {
        // (UTF-8 bytes, Apple's `tokenCount`) from the P0 spike's `tokencount.txt`: the weekly
        // sync's 15 lines, and the first 200 lines of the two-hour planning meeting.
        for (bytes, apple) in [(885, 322), (14_256, 4_720)] {
            let estimate = count_tokens(&"x".repeat(bytes));
            assert!(
                estimate >= apple,
                "{bytes} bytes: estimate {estimate} < {apple}"
            );
        }
        assert_eq!(count_tokens(&"x".repeat(885)), 328);
        assert_eq!(count_tokens(&"x".repeat(14_256)), 5_280);
    }

    #[test]
    fn the_estimate_counts_bytes_not_characters() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("é"), 1);
        assert_eq!(count_tokens(&"日本語".repeat(9)), 30);
    }

    /// A provider that pretends `state` with caps read from `languages`.
    fn read(state: Availability, languages: &[&str]) -> FoundationModels {
        FoundationModels {
            caps: caps(&Info {
                availability: state,
                context_size: 4096,
                languages: languages.iter().map(|&tag| tag.to_owned()).collect(),
            }),
            pretend: Some(state),
        }
    }

    #[test]
    fn info_reads_every_apple_state() {
        assert_eq!(
            info_of(ffi::FM_AVAILABLE, 4096, "de,en,en-GB"),
            Info {
                availability: Availability::Available,
                context_size: 4096,
                languages: vec!["de".into(), "en".into(), "en-GB".into()],
            }
        );
        assert_eq!(
            info_of(ffi::FM_NOT_ENABLED, 4096, "de,en"),
            Info {
                availability: Availability::NotEnabled,
                context_size: 4096,
                languages: vec!["de".into(), "en".into()],
            },
            "languages Apple names while off are kept"
        );
        let state = |code| info_of(code, 0, "").availability;
        assert_eq!(state(ffi::FM_AVAILABLE), Availability::Available);
        assert_eq!(state(ffi::FM_NOT_ENABLED), Availability::NotEnabled);
        assert_eq!(state(ffi::FM_NOT_ELIGIBLE), Availability::NotEligible);
        assert_eq!(state(ffi::FM_NOT_READY), Availability::NotReady);
        assert_eq!(state(ffi::FM_OS_TOO_OLD), Availability::OsTooOld);
        assert_eq!(state(9), Availability::Unknown);
        assert!(info_of(ffi::FM_AVAILABLE, 4096, "").languages.is_empty());
    }

    #[test]
    fn each_swift_code_reads_as_its_own_failure() {
        let codes = [
            ffi::CANCELLED,
            ffi::EXCEEDED_CONTEXT_WINDOW_SIZE,
            ffi::ASSETS_UNAVAILABLE,
            ffi::GUARDRAIL_VIOLATION,
            ffi::UNSUPPORTED_GUIDE,
            ffi::UNSUPPORTED_LANGUAGE_OR_LOCALE,
            ffi::DECODING_FAILURE,
            ffi::RATE_LIMITED,
            ffi::CONCURRENT_REQUESTS,
            ffi::REFUSAL,
            ffi::OTHER_GENERATION_ERROR,
            20,
        ];
        let kinds: Vec<_> = codes.iter().map(|&c| failure_kind(c)).collect();
        for (i, kind) in kinds.iter().enumerate() {
            assert!(kind.is_some(), "code {} reads as success", codes[i]);
            assert!(
                !kinds[..i].contains(kind),
                "code {} repeats a kind",
                codes[i]
            );
        }
        assert_eq!(failure_kind(ffi::OK), None);
        assert_eq!(
            failure_kind(ffi::INVALID_ARGUMENT),
            Some(FailureKind::Other)
        );
    }

    #[test]
    fn availability_maps_every_apple_state() {
        let unavailable = |reason, fix: Option<&str>| {
            models::Availability::Unavailable(Unavailable {
                reason,
                fix: fix.map(str::to_owned),
            })
        };
        let not_eligible = |detail: &str| UnavailableReason::NotEligible {
            detail: detail.into(),
        };
        let cases = [
            (Availability::Available, models::Availability::Ready),
            (
                Availability::OsTooOld,
                unavailable(
                    UnavailableReason::OsTooOld {
                        required: "macOS 26.4".into(),
                    },
                    None,
                ),
            ),
            (
                Availability::NotEnabled,
                unavailable(
                    UnavailableReason::TurnedOff {
                        detail: "Apple Intelligence is turned off".into(),
                        settings_url: SETTINGS_URL.into(),
                    },
                    Some("Turn it on in System Settings, Apple Intelligence & Siri"),
                ),
            ),
            (
                Availability::NotEligible,
                unavailable(not_eligible("This Mac can't run Apple Intelligence"), None),
            ),
            (
                Availability::NotReady,
                unavailable(
                    UnavailableReason::Preparing {
                        detail: "Apple is getting the model ready".into(),
                    },
                    None,
                ),
            ),
            (
                Availability::Unknown,
                unavailable(
                    not_eligible("Apple Intelligence isn't available on this Mac right now"),
                    None,
                ),
            ),
        ];
        for (state, want) in cases {
            let provider = read(state, &["en"]);
            assert_eq!(provider.availability(&model_id()), want, "{state:?}");
        }
    }

    #[test]
    fn a_row_built_with_no_languages_fits_no_meeting() {
        let needs = |langs: &[&str]| LlmNeeds {
            main: None,
            spoken: langs.iter().map(|t| LangTag::parse(t).unwrap()).collect(),
        };
        for state in [
            Availability::Available,
            Availability::NotEnabled,
            Availability::NotReady,
            Availability::Unknown,
        ] {
            for provider in [FoundationModels::pretending(state), read(state, &[])] {
                let [row] = provider.catalog().try_into().expect("one row");
                assert_eq!(row.caps.languages, LangSet::Only(Vec::new()), "{state:?}");
                assert_eq!(row.caps.context_tokens, 4096, "{state:?}");
                for langs in [&["en"][..], &["tl"], &["en", "tl"], &[]] {
                    assert!(
                        matches!(Llm::fit(&row.caps, &needs(langs)), Fit::No(_)),
                        "{state:?} fits {langs:?} with no languages read"
                    );
                }
            }
        }
    }

    #[test]
    fn apple_turning_ready_after_launch_asks_for_a_restart() {
        let restart = models::Availability::Unavailable(Unavailable {
            reason: UnavailableReason::Preparing {
                detail: "Restart Dory to finish setting up Apple Intelligence".into(),
            },
            fix: None,
        });
        assert_eq!(
            FoundationModels::pretending(Availability::Available).availability(&model_id()),
            restart
        );
        assert_eq!(
            read(Availability::Available, &[]).availability(&model_id()),
            restart
        );
        assert_eq!(
            read(Availability::Available, &["en"]).availability(&model_id()),
            models::Availability::Ready,
            "languages read at launch need no restart"
        );
        assert!(
            matches!(
                FoundationModels::pretending(Availability::NotEnabled).availability(&model_id()),
                models::Availability::Unavailable(Unavailable {
                    reason: UnavailableReason::TurnedOff { .. },
                    ..
                })
            ),
            "turned off still asks to turn it on"
        );
    }

    fn variant(error: &EngineError) -> String {
        let debug = format!("{error:?}");
        debug
            .split(['(', ' '])
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn errors_map_to_engine_errors() {
        use dory_core::domain::FailureClass::{self, *};
        use dory_core::ports::Classify;
        let cases: [(FailureKind, &str, FailureClass); 12] = [
            (FailureKind::Cancelled, "Cancelled", Cancelled),
            (FailureKind::RateLimited, "Busy", Transient),
            (FailureKind::ConcurrentRequests, "Busy", Transient),
            (
                FailureKind::UnsupportedLanguageOrLocale,
                "UnsupportedLanguage",
                NeedsUser,
            ),
            (
                FailureKind::ExceededContextWindowSize,
                "InputRejected",
                Permanent,
            ),
            (FailureKind::GuardrailViolation, "InputRejected", Permanent),
            (FailureKind::Refusal, "InputRejected", Permanent),
            (FailureKind::DecodingFailure, "InputRejected", Permanent),
            (FailureKind::UnsupportedGuide, "InputRejected", Permanent),
            (FailureKind::AssetsUnavailable, "ModelMissing", NeedsUser),
            (FailureKind::OtherGenerationError, "Other", Permanent),
            (FailureKind::Other, "Other", Permanent),
        ];
        for (kind, want, class) in cases {
            let error = engine_error(Failure {
                kind,
                message: "apple says why".into(),
            });
            assert_eq!(variant(&error), want, "{kind:?}");
            assert_eq!(error.class(), class, "{kind:?}");
            let text = error.to_string();
            let keeps_apple = matches!(kind, FailureKind::Cancelled)
                || matches!(error, EngineError::UnsupportedLanguage(_))
                || text.contains("apple says why");
            assert!(keeps_apple, "{kind:?} dropped Apple's message: {text}");
        }
        match engine_error(Failure {
            kind: FailureKind::UnsupportedLanguageOrLocale,
            message: String::new(),
        }) {
            EngineError::UnsupportedLanguage(tag) => assert_eq!(tag.as_str(), "und"),
            other => panic!("expected UnsupportedLanguage, got {other:?}"),
        }
    }

    #[test]
    fn the_row_reads_its_caps_from_apple() {
        let ready = read(Availability::Available, &["de", "en", "zh-Hans", "!"]);
        let [row] = ready.catalog().try_into().expect("one row");
        assert_eq!(row.id.to_string(), "apple/foundation-model");
        assert_eq!(row.display_name, "Apple Intelligence");
        assert!(
            row.built_in,
            "macOS ships it, so there is nothing to install"
        );
        assert!(!row.recommended);
        assert!(row.rank < 0, "Gemma's rows rank 0 and 1, Apple ranks below");
        assert_eq!(row.caps.context_tokens, 4096);
        let tags = |list: &[&str]| -> Vec<LangTag> {
            list.iter().map(|t| LangTag::parse(t).unwrap()).collect()
        };
        assert_eq!(
            row.caps.languages,
            LangSet::Only(tags(&["de", "en", "zh-Hans"]))
        );
        assert!(!row.caps.languages.supports(&LangTag::parse("tl").unwrap()));

        let [off] = read(Availability::NotEnabled, &["de", "en"])
            .catalog()
            .try_into()
            .expect("one row");
        assert_eq!(
            off.caps.languages,
            LangSet::Only(tags(&["de", "en"])),
            "languages Apple names while off make the row's caps"
        );
    }

    #[test]
    fn the_engine_estimates_tokens_without_calling_apple() {
        let provider = FoundationModels::pretending(Availability::NotReady);
        let engine = provider
            .open(&model_id(), &CancelToken::new())
            .expect("opening calls nothing");
        assert_eq!(engine.count_tokens(&"x".repeat(885)), 328);
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(matches!(
            provider.open(&model_id(), &cancelled),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn a_nul_byte_fails_before_reaching_apple() {
        let err = complete("sys", "a\0b", 16, 0.2, |_| true, &CancelToken::new()).unwrap_err();
        assert_eq!(err.kind, FailureKind::Other);
    }
}
