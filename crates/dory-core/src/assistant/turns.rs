//! A transcript as speaker turns, a page at a time.
//!
//! Consecutive segments by the same speaker in the same language merge into one turn of at most [`MAX_TURN_CHARS`], so
//! any page budget of at least that size holds at least one turn. A cursor names the transcript
//! revision and the next turn, so a page from a transcript that changed since is refused rather
//! than spliced into the new one.

use crate::domain::{LangTag, Ms, Revision, SpeakerNames, Transcript, Translation};

pub const MAX_TURN_CHARS: usize = 4_000;
pub const DEFAULT_PAGE_CHARS: usize = 20_000;
pub const MAX_PAGE_CHARS: usize = 40_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub speaker: String,
    /// `None` for transcripts imported without times.
    pub start: Option<Ms>,
    pub end: Option<Ms>,
    /// The language the turn was spoken in, whether or not it was translated. `None` when the
    /// engine did not say.
    pub language: Option<LangTag>,
    /// What the assistant should read: the translation when the turn was translated, else what
    /// was spoken.
    pub text: String,
    /// Set when `text` is a translation.
    pub translated: Option<Translated>,
}

/// What a translated turn was before it was translated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translated {
    /// The language the turn was heard in, when the engine said.
    pub from: Option<LangTag>,
    /// The spoken text.
    pub original: String,
}

/// Where the next page starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnCursor {
    pub revision: Revision,
    pub turn: u32,
}

impl TurnCursor {
    pub fn parse(s: &str) -> Option<TurnCursor> {
        let (r, t) = s.strip_prefix('r')?.split_once(".t")?;
        Some(TurnCursor {
            revision: Revision(r.parse().ok()?),
            turn: t.parse().ok()?,
        })
    }
}

impl std::fmt::Display for TurnCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "r{}.t{}", self.revision.0, self.turn)
    }
}

/// Clamps a requested page size to what a page may hold.
pub fn page_chars(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DEFAULT_PAGE_CHARS)
        .clamp(MAX_TURN_CHARS, MAX_PAGE_CHARS)
}

