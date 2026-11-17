//! Where a query matches inside text the caller already has: the marks on a title, the turn to
//! open at, and the snippet under a row. Every rule here reads the same [`tokens`] the index
//! stores, so a store hit always has a match to show.

use std::ops::Range;

use super::query::{Clause, Query};
use super::tokens::{Token, tokens};

/// The item a query matches best, and where in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub item: usize,
    pub spans: Vec<Range<usize>>,
}

/// A short window of text around the first match. `marks` are bytes of `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    pub marks: Vec<Range<usize>>,
}

const ELLIPSIS: &str = "…";
/// How many words a snippet shows before its first match.
const LEAD_WORDS: usize = 3;

/// Each place a clause matches, as (clause index, token indexes), in text order.
fn hits<'a>(
    clauses: &'a [Clause],
    words: &'a [Token],
) -> impl Iterator<Item = (usize, Range<usize>)> + 'a {
    (0..words.len()).flat_map(move |at| {
        clauses
            .iter()
            .enumerate()
            .filter_map(move |(index, clause)| matches_at(clause, words, at).map(|r| (index, r)))
    })
}

fn matches_at(clause: &Clause, words: &[Token], at: usize) -> Option<Range<usize>> {
    match clause {
        Clause::Word { alts, prefix } => {
            let word = words[at].folded.as_str();
            alts.iter()
                .any(|alt| match prefix {
                    true => word.starts_with(alt.as_str()),
                    false => word == alt,
                })
                .then_some(at..at + 1)
        }
        Clause::Phrase {
            words: phrase,
            prefix,
        } => {
            let run = words.get(at..at + phrase.len())?;
            let (last, whole) = run.split_last()?;
            let (want_last, want_whole) = phrase.split_last()?;
            let last_matches = match prefix {
                true => last.folded.starts_with(want_last.as_str()),
                false => &last.folded == want_last,
            };
            (last_matches
                && whole
                    .iter()
                    .zip(want_whole)
                    .all(|(w, want)| &w.folded == want))
            .then_some(at..at + phrase.len())
        }
    }
}

/// Which clauses match somewhere in `text`.
fn clauses_in(query: &Query, text: &str) -> Vec<bool> {
    let words: Vec<Token> = tokens(text).collect();
    let mut found = vec![false; query.clauses().len()];
    for (clause, _) in hits(query.clauses(), &words) {
        found[clause] = true;
    }
    found
}

/// True when every clause matches in at least one of `fields`. A phrase must sit inside one field.
/// This is the rule a store's index implements, and the in-memory store's whole search.
pub fn matches(query: &Query, fields: &[&str]) -> bool {
    let mut found = vec![false; query.clauses().len()];
    for field in fields {
        for (seen, here) in found.iter_mut().zip(clauses_in(query, field)) {
            *seen |= here;
        }
    }
    found.into_iter().all(|f| f)
}

