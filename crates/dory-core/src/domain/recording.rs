use super::{ChannelLayout, JobState, LanguagePref, Ms, RecordingId, Revision, Timestamp};

/// What the library shows for one recording. The take's bytes live behind [`crate::ports::Takes`].
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub id: RecordingId,
    /// `None` shows a date-based title.
    pub title: Option<String>,
    pub created_at: Timestamp,
    pub language: LanguagePref,
    pub layout: ChannelLayout,
    /// `None` until the first frames land.
    pub take: Option<TakeInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TakeContainer {
    /// Crash-tolerant, written while recording.
    Wav,
    /// The permanent form after sealing.
    Flac,
    /// Pre-rebuild MediaRecorder output. Read-only.
    LegacyWebm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakeInfo {
    pub container: TakeContainer,
    pub rate_hz: u32,
    pub duration: Ms,
}

/// A library list row: the recording plus what the list needs, in one read.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingRow {
    pub recording: Recording,
    pub job: Option<JobState>,
    pub has_transcript: bool,
    pub summary: SummaryFlag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryFlag {
    None,
    Current,
    Stale,
}

impl SummaryFlag {
    /// The only place staleness is decided. A summary is stale when it was made from a different
    /// transcript revision than the one now stored.
    pub fn derive(summary_of: Option<Revision>, transcript: Option<Revision>) -> SummaryFlag {
        match (summary_of, transcript) {
            (None, _) => SummaryFlag::None,
            (Some(made_from), Some(now)) if made_from == now => SummaryFlag::Current,
            (Some(_), _) => SummaryFlag::Stale,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordingQuery {
    pub text: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecordingEdit {
    Title(Option<String>),
    Language(LanguagePref),
    Take(TakeInfo),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_flag_table() {
        let r = Revision;
        let cases = [
            (None, None, SummaryFlag::None),
            (None, Some(r(3)), SummaryFlag::None),
            (Some(r(3)), Some(r(3)), SummaryFlag::Current),
            (Some(r(2)), Some(r(3)), SummaryFlag::Stale),
            (Some(r(3)), Some(r(2)), SummaryFlag::Stale),
            (Some(r(1)), None, SummaryFlag::Stale),
        ];
        for (summary, transcript, want) in cases {
            assert_eq!(
                SummaryFlag::derive(summary, transcript),
                want,
                "{summary:?} vs {transcript:?}"
            );
        }
    }
}
