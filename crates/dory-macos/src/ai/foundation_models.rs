//! Safe Rust over Apple's Foundation Models: what the Mac offers, and one blocking completion.

use std::ffi::CString;
use std::sync::{Arc, Mutex, PoisonError};

use dory_core::ports::CancelToken;

use super::ffi;

/// What `SystemLanguageModel.default` reports on this Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// Ready to generate. `languages` are BCP 47 tags such as `en`, `en-GB`, and `zh-Hant`.
    Available {
        context_size: u32,
        languages: Vec<String>,
    },
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
pub fn info() -> Availability {
    let raw = ffi::fm_info();
    availability(raw.state, raw.context_size, &raw.languages_csv)
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

fn availability(state: i32, context_size: u32, languages_csv: &str) -> Availability {
    match state {
        ffi::FM_AVAILABLE => Availability::Available {
            context_size,
            languages: languages_csv
                .split(',')
                .filter(|tag| !tag.is_empty())
                .map(str::to_owned)
                .collect(),
        },
        ffi::FM_NOT_ENABLED => Availability::NotEnabled,
        ffi::FM_NOT_ELIGIBLE => Availability::NotEligible,
        ffi::FM_NOT_READY => Availability::NotReady,
        ffi::FM_OS_TOO_OLD => Availability::OsTooOld,
        _ => Availability::Unknown,
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

    #[test]
    fn availability_reads_every_apple_state() {
        assert_eq!(
            availability(ffi::FM_AVAILABLE, 4096, "de,en,en-GB"),
            Availability::Available {
                context_size: 4096,
                languages: vec!["de".into(), "en".into(), "en-GB".into()],
            }
        );
        assert_eq!(
            availability(ffi::FM_AVAILABLE, 4096, ""),
            Availability::Available {
                context_size: 4096,
                languages: vec![],
            }
        );
        assert_eq!(
            availability(ffi::FM_NOT_ENABLED, 0, ""),
            Availability::NotEnabled
        );
        assert_eq!(
            availability(ffi::FM_NOT_ELIGIBLE, 0, ""),
            Availability::NotEligible
        );
        assert_eq!(
            availability(ffi::FM_NOT_READY, 0, ""),
            Availability::NotReady
        );
        assert_eq!(
            availability(ffi::FM_OS_TOO_OLD, 4096, ""),
            Availability::OsTooOld
        );
        assert_eq!(availability(9, 0, ""), Availability::Unknown);
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
    fn a_nul_byte_fails_before_reaching_apple() {
        let err = complete("sys", "a\0b", 16, 0.2, |_| true, &CancelToken::new()).unwrap_err();
        assert_eq!(err.kind, FailureKind::Other);
    }
}