/// The bytes of `text` to mark, in order, with overlapping matches merged. A phrase is one span
/// from its first word to its last.
pub fn highlights(query: &Query, text: &str) -> Vec<Range<usize>> {
    let words: Vec<Token> = tokens(text).collect();
    let mut spans: Vec<Range<usize>> = hits(query.clauses(), &words)
        .map(|(_, r)| words[r.start].span.start..words[r.end - 1].span.end)
        .collect();
    spans.sort_by_key(|s| (s.start, s.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start < last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    merged
}

/// The item to open at, such as a turn of a transcript: the first item that holds every clause,
/// else the first that holds any. A phrase split across two items is in neither, so when no item
/// holds any clause, it is the first item with a word of a phrase.
pub fn locate(query: &Query, items: &[&str]) -> Option<Located> {
    let mut first_any = None;
    for (index, item) in items.iter().enumerate() {
        let found = clauses_in(query, item);
        if found.iter().all(|f| *f) {
            first_any = Some(index);
            break;
        }
        if first_any.is_none() && found.iter().any(|f| *f) {
            first_any = Some(index);
        }
    }
    if let Some(item) = first_any {
        return Some(Located {
            item,
            spans: highlights(query, items[item]),
        });
    }
    let words = query.phrase_words()?;
    let item = items
        .iter()
        .position(|item| clauses_in(&words, item).iter().any(|f| *f))?;
    Some(Located {
        item,
        spans: highlights(&words, items[item]),
    })
}

/// About `width` characters of `text` around its first match: up to three words before it, then
/// as many whole words after as fit, with an ellipsis where the window cuts the text. `None` when
/// nothing matches.
pub fn snippet(query: &Query, text: &str, width: usize) -> Option<Snippet> {
    let words: Vec<Token> = tokens(text).collect();
    let first = hits(query.clauses(), &words).next()?.1;
    let chars = |range: Range<usize>| text[range].chars().count();
    let match_start = words[first.start].span.start;
    let mut lead = first.start;
    while lead > 0
        && first.start - lead < LEAD_WORDS
        && chars(words[lead - 1].span.start..match_start) <= width / 3
    {
        lead -= 1;
    }
    let start = if lead == 0 { 0 } else { words[lead].span.start };
    let mut end = words[first.end - 1].span.end;
    let mut fits_all = true;
    for word in &words[first.end..] {
        if chars(start..word.span.end) > width {
            fits_all = false;
            break;
        }
        end = word.span.end;
    }
    if fits_all {
        end = text.len();
    }

    let prefix = if start > 0 { ELLIPSIS } else { "" };
    let mut out = String::with_capacity(end - start + 2 * ELLIPSIS.len());
    out.push_str(prefix);
    out.extend(text[start..end].chars().map(|c| match c {
        '\n' | '\r' | '\t' => ' ',
        c => c,
    }));
    if end < text.len() {
        out.push_str(ELLIPSIS);
    }
    let shift = prefix.len();
    let marks = highlights(query, text)
        .into_iter()
        .filter(|s| s.start < end && s.end > start)
        .map(|s| s.start.max(start) - start + shift..s.end.min(end) - start + shift)
        .collect();
    Some(Snippet { text: out, marks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query::parse(text).unwrap()
    }

    fn marked(text: &str, spans: &[Range<usize>]) -> Vec<String> {
        spans.iter().map(|s| text[s.clone()].to_owned()).collect()
    }

    #[test]
    fn highlight_table() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "budget",
                "The Budget and the budgets.",
                &["Budget", "budgets"],
            ),
            ("bud rev", "review of the budget", &["review", "budget"]),
            ("muller", "Herr Müller und Muller", &["Müller", "Muller"]),
            ("MÜLLER", "Herr Mueller, Herr Muller", &["Muller"]),
            ("strasse", "Hauptstraße, Straße", &["Straße"]),
            ("orsted", "Ørsted's", &["Ørsted"]),
            (
                "\"budget review\"",
                "the budget review, not a review budget",
                &["budget review"],
            ),
            ("\"budget\"", "budget budgets", &["budget"]),
            (
                "e-mail",
                "send an e-mail, or an e mail",
                &["e-mail", "e mail"],
            ),
            (
                "follow-u",
                "a follow-up, a follow upside, follow it",
                &["follow-up", "follow upside"],
            ),
            ("\"follow u\"", "a follow-up, follow u", &["follow u"]),
            ("map", "roadmap", &[]),
            ("東京", "明日東京で", &["東京"]),
            ("re review", "review", &["review"]),
        ];
        for (query, text, want) in cases {
            assert_eq!(
                marked(text, &highlights(&q(query), text)),
                *want,
                "{query:?} in {text:?}"
            );
        }
    }

    #[test]
    fn matches_needs_every_clause_and_a_phrase_inside_one_field() {
        let fields = ["Budget review", "", "", "kilimanjaro trip"];
        assert!(matches(&q("review kili"), &fields));
        assert!(!matches(&q("review zanzibar"), &fields));
        assert!(matches(&q("\"budget review\""), &fields));
        assert!(!matches(&q("\"review kilimanjaro\""), &fields));
        assert!(matches(&q("trip budget"), &fields));
        assert!(matches(&q("e-mai"), &["send an e-mail"]));
        assert!(!matches(&q("\"e mai\""), &["send an e-mail"]));
    }

    #[test]
    fn locate_prefers_the_item_with_every_clause() {
        let turns = [
            "we set the budget",
            "the review is friday",
            "budget review done",
        ];
        assert_eq!(
            locate(&q("review budget"), &turns),
            Some(Located {
                item: 2,
                spans: vec![0..6, 7..13]
            })
        );
        assert_eq!(locate(&q("friday"), &turns).map(|l| l.item), Some(1));
        let notes = ["follow it", "a follow-up plan"];
        assert_eq!(locate(&q("follow-u"), &notes).map(|l| l.item), Some(1));
        assert_eq!(locate(&q("zanzibar"), &turns), None);
    }

    #[test]
    fn a_phrase_split_across_turns_falls_back_to_the_first_turn_with_any_clause() {
        let turns = ["intro", "about the budget", "review it is", "budget again"];
        let found = locate(&q("\"budget review\" again"), &turns).unwrap();
        assert_eq!(found.item, 3, "only turn 3 holds a clause, the word again");
        assert_eq!(marked(turns[3], &found.spans), ["again"]);
        let found = locate(&q("\"budget review\""), &turns).unwrap();
        assert_eq!(
            (found.item, marked(turns[1], &found.spans)),
            (1, vec!["budget".to_owned()]),
            "no turn holds the phrase, so the first turn with one of its words"
        );
        assert_eq!(
            locate(&q("\"budget zanzibar\""), &["intro", "zanzibars"]),
            None
        );
    }

    #[test]
    fn snippet_starts_a_few_words_before_the_match_and_marks_it() {
        let text = "One two three four five six seven eight roadmap nine ten eleven twelve \
                    thirteen fourteen fifteen sixteen";
        let s = snippet(&q("roadmap"), text, 40).unwrap();
        assert_eq!(s.text, "…seven eight roadmap nine ten eleven…");
        assert_eq!(marked(&s.text, &s.marks), ["roadmap"]);

        let s = snippet(&q("one"), "One two three", 40).unwrap();
        assert_eq!(s.text, "One two three");
        assert_eq!(s.marks, vec![0..3]);

        let s = snippet(&q("three"), "One two\nthree.", 40).unwrap();
        assert_eq!(s.text, "One two three.", "a short text shows whole");

        assert_eq!(snippet(&q("zanzibar"), text, 40), None);
    }

    #[test]
    fn snippet_marks_every_match_in_the_window_and_clips_at_its_edges() {
        let text = "budget alpha beta gamma delta budget epsilon zeta eta theta iota budget";
        let s = snippet(&q("budget"), text, 30).unwrap();
        assert_eq!(s.text, "budget alpha beta gamma delta…");
        assert_eq!(marked(&s.text, &s.marks), ["budget"]);
        let s = snippet(&q("budget"), text, 36).unwrap();
        assert_eq!(marked(&s.text, &s.marks), ["budget", "budget"]);

        let long = "x".repeat(10) + " supercalifragilistic tail";
        let s = snippet(&q("super"), &long, 8).unwrap();
        assert_eq!(
            s.text, "…supercalifragilistic…",
            "a match longer than the width shows whole"
        );
        assert_eq!(marked(&s.text, &s.marks), ["supercalifragilistic"]);
    }
}
