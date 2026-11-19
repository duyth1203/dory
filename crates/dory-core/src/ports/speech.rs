use super::{CancelToken, Clip, EngineError};
use crate::domain::{Fraction, LangTag, LanguagePref, TimeRange, VocabularyTerm};

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
    /// Threads the engine may compute with. An engine that holds a thread pool rebuilds it when
    /// this changes; the pace policy's hold bounds how often.
    pub threads: u8,
    /// One `RawSegment` per sentence, each with its own range. Set only for an engine whose caps
    /// say `sentence_times`; others ignore it.
    pub sentence_times: bool,
    /// The locale the meeting's first window ran in, as its outcome reported it, on every later
    /// window of a pinned meeting, a resumed run's included. An engine that runs one regional
    /// variant of the pinned language keeps to it, so a meeting never mixes en-US and en-GB.
    /// Engines that run no regional variant ignore it.
    pub locale: Option<LangTag>,
    /// Terms the engine should favor. An engine whose caps say no vocabulary ignores it.
    pub vocabulary: &'a [VocabularyTerm],
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
    /// The language the engine heard. An engine that runs one regional variant of a pinned
    /// language reports the locale it ran in, which later windows get back as
    /// [`TranscribeRequest::locale`].
    pub detected_language: Option<LangTag>,
}
