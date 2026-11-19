//! A transcript or a summary written in another language, kept beside the original, never over it.

use thiserror::Error;

use super::{LangTag, Revision, Segment, Summary, Transcript};
use crate::models::ModelRef;

/// The spoken transcript translated into `target`. It belongs to one transcript revision: a
/// re-transcribe renumbers and re-cuts segments, so a translation of the old revision lines up
/// with nothing and every reader checks [`Translation::applies_to`].
#[derive(Clone, Debug, PartialEq)]
pub struct Translation {
    pub target: LangTag,
    pub transcript_revision: Revision,
    pub produced_by: ModelRef,
    /// One entry per segment of that revision, by index. `None` keeps the spoken text, because
    /// the segment was already in the target or had nothing to say.
    pub texts: Vec<Option<String>>,
}

impl Translation {
    /// Whether this translation lines up with the stored transcript.
    pub fn applies_to(&self, transcript: &Transcript) -> bool {
        self.transcript_revision == transcript.revision
            && self.texts.len() == transcript.segments.len()
    }

    /// The translated text of segment `ix`, if it has one.
    pub fn text(&self, ix: usize) -> Option<&str> {
        self.texts.get(ix)?.as_deref()
    }

    /// Whether any segment differs from what was spoken, so a switch between the two has
    /// something to show.
    pub fn has_text(&self) -> bool {
        self.texts.iter().any(Option::is_some)
    }

    /// The text of `segment` at `ix`: its translation when it has one, else what was spoken.
    pub fn shown<'a>(&'a self, ix: usize, segment: &'a Segment) -> &'a str {
        self.text(ix).unwrap_or(&segment.text)
    }
}

/// The recording's summary translated into `target`, on the user's request. A recording has at
/// most one. It is never stale: the store drops it in the commit that writes or restores a
/// summary, so it always describes the summary now stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryTranslation {
    pub target: LangTag,
    pub produced_by: ModelRef,
    pub markdown: String,
}

/// Why a summary cannot be translated into the language asked for.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SummaryRefusal {
    #[error("There is no summary to translate yet.")]
    NoSummary,
    #[error("The summary is already written in {0}.")]
    AlreadyWritten(LangTag),
}

impl SummaryTranslation {
    /// The one rule for asking: there must be a summary, and it must not already be in the target.
    /// A summary with no recorded language is translated, as a segment of unknown language is.
    /// `en-US` against `en` is the same language.
    pub fn refusal(summary: Option<&Summary>, target: &LangTag) -> Option<SummaryRefusal> {
        let Some(summary) = summary else {
            return Some(SummaryRefusal::NoSummary);
        };
        summary
            .language
            .as_ref()
            .filter(|written| written.primary() == target.primary())
            .map(|_| SummaryRefusal::AlreadyWritten(target.clone()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslationFlag {
    None,
    Current,
    Stale,
}

impl TranslationFlag {
    /// The only place translation staleness is decided, as [`super::SummaryFlag::derive`] is for
    /// summaries. A translation is current when it was made from the transcript revision now
    /// stored, into the language now wanted. A new revision or another target makes it stale.
    /// Asking for no translation leaves an existing one stale, never current.
    pub fn derive(
        translated: Option<(Revision, &LangTag)>,
        transcript: Option<Revision>,
        wanted: Option<&LangTag>,
    ) -> TranslationFlag {
        match (translated, transcript) {
            (None, _) => TranslationFlag::None,
            (Some((made_from, target)), Some(now))
                if made_from == now && wanted == Some(target) =>
            {
                TranslationFlag::Current
            }
            (Some(_), _) => TranslationFlag::Stale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Channel, SpeakerId};

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn segment(text: &str) -> Segment {
        Segment {
            range: None,
            speaker: SpeakerId(1),
            channel: Channel::System,
            text: text.into(),
            language: None,
        }
    }

    fn transcript(revision: u32, texts: &[&str]) -> Transcript {
        Transcript {
            revision: Revision(revision),
            segments: texts.iter().map(|t| segment(t)).collect(),
            detected_languages: vec![],
            produced_by: ModelRef::parse("sona/whisper-small").unwrap(),
        }
    }

    fn translation(revision: u32, texts: &[Option<&str>]) -> Translation {
        Translation {
            target: tag("en"),
            transcript_revision: Revision(revision),
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            texts: texts.iter().map(|t| t.map(str::to_owned)).collect(),
        }
    }

    #[test]
    fn flag_table() {
        let (en, fr) = (tag("en"), tag("fr"));
        let r = Revision;
        let cases = [
            (None, Some(r(1)), Some(&en), TranslationFlag::None),
            (None, None, None, TranslationFlag::None),
            (
                Some((r(1), &en)),
                Some(r(1)),
                Some(&en),
                TranslationFlag::Current,
            ),
            (
                Some((r(1), &en)),
                Some(r(2)),
                Some(&en),
                TranslationFlag::Stale,
            ),
            (
                Some((r(2), &en)),
                Some(r(1)),
                Some(&en),
                TranslationFlag::Stale,
            ),
            (
                Some((r(1), &en)),
                Some(r(1)),
                Some(&fr),
                TranslationFlag::Stale,
            ),
            (Some((r(1), &en)), Some(r(1)), None, TranslationFlag::Stale),
            (Some((r(1), &en)), None, Some(&en), TranslationFlag::Stale),
        ];
        for (translated, transcript, wanted, want) in cases {
            assert_eq!(
                TranslationFlag::derive(translated, transcript, wanted),
                want,
                "{translated:?} vs {transcript:?} wanting {wanted:?}"
            );
        }
    }

    #[test]
    fn a_translation_applies_only_to_the_revision_and_length_it_was_made_for() {
        let t = transcript(3, &["a", "b"]);
        assert!(translation(3, &[Some("A"), None]).applies_to(&t));
        assert!(!translation(2, &[Some("A"), None]).applies_to(&t));
        assert!(!translation(3, &[Some("A")]).applies_to(&t));
    }

    fn summary(language: Option<&str>) -> Summary {
        Summary {
            markdown: "## Key points\n- one".into(),
            language: language.map(tag),
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            transcript_revision: Revision(1),
        }
    }

    #[test]
    fn a_summary_is_translated_only_when_it_exists_and_is_in_another_language() {
        let en = tag("en");
        let cases = [
            (None, SummaryRefusal::NoSummary.into()),
            (
                Some(summary(Some("en"))),
                Some(SummaryRefusal::AlreadyWritten(en.clone())),
            ),
            (
                Some(summary(Some("en-US"))),
                Some(SummaryRefusal::AlreadyWritten(en.clone())),
            ),
            (Some(summary(Some("tl"))), None),
            (Some(summary(None)), None),
        ];
        for (summary, want) in cases {
            assert_eq!(
                SummaryTranslation::refusal(summary.as_ref(), &en),
                want,
                "{summary:?}"
            );
        }
        assert_eq!(
            SummaryRefusal::AlreadyWritten(en).to_string(),
            "The summary is already written in en."
        );
    }

    #[test]
    fn the_shown_text_is_the_translation_or_else_what_was_spoken() {
        let t = transcript(1, &["bonjour", "ok"]);
        let tr = translation(1, &[Some("hello"), None]);
        let shown: Vec<&str> = t
            .segments
            .iter()
            .enumerate()
            .map(|(ix, s)| tr.shown(ix, s))
            .collect();
        assert_eq!(shown, ["hello", "ok"]);
        assert!(tr.has_text());
        assert!(!translation(1, &[None, None]).has_text());
    }
}
