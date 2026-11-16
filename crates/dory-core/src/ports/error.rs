use std::error::Error as StdError;

use thiserror::Error;

use super::capture::CaptureError;
use crate::domain::{FailureCause, FailureClass, IllegalTransition, LangTag, RecordingId};

type Source = Box<dyn StdError + Send + Sync>;

/// Maps an adapter error to what the core should do about it.
pub trait Classify {
    fn class(&self) -> FailureClass;

    fn cause(&self) -> FailureCause {
        FailureCause::Other
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("{0} not found")]
    NotFound(String),
    #[error(transparent)]
    IllegalTransition(#[from] IllegalTransition),
    #[error("storage is busy or unavailable")]
    Unavailable(#[source] Source),
    #[error("stored data is corrupt: {0}")]
    Corrupt(String),
    #[error("storage schema {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },
}

impl Classify for StoreError {
    fn class(&self) -> FailureClass {
        match self {
            StoreError::Unavailable(_) => FailureClass::Transient,
            StoreError::NotFound(_)
            | StoreError::IllegalTransition(_)
            | StoreError::Corrupt(_)
            | StoreError::SchemaTooNew { .. } => FailureClass::Permanent,
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TakeError {
    #[error("no audio for {0}")]
    Missing(RecordingId),
    #[error("a take for {0} already exists")]
    Exists(RecordingId),
    #[error("audio io failed")]
    Io(#[source] std::io::Error),
    #[error("not enough disk space")]
    DiskFull,
    #[error("audio could not be decoded: {0}")]
    Decode(String),
    /// Legacy WebM needs a converter that is not installed.
    #[error("a converter is required but not installed: {0}")]
    ConverterMissing(String),
}

impl Classify for TakeError {
    fn class(&self) -> FailureClass {
        match self {
            TakeError::Io(_) => FailureClass::Transient,
            TakeError::DiskFull | TakeError::ConverterMissing(_) => FailureClass::NeedsUser,
            TakeError::Missing(_) | TakeError::Exists(_) | TakeError::Decode(_) => {
                FailureClass::Permanent
            }
        }
    }

    fn cause(&self) -> FailureCause {
        match self {
            TakeError::DiskFull => FailureCause::DiskFull,
            TakeError::Missing(_) | TakeError::Decode(_) | TakeError::ConverterMissing(_) => {
                FailureCause::AudioUnreadable
            }
            TakeError::Io(_) | TakeError::Exists(_) => FailureCause::Other,
        }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProvisionError {
    #[error("cancelled")]
    Cancelled,
    #[error("cannot be provisioned here: {0}")]
    NotProvisionable(String),
    #[error("network: {0}")]
    Network(String),
    #[error("downloaded file failed verification")]
    ChecksumMismatch,
    #[error("not enough disk space, need {needed} bytes")]
    DiskFull { needed: u64 },
    #[error("provisioning failed: {0}")]
    Other(String),
}

impl Classify for ProvisionError {
    fn class(&self) -> FailureClass {
        match self {
            ProvisionError::Cancelled => FailureClass::Cancelled,
            ProvisionError::Network(_) | ProvisionError::ChecksumMismatch => {
                FailureClass::Transient
            }
            ProvisionError::NotProvisionable(_) | ProvisionError::DiskFull { .. } => {
                FailureClass::NeedsUser
            }
            ProvisionError::Other(_) => FailureClass::Permanent,
        }
    }

    fn cause(&self) -> FailureCause {
        match self {
            ProvisionError::DiskFull { .. } => FailureCause::DiskFull,
            _ => FailureCause::Other,
        }
    }
}

/// Errors from opening or running a loaded engine.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("cancelled")]
    Cancelled,
    #[error("model files missing: {0}")]
    ModelMissing(String),
    #[error("engine server unavailable: {0}")]
    ServerUnavailable(String),
    #[error("language {0} is not supported by this model")]
    UnsupportedLanguage(LangTag),
    #[error("engine rejected the input: {0}")]
    InputRejected(String),
    /// The engine process or runtime died. The pipeline reopens it and retries.
    #[error("engine crashed: {0}")]
    Crashed(String),
    #[error("engine error: {message}")]
    Other {
        message: String,
        #[source]
        source: Option<Source>,
    },
}

impl Classify for EngineError {
    fn class(&self) -> FailureClass {
        match self {
            EngineError::Cancelled => FailureClass::Cancelled,
            EngineError::ServerUnavailable(_) | EngineError::Crashed(_) => FailureClass::Transient,
            EngineError::ModelMissing(_) | EngineError::UnsupportedLanguage(_) => {
                FailureClass::NeedsUser
            }
            EngineError::InputRejected(_) | EngineError::Other { .. } => FailureClass::Permanent,
        }
    }

    fn cause(&self) -> FailureCause {
        match self {
            EngineError::Crashed(_) => FailureCause::Crashed,
            _ => FailureCause::Other,
        }
    }
}

impl Classify for CaptureError {
    fn class(&self) -> FailureClass {
        match self {
            CaptureError::AccessDenied(_) | CaptureError::DeviceNotFound(_) => {
                FailureClass::NeedsUser
            }
            CaptureError::Unsupported(_) | CaptureError::Platform { .. } => FailureClass::Permanent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{JobEvent, JobState};
    use crate::ports::capture::Source as CaptureSource;
    use FailureClass::*;

    fn io() -> std::io::Error {
        std::io::Error::other("x")
    }

    fn boxed() -> Source {
        Box::new(io())
    }

    #[test]
    fn store_error_classes() {
        let id = RecordingId::parse("r").unwrap();
        let illegal = IllegalTransition {
            from: Box::new(JobState::Succeeded),
            event: Box::new(JobEvent::Retake),
        };
        let cases: Vec<(StoreError, FailureClass)> = vec![
            (StoreError::Unavailable(boxed()), Transient),
            (StoreError::NotFound(id.to_string()), Permanent),
            (StoreError::IllegalTransition(illegal), Permanent),
            (StoreError::Corrupt("x".into()), Permanent),
            (
                StoreError::SchemaTooNew {
                    found: 3,
                    supported: 2,
                },
                Permanent,
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.class(), want, "{err}");
        }
    }

    #[test]
    fn take_error_classes() {
        let id = RecordingId::parse("r").unwrap();
        let cases: Vec<(TakeError, FailureClass)> = vec![
            (TakeError::Io(io()), Transient),
            (TakeError::DiskFull, NeedsUser),
            (TakeError::ConverterMissing("ffmpeg".into()), NeedsUser),
            (TakeError::Missing(id.clone()), Permanent),
            (TakeError::Exists(id), Permanent),
            (TakeError::Decode("x".into()), Permanent),
        ];
        for (err, want) in cases {
            assert_eq!(err.class(), want, "{err}");
        }
    }

    #[test]
    fn causes_name_what_the_user_can_act_on() {
        use FailureCause::*;
        let id = RecordingId::parse("r").unwrap();
        let cases: Vec<(Box<dyn Classify>, FailureCause)> = vec![
            (Box::new(TakeError::DiskFull), DiskFull),
            (Box::new(ProvisionError::DiskFull { needed: 1 }), DiskFull),
            (Box::new(TakeError::Decode("x".into())), AudioUnreadable),
            (Box::new(TakeError::Missing(id.clone())), AudioUnreadable),
            (
                Box::new(TakeError::ConverterMissing("ffmpeg".into())),
                AudioUnreadable,
            ),
            (Box::new(EngineError::Crashed("x".into())), Crashed),
            (Box::new(TakeError::Io(io())), Other),
            (Box::new(TakeError::Exists(id)), Other),
            (Box::new(EngineError::InputRejected("x".into())), Other),
            (Box::new(StoreError::Corrupt("x".into())), Other),
        ];
        for (ix, (err, want)) in cases.into_iter().enumerate() {
            assert_eq!(err.cause(), want, "case {ix}");
        }
    }

    #[test]
    fn provision_error_classes() {
        let cases: Vec<(ProvisionError, FailureClass)> = vec![
            (ProvisionError::Cancelled, Cancelled),
            (ProvisionError::Network("x".into()), Transient),
            (ProvisionError::ChecksumMismatch, Transient),
            (ProvisionError::DiskFull { needed: 1 }, NeedsUser),
            (ProvisionError::NotProvisionable("x".into()), NeedsUser),
            (ProvisionError::Other("x".into()), Permanent),
        ];
        for (err, want) in cases {
            assert_eq!(err.class(), want, "{err}");
        }
    }

    #[test]
    fn engine_error_classes() {
        let cases: Vec<(EngineError, FailureClass)> = vec![
            (EngineError::Cancelled, Cancelled),
            (EngineError::ServerUnavailable("x".into()), Transient),
            (EngineError::Crashed("x".into()), Transient),
            (EngineError::ModelMissing("x".into()), NeedsUser),
            (
                EngineError::UnsupportedLanguage(LangTag::parse("vi").unwrap()),
                NeedsUser,
            ),
            (EngineError::InputRejected("x".into()), Permanent),
            (
                EngineError::Other {
                    message: "x".into(),
                    source: None,
                },
                Permanent,
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.class(), want, "{err}");
        }
    }

    #[test]
    fn capture_error_classes() {
        let cases: Vec<(CaptureError, FailureClass)> = vec![
            (
                CaptureError::AccessDenied(CaptureSource::Microphone),
                NeedsUser,
            ),
            (CaptureError::DeviceNotFound("x".into()), NeedsUser),
            (CaptureError::Unsupported("x".into()), Permanent),
            (
                CaptureError::Platform {
                    message: "x".into(),
                    source: None,
                },
                Permanent,
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.class(), want, "{err}");
        }
    }
}
