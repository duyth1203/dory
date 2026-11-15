use super::{CancelToken, Clip, EngineError};
use crate::domain::{Fraction, LangTag, LanguagePref, TimeRange};

/// A loaded speech engine. The core decides which clips to send and merges the results. The engine
/// turns one clip into timed text.
pub trait Transcriber: Send + Sync {
    fn transcribe(
        &self,
        req: &TranscribeRequest<'_>,
        sink: &mut dyn SegmentSink,
        cancel: &CancelToken,
    ) -> Result<TranscribeOutcome, EngineError>;
}

pub struct TranscribeRequest<'a> {
    pub clip: &'a Clip,
    /// An engine whose caps say `FixedLocale` always gets `Pinned`, because `fit` rules out the rest.
    pub language: LanguagePref,
    /// Set only for an engine with native diarization, on a clip that mixes speakers.
    pub diarize: bool,
    /// Vocabulary or context, such as the meeting title. Engines may ignore it.
    pub hint: Option<&'a str>,
}

pub trait SegmentSink {
    fn progress(&mut self, fraction: Fraction);
    /// In time order. The core re-sorts across channels.
    fn segment(&mut self, segment: RawSegment);
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawSegment {
    /// Relative to the clip.
    pub range: TimeRange,
    pub text: String,
    /// The engine's own speaker index when it diarizes natively. Only stable within one call.
    pub speaker: Option<u16>,
    pub language: Option<LangTag>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscribeOutcome {
    pub detected_language: Option<LangTag>,
}
