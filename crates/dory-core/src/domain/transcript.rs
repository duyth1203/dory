use std::collections::BTreeMap;

use super::{Channel, LangTag, Revision, TimeRange};
use crate::models::ModelRef;

/// A speaker within one transcript. `SpeakerId(0)` is the local user whenever a mic channel exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpeakerId(pub u16);

impl SpeakerId {
    pub const LOCAL: SpeakerId = SpeakerId(0);
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// `None` only for transcripts imported from the old `[Speaker N]` string, which had no times.
    pub range: Option<TimeRange>,
    pub speaker: SpeakerId,
    pub channel: Channel,
    pub text: String,
}

/// Replaced whole by the transcribe stage, never patched line by line.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcript {
    pub revision: Revision,
    pub segments: Vec<Segment>,
    /// Most spoken first.
    pub detected_languages: Vec<LangTag>,
    pub produced_by: ModelRef,
}

impl Transcript {
    pub fn is_timed(&self) -> bool {
        self.segments.iter().all(|s| s.range.is_some())
    }

    /// Summaries are written in this language.
    pub fn main_language(&self) -> Option<&LangTag> {
        self.detected_languages.first()
    }

    /// Distinct speakers in order of first appearance.
    pub fn speakers(&self) -> Vec<SpeakerId> {
        let mut seen = Vec::new();
        for segment in &self.segments {
            if !seen.contains(&segment.speaker) {
                seen.push(segment.speaker);
            }
        }
        seen
    }
}

/// Names the user gave speakers, owned by the user and kept with the recording. Remote speaker
/// numbers mean nothing after a re-transcribe renumbers them, so the names carry the revision they
/// were given for and are ignored (not deleted) on any other revision. They apply again if the
/// user reverts to that revision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpeakerNames {
    pub for_revision: Revision,
    pub names: BTreeMap<SpeakerId, String>,
}

impl SpeakerNames {
    pub fn name_of(&self, speaker: SpeakerId, revision: Revision) -> Option<&str> {
        if self.for_revision != revision {
            return None;
        }
        self.names.get(&speaker).map(String::as_str)
    }

    /// Sets or clears one name for `revision`. Renaming against a different revision starts a new
    /// set, because the old numbers no longer describe the same people.
    pub fn rename(&mut self, revision: Revision, speaker: SpeakerId, name: Option<String>) {
        if self.for_revision != revision {
            self.names.clear();
            self.for_revision = revision;
        }
        match name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()) {
            Some(name) => {
                self.names.insert(speaker, name);
            }
            None => {
                self.names.remove(&speaker);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub markdown: String,
    pub language: Option<LangTag>,
    pub produced_by: ModelRef,
    /// The transcript revision this was written from.
    pub transcript_revision: Revision,
}

impl Summary {
    pub fn is_stale_for(&self, transcript: Revision) -> bool {
        self.transcript_revision != transcript
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Ms;

    fn seg(speaker: u16, range: Option<(u64, u64)>) -> Segment {
        Segment {
            range: range.and_then(|(a, b)| TimeRange::new(Ms(a), Ms(b))),
            speaker: SpeakerId(speaker),
            channel: Channel::System,
            text: String::new(),
        }
    }

    fn transcript(segments: Vec<Segment>) -> Transcript {
        Transcript {
            revision: Revision(1),
            segments,
            detected_languages: vec![],
            produced_by: ModelRef::parse("sona/whisper-small").unwrap(),
        }
    }

    #[test]
    fn timed_only_when_every_segment_has_a_range() {
        assert!(transcript(vec![seg(0, Some((0, 5))), seg(1, Some((5, 9)))]).is_timed());
        assert!(!transcript(vec![seg(0, Some((0, 5))), seg(1, None)]).is_timed());
    }

    #[test]
    fn speakers_are_distinct_in_first_appearance_order() {
        let t = transcript(vec![seg(2, None), seg(0, None), seg(2, None), seg(1, None)]);
        assert_eq!(t.speakers(), vec![SpeakerId(2), SpeakerId(0), SpeakerId(1)]);
    }

    #[test]
    fn names_apply_only_to_the_revision_they_were_given_for() {
        let mut names = SpeakerNames::default();
        names.rename(Revision(1), SpeakerId(2), Some("Ana".into()));
        let cases = [
            (Revision(1), SpeakerId(2), Some("Ana")),
            (Revision(1), SpeakerId(3), None),
            (Revision(2), SpeakerId(2), None),
        ];
        for (rev, id, want) in cases {
            assert_eq!(names.name_of(id, rev), want, "{rev:?} {id:?}");
        }
    }

    #[test]
    fn renaming_on_a_new_revision_starts_over_and_reverting_loses_the_old_set() {
        let mut names = SpeakerNames::default();
        names.rename(Revision(1), SpeakerId(1), Some("Ana".into()));
        names.rename(Revision(2), SpeakerId(1), Some("Bo".into()));
        assert_eq!(names.name_of(SpeakerId(1), Revision(2)), Some("Bo"));
        assert_eq!(names.name_of(SpeakerId(1), Revision(1)), None);
    }

    #[test]
    fn blank_name_clears() {
        let mut names = SpeakerNames::default();
        names.rename(Revision(1), SpeakerId(1), Some("Ana".into()));
        names.rename(Revision(1), SpeakerId(1), Some("  ".into()));
        assert_eq!(names.name_of(SpeakerId(1), Revision(1)), None);
    }

    #[test]
    fn summary_staleness_follows_transcript_revision() {
        let summary = Summary {
            markdown: String::new(),
            language: None,
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            transcript_revision: Revision(2),
        };
        assert!(!summary.is_stale_for(Revision(2)));
        assert!(summary.is_stale_for(Revision(3)));
    }
}
