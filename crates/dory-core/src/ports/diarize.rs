use super::{CancelToken, Clip, EngineError};
use crate::domain::{Fraction, TimeRange};

/// A speaker separator, used only when the speech engine does not diarize natively.
pub trait Diarizer: Send + Sync {
    fn diarize(
        &self,
        clip: &Clip,
        progress: &mut dyn FnMut(Fraction),
        cancel: &CancelToken,
    ) -> Result<Vec<Turn>, EngineError>;
}

/// Who spoke when, relative to the clip. The core overlays turns on segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    pub range: TimeRange,
    pub speaker: u16,
}
