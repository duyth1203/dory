//! Scoring a transcript and a summary against what a case expects. Pure: text in, counts out.

use std::ops::Add;

use dory_core::domain::{Sections, list_items, sections};
use dory_core::search::tokens;

use super::case::{Concept, Expected, Kind, Term};

/// The words of `text` as the search index sees them, so scoring and search agree on case,
/// accents, and punctuation.
pub fn words(text: &str) -> Vec<String> {
    tokens(text).map(|t| t.folded).collect()
}

/// Word errors of a transcript against its reference. `rate` is only meaningful as a share of
/// `reference`, so corpus totals add the counts and take the rate once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WordErrors {
    pub reference: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
}

impl WordErrors {
    pub fn errors(&self) -> usize {
        self.substitutions + self.deletions + self.insertions
    }

    /// Errors per reference word. `None` when the reference is empty.
    pub fn rate(&self) -> Option<f64> {
        (self.reference > 0).then(|| self.errors() as f64 / self.reference as f64)
    }
}

impl Add for WordErrors {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            reference: self.reference + other.reference,
            substitutions: self.substitutions + other.substitutions,
            deletions: self.deletions + other.deletions,
            insertions: self.insertions + other.insertions,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Cell {
    cost: usize,
    substitutions: usize,
    deletions: usize,
    insertions: usize,
}

/// The cheapest edit script from `reference` to `hypothesis`, counted by kind. Two rolling rows
/// carry each cell's counts, so a 9,000-word meeting needs kilobytes, not a full matrix. Equal
/// cost goes to a match or substitution first, then a deletion, then an insertion, so the split
/// is the same on every run.
pub fn word_errors(reference: &[String], hypothesis: &[String]) -> WordErrors {
    let mut previous: Vec<Cell> = (0..=hypothesis.len())
        .map(|j| Cell {
            cost: j,
            insertions: j,
            ..Cell::default()
        })
        .collect();
    let mut current = previous.clone();
    for (i, spoken) in reference.iter().enumerate() {
        current[0] = Cell {
            cost: i + 1,
            deletions: i + 1,
            ..Cell::default()
        };
        for (j, heard) in hypothesis.iter().enumerate() {
            let mut best = previous[j];
            if spoken != heard {
                best.cost += 1;
                best.substitutions += 1;
            }
            let mut delete = previous[j + 1];
            delete.cost += 1;
            delete.deletions += 1;
            let mut insert = current[j];
            insert.cost += 1;
            insert.insertions += 1;
            for other in [delete, insert] {
                if other.cost < best.cost {
                    best = other;
                }
            }
            current[j + 1] = best;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let end = previous[hypothesis.len()];
    WordErrors {
        reference: reference.len(),
        substitutions: end.substitutions,
        deletions: end.deletions,
        insertions: end.insertions,
    }
}

/// How many of the terms the reference says the transcript says too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermHits {
    pub expected: usize,
    pub found: usize,
}

impl TermHits {
    pub fn recall(&self) -> Option<f64> {
        (self.expected > 0).then(|| self.found as f64 / self.expected as f64)
    }
}

impl Add for TermHits {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            expected: self.expected + other.expected,
            found: self.found + other.found,
        }
    }
}

/// Counts each term's non-overlapping occurrences as a folded word sequence. A term said `n`
/// times, by its own count or else by the reference, is expected `n` times, and the transcript
/// earns at most `n` of them, so saying it more often never hides a miss elsewhere. A term the
/// reference never says counts for nothing and comes back in the list, because it is probably a
/// typo in `terms.txt`.
pub fn term_hits(
    terms: &[Term],
    reference: Option<&[String]>,
    hypothesis: &[String],
) -> (TermHits, Vec<String>) {
    let mut hits = TermHits::default();
    let mut absent = Vec::new();
    for term in terms {
        let needle = words(&term.text);
        let wanted = term
            .count
            .unwrap_or_else(|| reference.map_or(0, |r| occurrences(r, &needle)));
        if wanted == 0 {
            absent.push(term.text.clone());
            continue;
        }
        hits.expected += wanted;
        hits.found += occurrences(hypothesis, &needle).min(wanted);
    }
    (hits, absent)
}

