use std::collections::BTreeMap;

use super::{Channel, Fraction, LangTag, Revision, TimeRange};
use crate::models::ModelRef;

/// The least share of a transcript's timed speech a language must hold, in percent, for the
/// transcript to count as spoken in two languages. A greeting or a quoted phrase in another
/// language stays under it.
pub const BILINGUAL_SHARE: u64 = 10;

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
    /// The language the engine detected for this stretch of speech. `None` when unknown: an
    /// import, or a transcript made before Dory kept it. Translation treats `None` as unknown
    /// and translates it.
    pub language: Option<LangTag>,
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

    /// Each language's share of the timed speech, most first and ties in order of first
    /// appearance. Segments with no time or no language are ignored. A language counts once
    /// however its tag is spelled, so `en` and `en-US` add up under the first one seen.
    pub fn language_shares(&self) -> Vec<(LangTag, Fraction)> {
        let (totals, all) = self.language_time();
        totals
            .into_iter()
            .map(|(tag, time)| (tag, Fraction::new(time as f32 / all.max(1) as f32)))
            .collect()
    }

    /// Whether two or more languages each hold at least [`BILINGUAL_SHARE`] of the timed speech.
    pub fn is_bilingual(&self) -> bool {
        let (totals, all) = self.language_time();
        totals
            .iter()
            .filter(|(_, time)| all > 0 && time * 100 >= all * BILINGUAL_SHARE)
            .count()
            >= 2
    }

    /// Whole milliseconds per language, most first, and their total.
    fn language_time(&self) -> (Vec<(LangTag, u64)>, u64) {
        let mut totals: Vec<(LangTag, u64)> = Vec::new();
        let mut all = 0;
        for segment in &self.segments {
            let (Some(tag), Some(range)) = (&segment.language, segment.range) else {
                continue;
            };
            let time = range.len().0;
            all += time;
            match totals
                .iter_mut()
                .find(|(t, _)| t.primary() == tag.primary())
            {
                Some((_, total)) => *total += time,
                None => totals.push((tag.clone(), time)),
            }
        }
        totals.sort_by_key(|(_, total)| std::cmp::Reverse(*total));
        (totals, all)
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
    /// What a segment's speaker is called: the user's name for them when it was given for this
    /// revision, "Me" for the local user on the mic, else "Speaker N".
    pub fn label(&self, segment: &Segment, revision: Revision) -> String {
        match self.name_of(segment.speaker, revision) {
            Some(name) => name.to_owned(),
            None if segment.speaker == SpeakerId::LOCAL && segment.channel == Channel::Mic => {
                "Me".to_owned()
            }
            None => format!("Speaker {}", segment.speaker.0),
        }
    }

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
            language: None,
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

    fn spoken(language: &str, range: Option<(u64, u64)>) -> Segment {
        Segment {
            language: LangTag::parse(language).ok(),
            ..seg(0, range)
        }
    }

    fn shares(t: &Transcript) -> Vec<(String, u32)> {
        t.language_shares()
            .into_iter()
            .map(|(tag, f)| (tag.to_string(), (f.get() * 1000.0).round() as u32))
            .collect()
    }

    #[test]
    fn shares_are_each_languages_timed_speech_most_first() {
        let t = transcript(vec![
            spoken("en", Some((0, 2_000))),
            spoken("tl", Some((2_000, 8_000))),
            spoken("en-US", Some((8_000, 10_000))),
            spoken("tl", None),
            seg(0, Some((10_000, 50_000))),
        ]);
        assert_eq!(shares(&t), [("tl".into(), 600), ("en".into(), 400)]);
        assert!(shares(&transcript(vec![spoken("en", None)])).is_empty());
    }

    #[test]
    fn a_tie_keeps_the_language_heard_first() {
        let t = transcript(vec![
            spoken("en", Some((0, 1_000))),
            spoken("tl", Some((1_000, 2_000))),
        ]);
        assert_eq!(shares(&t), [("en".into(), 500), ("tl".into(), 500)]);
    }

    #[test]
    fn bilingual_needs_two_languages_at_ten_percent_each() {
        let with_other = |other_ms: u64| {
            transcript(vec![
                spoken("tl", Some((0, 90_000))),
                spoken("en", Some((90_000, 90_000 + other_ms))),
                spoken("en", None),
                seg(0, None),
            ])
        };
        // 8_900 of 98_900 is 9.0%, and 10_000 of 100_000 is 10% on the dot.
        assert!(!with_other(8_900).is_bilingual());
        assert!(with_other(10_000).is_bilingual());
        assert!(with_other(90_000).is_bilingual());
        assert!(!transcript(vec![spoken("tl", Some((0, 5_000)))]).is_bilingual());
        assert!(!transcript(vec![]).is_bilingual());
    }

    #[test]
    fn a_language_under_the_share_does_not_make_a_third() {
        let t = transcript(vec![
            spoken("tl", Some((0, 50_000))),
            spoken("en", Some((50_000, 90_000))),
            spoken("es", Some((90_000, 95_000))),
        ]);
        assert!(t.is_bilingual());
        assert_eq!(t.language_shares().len(), 3);
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