/// Splits `text` into pieces of at most `max` characters, at a space when one is near the cut.
fn pieces(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let at = rest[..cut]
            .rfind(' ')
            .filter(|&i| i > cut / 2)
            .unwrap_or(cut);
        out.push(rest[..at].trim_end().to_owned());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

/// Whether two segments were spoken in the same language: `en` and `en-US` are, and an unknown
/// language only matches itself.
fn same_language(a: Option<&LangTag>, b: Option<&LangTag>) -> bool {
    a.map(LangTag::primary) == b.map(LangTag::primary)
}

/// Every turn of a transcript, speakers named as the summary names them. With a translation
/// that applies, a translated segment reads as translated and carries what was spoken. A turn
/// ends when the speaker or the language changes, so a turn has one language, and a turn
/// merges only within one kind: untranslated text, or text translated from one language.
pub fn turns(
    transcript: &Transcript,
    names: &SpeakerNames,
    translation: Option<&Translation>,
) -> Vec<Turn> {
    let translation = translation.filter(|t| t.applies_to(transcript));
    let mut out: Vec<Turn> = Vec::new();
    for (ix, segment) in transcript.segments.iter().enumerate() {
        let speaker = names.label(segment, transcript.revision);
        let start = segment.range.map(|r| r.start());
        let end = segment.range.map(|r| r.end());
        let translated = translation.and_then(|t| t.text(ix));
        let from = translated.map(|_| segment.language.clone());
        let spoken = translated.map(|_| segment.text.trim());
        for (n, piece) in pieces(translated.unwrap_or(&segment.text), MAX_TURN_CHARS)
            .into_iter()
            .enumerate()
        {
            // The spoken text rides with the first piece of its segment.
            let original = spoken.filter(|_| n == 0);
            match out.last_mut() {
                Some(last)
                    if last.speaker == speaker
                        && same_language(last.language.as_ref(), segment.language.as_ref())
                        && last.translated.as_ref().map(|t| &t.from) == from.as_ref()
                        && last.text.chars().count() + 1 + piece.chars().count()
                            <= MAX_TURN_CHARS =>
                {
                    last.text.push(' ');
                    last.text.push_str(&piece);
                    last.end = end.or(last.end);
                    if let (Some(t), Some(original)) = (&mut last.translated, original) {
                        t.original.push(' ');
                        t.original.push_str(original);
                    }
                }
                _ => out.push(Turn {
                    speaker: speaker.clone(),
                    start,
                    end,
                    language: segment.language.clone(),
                    text: piece,
                    translated: from.clone().map(|from| Translated {
                        from,
                        original: original.unwrap_or_default().to_owned(),
                    }),
                }),
            }
        }
    }
    out
}

/// The size a turn takes on a page: its text, what it was before translation, its speaker, and
/// room for its time stamp.
fn cost(turn: &Turn) -> usize {
    turn.text.chars().count()
        + turn
            .translated
            .as_ref()
            .map_or(0, |t| t.original.chars().count())
        + turn.speaker.chars().count()
        + 16
}

/// The turns from `from` that fit in `budget`, and where the next page starts. A page always
/// holds at least one turn, so paging always moves forward.
pub fn page(all: &[Turn], from: usize, budget: usize) -> (&[Turn], Option<usize>) {
    let rest = all.get(from..).unwrap_or_default();
    let mut used = 0;
    let mut count = 0;
    for turn in rest {
        used += cost(turn);
        if count > 0 && used > budget {
            break;
        }
        count += 1;
    }
    let next = from + count;
    (&rest[..count], (next < all.len()).then_some(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Channel, Segment, SpeakerId, TimeRange};
    use crate::models::ModelRef;

    type Line<'a> = (u16, Option<(u64, u64)>, &'a str);

    fn transcript(segments: &[Line<'_>]) -> Transcript {
        Transcript {
            revision: Revision(3),
            segments: segments
                .iter()
                .map(|(speaker, range, text)| Segment {
                    range: range.and_then(|(a, b)| TimeRange::new(Ms(a), Ms(b))),
                    speaker: SpeakerId(*speaker),
                    channel: Channel::System,
                    text: (*text).to_owned(),
                    language: None,
                })
                .collect(),
            detected_languages: vec![],
            produced_by: ModelRef::parse("fake/stt").unwrap(),
        }
    }

    #[test]
    fn same_speaker_segments_merge_and_names_apply_only_to_their_revision() {
        let t = transcript(&[
            (1, Some((0, 1000)), "Hello"),
            (1, Some((1000, 2000)), "there."),
            (2, Some((2000, 3000)), "Hi."),
            (1, Some((3000, 4000)), "Back."),
        ]);
        let mut names = SpeakerNames::default();
        names.rename(Revision(3), SpeakerId(2), Some("Ana".into()));
        let all = turns(&t, &names, None);
        let summary: Vec<_> = all
            .iter()
            .map(|t| (t.speaker.as_str(), t.text.as_str(), t.start, t.end))
            .collect();
        assert_eq!(
            summary,
            [
                ("Speaker 1", "Hello there.", Some(Ms(0)), Some(Ms(2000))),
                ("Ana", "Hi.", Some(Ms(2000)), Some(Ms(3000))),
                ("Speaker 1", "Back.", Some(Ms(3000)), Some(Ms(4000))),
            ]
        );
        let mut stale = SpeakerNames::default();
        stale.rename(Revision(2), SpeakerId(2), Some("Ana".into()));
        assert_eq!(turns(&t, &stale, None)[1].speaker, "Speaker 2");
    }

    #[test]
    fn a_translated_turn_reads_as_translated_and_keeps_what_was_spoken() {
        let mut t = transcript(&[
            (1, Some((0, 1000)), "Ship it."),
            (2, Some((1000, 2000)), "Sige."),
            (2, Some((2000, 3000)), "Ako na."),
            (2, Some((3000, 4000)), "Okay then."),
        ]);
        let tl = LangTag::parse("tl").unwrap();
        t.segments[1].language = Some(tl.clone());
        t.segments[2].language = Some(tl.clone());
        t.segments[3].language = Some(LangTag::parse("en").unwrap());
        let translation = Translation {
            target: LangTag::parse("en").unwrap(),
            transcript_revision: t.revision,
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            texts: vec![None, Some("Sure.".into()), Some("I'll do it.".into()), None],
        };
        let all = turns(&t, &SpeakerNames::default(), Some(&translation));
        let spoken: Vec<(&str, &str)> = all
            .iter()
            .map(|t| (t.speaker.as_str(), t.text.as_str()))
            .collect();
        assert_eq!(
            spoken,
            [
                ("Speaker 1", "Ship it."),
                ("Speaker 2", "Sure. I'll do it."),
                ("Speaker 2", "Okay then."),
            ],
            "translated and spoken text never share a turn"
        );
        let from: Vec<Option<(Option<LangTag>, &str)>> = all
            .iter()
            .map(|t| {
                t.translated
                    .as_ref()
                    .map(|x| (x.from.clone(), x.original.as_str()))
            })
            .collect();
        assert_eq!(from, [None, Some((Some(tl), "Sige. Ako na.")), None]);
        let mismatched = Translation {
            transcript_revision: Revision(99),
            ..translation
        };
        assert_eq!(
            turns(&t, &SpeakerNames::default(), Some(&mismatched)),
            turns(&t, &SpeakerNames::default(), None),
            "a translation of another revision is ignored"
        );
    }

    #[test]
    fn a_change_of_language_ends_a_turn_of_one_speaker() {
        let mut t = transcript(&[
            (1, Some((0, 1000)), "Sige."),
            (1, Some((1000, 2000)), "Ako na."),
            (1, Some((2000, 3000)), "Okay then."),
            (1, Some((3000, 4000)), "Done."),
            (2, Some((4000, 5000)), "Salamat."),
        ]);
        for (segment, tag) in t.segments.iter_mut().zip(["tl", "tl", "en", "en-US", "tl"]) {
            segment.language = Some(LangTag::parse(tag).unwrap());
        }
        let all = turns(&t, &SpeakerNames::default(), None);
        let got: Vec<(&str, Option<&str>, &str)> = all
            .iter()
            .map(|t| {
                (
                    t.speaker.as_str(),
                    t.language.as_ref().map(LangTag::as_str),
                    t.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("Speaker 1", Some("tl"), "Sige. Ako na."),
                ("Speaker 1", Some("en"), "Okay then. Done."),
                ("Speaker 2", Some("tl"), "Salamat."),
            ],
            "en and en-US are one language"
        );
    }

    #[test]
    fn untimed_legacy_turns_have_no_span() {
        let all = turns(
            &transcript(&[(1, None, "old"), (1, None, "text")]),
            &SpeakerNames::default(),
            None,
        );
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].start, all[0].end), (None, None));
    }

    #[test]
    fn a_long_turn_is_split_and_no_turn_passes_the_cap() {
        let long = "word ".repeat(3_000);
        let all = turns(
            &transcript(&[(1, None, &long), (1, None, "tail")]),
            &SpeakerNames::default(),
            None,
        );
        assert!(all.len() >= 4, "{}", all.len());
        assert!(all.iter().all(|t| t.text.chars().count() <= MAX_TURN_CHARS));
        let rejoined: Vec<&str> = all.iter().flat_map(|t| t.text.split(' ')).collect();
        assert_eq!(rejoined.len(), 3_001, "no word lost or duplicated");
    }

    #[test]
    fn pages_cover_every_turn_once_and_end_on_turn_boundaries() {
        let texts: Vec<String> = (0..300).map(|i| format!("line {i} ").repeat(20)).collect();
        let borrowed: Vec<Line<'_>> = texts
            .iter()
            .zip(0u64..)
            .map(|(t, i)| ((i % 3) as u16, Some((i * 10, i * 10 + 10)), t.as_str()))
            .collect();
        let all = turns(&transcript(&borrowed), &SpeakerNames::default(), None);
        for budget in [MAX_TURN_CHARS, DEFAULT_PAGE_CHARS, MAX_PAGE_CHARS] {
            let mut seen = 0;
            let mut from = Some(0);
            while let Some(at) = from {
                let (turns, next) = page(&all, at, budget);
                assert!(!turns.is_empty());
                assert!(turns.len() == 1 || turns.iter().map(cost).sum::<usize>() <= budget);
                assert_eq!(at, seen, "pages are contiguous");
                seen += turns.len();
                from = next;
            }
            assert_eq!(seen, all.len(), "budget {budget}");
        }
    }

    #[test]
    fn cursors_round_trip_and_sizes_clamp() {
        let c = TurnCursor {
            revision: Revision(3),
            turn: 41,
        };
        assert_eq!(c.to_string(), "r3.t41");
        assert_eq!(TurnCursor::parse("r3.t41"), Some(c));
        for bad in ["", "r3", "3.t41", "r3.41", "rx.t1", "r3.t-1"] {
            assert_eq!(TurnCursor::parse(bad), None, "{bad}");
        }
        assert_eq!(page_chars(None), DEFAULT_PAGE_CHARS);
        assert_eq!(page_chars(Some(10)), MAX_TURN_CHARS);
        assert_eq!(page_chars(Some(1_000_000)), MAX_PAGE_CHARS);
    }
}