fn occurrences(haystack: &[String], needle: &[String]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let (mut count, mut at) = (0, 0);
    while at + needle.len() <= haystack.len() {
        if haystack[at..at + needle.len()] == *needle {
            count += 1;
            at += needle.len();
        } else {
            at += 1;
        }
    }
    count
}

/// Whether any alternative of the concept appears in the bullet as consecutive words.
pub fn concept_matches(concept: &Concept, bullet: &[String]) -> bool {
    concept
        .0
        .iter()
        .any(|alternative| occurrences(bullet, alternative) > 0)
}

/// Whether the bullet carries every concept of the item.
pub fn item_matches(item: &Expected, bullet: &[String]) -> bool {
    item.concepts
        .iter()
        .all(|concept| concept_matches(concept, bullet))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SectionScore {
    pub expected: usize,
    pub found: usize,
    pub extra: usize,
}

impl Add for SectionScore {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            expected: self.expected + other.expected,
            found: self.found + other.found,
            extra: self.extra + other.extra,
        }
    }
}

/// Scores one summary section. `expected` is the gold items for the section and `bullets` is
/// the section's list items, each as folded words. Each bullet pairs with at most one item, so
/// `found` counts items that got a bullet and `extra` counts bullets left unpaired, a repeated
/// item included. Items with the fewest matching bullets pick first, so a broad item never takes
/// the only bullet a narrow one could use.
pub fn score_section(expected: &[&Expected], bullets: &[Vec<String>]) -> SectionScore {
    let candidates = |item: &Expected| -> Vec<usize> {
        (0..bullets.len())
            .filter(|&ix| item_matches(item, &bullets[ix]))
            .collect()
    };
    let mut items: Vec<Vec<usize>> = expected.iter().map(|item| candidates(item)).collect();
    items.sort_by_key(Vec::len);
    let mut used = vec![false; bullets.len()];
    let mut found = 0;
    for candidates in items {
        if let Some(&ix) = candidates.iter().find(|&&ix| !used[ix]) {
            used[ix] = true;
            found += 1;
        }
    }
    SectionScore {
        expected: expected.len(),
        found,
        extra: used.iter().filter(|&&taken| !taken).count(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SummaryScore {
    /// The summary had the three sections Dory writes. When false, both scores read the list
    /// items of the whole text.
    pub structured: bool,
    pub actions: SectionScore,
    pub decisions: SectionScore,
}

/// Action items are scored against the action items section and decisions against the decisions
/// section.
pub fn score_summary(markdown: &str, expected: &[Expected]) -> SummaryScore {
    let (structured, action_items, decisions) = match sections(markdown) {
        Sections::Three {
            decisions,
            action_items,
            ..
        } => (true, list_items(&action_items), list_items(&decisions)),
        Sections::Unstructured => {
            let all = list_items(markdown);
            (false, all.clone(), all)
        }
    };
    let section = |kind: Kind, items: Vec<String>| {
        let gold: Vec<&Expected> = expected.iter().filter(|e| e.kind == kind).collect();
        let bullets: Vec<Vec<String>> = items.iter().map(|item| words(item)).collect();
        score_section(&gold, &bullets)
    };
    SummaryScore {
        structured,
        actions: section(Kind::Action, action_items),
        decisions: section(Kind::Decision, decisions),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::case::parse_expected;

    fn w(text: &str) -> Vec<String> {
        words(text)
    }

    fn errors(reference: &str, hypothesis: &str) -> WordErrors {
        word_errors(&w(reference), &w(hypothesis))
    }

    /// The edit distance by the textbook full matrix, to check the rolling rows against.
    fn distance(a: &[String], b: &[String]) -> usize {
        let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in d[0].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let swap = usize::from(a[i - 1] != b[j - 1]);
                d[i][j] = (d[i - 1][j - 1] + swap)
                    .min(d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1);
            }
        }
        d[a.len()][b.len()]
    }

    #[test]
    fn words_fold_case_accents_and_punctuation() {
        assert_eq!(
            w("Café, ORDERED-log: 3.5!"),
            ["cafe", "ordered", "log", "3", "5"]
        );
        assert!(w("  ...  ").is_empty());
    }

    #[test]
    fn identical_text_has_no_errors() {
        let e = errors("ship it on friday", "Ship it, on Friday.");
        assert_eq!((e.reference, e.errors()), (4, 0));
        assert_eq!(e.rate(), Some(0.0));
    }

    #[test]
    fn each_kind_of_error_is_counted_by_kind() {
        let sub = errors("a b c", "a x c");
        assert_eq!(
            (sub.substitutions, sub.deletions, sub.insertions),
            (1, 0, 0)
        );
        let del = errors("a b c", "a c");
        assert_eq!(
            (del.substitutions, del.deletions, del.insertions),
            (0, 1, 0)
        );
        let ins = errors("a c", "a b c");
        assert_eq!(
            (ins.substitutions, ins.deletions, ins.insertions),
            (0, 0, 1)
        );
        let all = errors("the quick brown fox", "quick red fox jumps");
        assert_eq!(all.errors(), 3);
        assert_eq!(all.rate(), Some(0.75));
    }

    #[test]
    fn an_empty_side_is_all_deletions_or_all_insertions() {
        let lost = errors("a b c", "");
        assert_eq!((lost.deletions, lost.errors()), (3, 3));
        assert_eq!(lost.rate(), Some(1.0));
        let made_up = errors("", "a b");
        assert_eq!((made_up.insertions, made_up.errors()), (2, 2));
        assert_eq!(made_up.rate(), None);
        assert_eq!(errors("", "").rate(), None);
    }

    #[test]
    fn the_split_always_sums_to_the_edit_distance() {
        let texts = [
            "",
            "a",
            "a b c d e f",
            "f e d c b a",
            "a b c a b c a b c",
            "b c a b c a",
            "the cat sat on the mat",
            "a cat sat on a mat today",
            "x y z",
            "a a a a",
            "a",
            "a b a b a b a b",
        ];
        for r in texts {
            for h in texts {
                let (r, h) = (w(r), w(h));
                let e = word_errors(&r, &h);
                assert_eq!(e.errors(), distance(&r, &h), "{r:?} vs {h:?}");
                assert_eq!(e.reference, r.len());
                assert_eq!(
                    e.reference + e.insertions - e.deletions,
                    h.len(),
                    "lengths balance for {r:?} vs {h:?}"
                );
            }
        }
    }

    #[test]
    fn a_tie_goes_to_substitutions_before_a_delete_and_insert_pair() {
        let swapped = errors("a b", "b a");
        assert_eq!(
            (swapped.substitutions, swapped.deletions, swapped.insertions),
            (2, 0, 0)
        );
    }

    #[test]
    fn corpus_totals_add_counts_not_rates() {
        let short = errors("a b", "a x");
        let long = errors("a b c d e f g h i j", "a b c d e f g h i j");
        let total = short + long;
        assert_eq!((total.reference, total.errors()), (12, 1));
        let rate = total.rate().unwrap();
        assert!((rate - 1.0 / 12.0).abs() < 1e-12);
        let mean_of_rates = (short.rate().unwrap() + long.rate().unwrap()) / 2.0;
        assert!((rate - mean_of_rates).abs() > 0.1);
    }

    fn terms(texts: &[&str]) -> Vec<Term> {
        texts
            .iter()
            .map(|text| Term {
                text: (*text).to_owned(),
                count: None,
            })
            .collect()
    }

    #[test]
    fn terms_are_counted_as_word_sequences_and_capped_at_the_reference() {
        let reference = w("Omar met Lena Park. Omar left. Lena Park stayed.");
        let heard = w("omar met lena park omar omar omar left lena stayed");
        let (hits, absent) = term_hits(&terms(&["Omar", "Lena Park"]), Some(&reference), &heard);
        assert_eq!((hits.expected, hits.found), (4, 3));
        assert!(absent.is_empty());
        assert_eq!(hits.recall(), Some(0.75));
    }

    #[test]
    fn a_term_the_reference_never_says_is_listed_and_not_counted() {
        let (hits, absent) = term_hits(
            &terms(&["Kubernetes", "Omar"]),
            Some(&w("Omar spoke")),
            &w("omar spoke"),
        );
        assert_eq!((hits.expected, hits.found), (1, 1));
        assert_eq!(absent, ["Kubernetes"]);
        assert_eq!(TermHits::default().recall(), None);
    }

    #[test]
    fn a_counted_term_needs_no_reference_and_is_capped_at_its_count() {
        let counted = [Term {
            text: "Maria will finish the billing dashboard".to_owned(),
            count: Some(2),
        }];
        let heard = w(
            "Maria will finish the billing dashboard. Maria will finish the billing \
                       dashboard. Maria will finish the billing dashboard. Mario will finish",
        );
        let (hits, absent) = term_hits(&counted, None, &heard);
        assert_eq!((hits.expected, hits.found), (2, 2));
        assert!(absent.is_empty());
        let (hits, _) = term_hits(
            &counted,
            None,
            &w("Mario will finish the billing dashboard"),
        );
        assert_eq!((hits.expected, hits.found), (2, 0));
    }

    #[test]
    fn term_occurrences_do_not_overlap() {
        let (hits, _) = term_hits(&terms(&["a a"]), Some(&w("a a a a a")), &w("a a a a a"));
        assert_eq!((hits.expected, hits.found), (2, 2));
    }

    #[test]
    fn a_concept_matches_any_alternative_as_consecutive_words() {
        let concept = Concept(vec![w("deck"), w("sales slides")]);
        assert!(concept_matches(&concept, &w("Omar updates the deck")));
        assert!(concept_matches(&concept, &w("refresh the Sales Slides.")));
        assert!(!concept_matches(&concept, &w("slides for the sales team")));
        assert!(!concept_matches(&concept, &w("decks")));
        assert!(!concept_matches(&concept, &[]));
    }

    #[test]
    fn an_item_needs_every_concept_in_one_bullet() {
        let items = parse_expected("action: omar, deck|slides, friday\n").unwrap();
        let item = &items[0];
        assert!(item_matches(item, &w("Omar to send the slides by Friday")));
        assert!(!item_matches(item, &w("Omar to send the slides")));
        assert!(!item_matches(item, &w("Lena to send the deck by Friday")));
    }

    #[test]
    fn section_scores_add() {
        let a = SectionScore {
            expected: 2,
            found: 1,
            extra: 3,
        };
        let b = SectionScore {
            expected: 2,
            found: 2,
            extra: 0,
        };
        assert_eq!(
            a + b,
            SectionScore {
                expected: 4,
                found: 3,
                extra: 3
            }
        );
    }

    fn score(markdown: &str, expected: &str) -> SummaryScore {
        score_summary(markdown, &parse_expected(expected).unwrap())
    }

    fn section(expected: usize, found: usize, extra: usize) -> SectionScore {
        SectionScore {
            expected,
            found,
            extra,
        }
    }

    #[test]
    fn each_kind_is_scored_in_its_own_section_and_key_points_never_count() {
        let got = score(
            "## Key points\n- Omar owns the pricing deck\n\n## Decisions\n- Ship the beta on Monday\n\n\
             ## Action items\n- Omar sends the pricing deck by Friday\n- Ana books the room\n",
            "action: omar, pricing, deck|slides, friday\ndecision: ship, beta, monday\n",
        );
        assert!(got.structured);
        assert_eq!(got.actions, section(1, 1, 1));
        assert_eq!(got.decisions, section(1, 1, 0));
    }

    #[test]
    fn a_repeated_item_finds_once_and_its_copy_is_extra() {
        let got = score(
            "## Key points\nNone.\n## Decisions\nNone.\n## Action items\n\
             - Omar sends the deck Friday\n- Omar sends the deck Friday\n",
            "action: omar, deck, friday\n",
        );
        assert_eq!(got.actions, section(1, 1, 1));
    }

    #[test]
    fn a_broad_item_leaves_a_narrow_item_its_only_bullet() {
        let got = score(
            "## Key points\nNone.\n## Decisions\nNone.\n## Action items\n\
             - Omar sends the deck Friday\n- Omar calls the vendor\n",
            "action: omar\naction: omar, deck, friday\n",
        );
        assert_eq!(got.actions, section(2, 2, 0));
    }

    #[test]
    fn a_missed_item_is_not_found_and_a_summary_without_sections_reads_every_bullet() {
        let got = score(
            "Notes\n- Omar sends the deck Friday\n",
            "action: omar, deck, friday\ndecision: ship, beta\n",
        );
        assert!(!got.structured);
        assert_eq!(got.actions, section(1, 1, 0));
        assert_eq!(got.decisions, section(1, 0, 1));
    }
}
