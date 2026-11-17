//! A transcript as speaker turns, a page at a time.
//!
//! Consecutive segments by the same speaker merge into one turn of at most [`MAX_TURN_CHARS`], so
//! any page budget of at least that size holds at least one turn. A cursor names the transcript
//! revision and the next turn, so a page from a transcript that changed since is refused rather
//! than spliced into the new one.

use crate::domain::{Ms, Revision, SpeakerNames, Transcript};

pub const MAX_TURN_CHARS: usize = 4_000;
pub const DEFAULT_PAGE_CHARS: usize = 20_000;
pub const MAX_PAGE_CHARS: usize = 40_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub speaker: String,
    /// `None` for transcripts imported without times.
    pub start: Option<Ms>,
    pub end: Option<Ms>,
    pub text: String,
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

/// Every turn of a transcript, speakers named as the summary names them.
pub fn turns(transcript: &Transcript, names: &SpeakerNames) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for segment in &transcript.segments {
        let speaker = names.label(segment, transcript.revision);
        let start = segment.range.map(|r| r.start());
        let end = segment.range.map(|r| r.end());
        for piece in pieces(&segment.text, MAX_TURN_CHARS) {
            match out.last_mut() {
                Some(last)
                    if last.speaker == speaker
                        && last.text.chars().count() + 1 + piece.chars().count()
                            <= MAX_TURN_CHARS =>
                {
                    last.text.push(' ');
                    last.text.push_str(&piece);
                    last.end = end.or(last.end);
                }
                _ => out.push(Turn {
                    speaker: speaker.clone(),
                    start,
                    end,
                    text: piece,
                }),
            }
        }
    }
    out
}

/// The size a turn takes on a page: its text, its speaker, and room for its time stamp.
fn cost(turn: &Turn) -> usize {
    turn.text.chars().count() + turn.speaker.chars().count() + 16
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
        let all = turns(&t, &names);
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
        assert_eq!(turns(&t, &stale)[1].speaker, "Speaker 2");
    }

    #[test]
    fn untimed_legacy_turns_have_no_span() {
        let all = turns(
            &transcript(&[(1, None, "old"), (1, None, "text")]),
            &SpeakerNames::default(),
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
        let all = turns(&transcript(&borrowed), &SpeakerNames::default());
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
