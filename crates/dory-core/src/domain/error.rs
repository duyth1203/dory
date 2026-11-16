use thiserror::Error;

/// A value that failed validation at a boundary (parsing, settings, CLI input).
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("invalid {what}: {detail}")]
pub struct InvalidInput {
    pub what: &'static str,
    pub detail: String,
}

impl InvalidInput {
    pub(crate) fn new(what: &'static str, detail: impl Into<String>) -> Self {
        Self {
            what,
            detail: detail.into(),
        }
    }
}

/// What a caller should do about a failure. Every port error maps to one through
/// [`crate::ports::Classify`], so retry and blocking policy lives in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The caller asked for it. Never retried, never shown as an error.
    Cancelled,
    /// Likely to succeed if tried again.
    Transient,
    /// Only the user can fix it (permission, missing model, full disk).
    NeedsUser,
    Permanent,
}

/// What a failure means to the user, so the app can say what to do about it (UX-13). The class
/// decides whether the core retries; the cause decides the sentence and the button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureCause {
    /// The disk has no room for the work.
    DiskFull,
    /// The take is missing or cannot be decoded.
    AudioUnreadable,
    /// The engine or the whole process died partway.
    Crashed,
    Other,
}
