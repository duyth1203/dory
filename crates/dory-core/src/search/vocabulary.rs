//! Every word a library holds, and the typo rule that finds the ones a typed word may have meant.

/// Shorter terms are never offered as a correction.
const MIN_TERM_LETTERS: usize = 4;
/// Shorter typed words are never corrected, because one edit changes too much of them.
const MIN_TYPED_LETTERS: usize = 5;
/// From this length a typed word may be two edits away.
const TWO_EDITS_LETTERS: usize = 9;

/// The sorted, folded terms of four letters or more that a store indexed, and the store
/// generation they were read at, so a cache can tell when a write made it stale.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vocabulary {
    generation: u64,
    terms: Vec<String>,
}

impl Vocabulary {
    /// Keeps the terms of four letters or more, sorted and without repeats. `terms` must already
    /// be folded, as the index stores them.
    pub fn new(generation: u64, terms: impl IntoIterator<Item = String>) -> Self {
        let mut terms: Vec<String> = terms
            .into_iter()
            .filter(|t| t.chars().count() >= MIN_TERM_LETTERS)
            .collect();
        if !terms.is_sorted() {
            terms.sort_unstable();
        }
        terms.dedup();
        Self { generation, terms }
    }

    /// The store generation this was read at.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// Whether some term starts with `prefix`.
    pub fn starts_any(&self, prefix: &str) -> bool {
        let at = self.terms.partition_point(|t| t.as_str() < prefix);
        self.terms.get(at).is_some_and(|t| t.starts_with(prefix))
    }

    /// The terms within the edits [`allowed_edits`] gives `word`, nearest first, then in term
    /// order. Empty for a word that is never corrected.
    pub fn near(&self, word: &str) -> Vec<String> {
        let limit = allowed_edits(word);
        if limit == 0 {
            return Vec::new();
        }
        let word: Vec<char> = word.chars().collect();
        let mut scratch = Scratch::default();
        let mut found: Vec<(usize, &str)> = Vec::new();
        for term in &self.terms {
            // Every char is at least one byte, so a term this much longer is out of reach, and
            // it spares counting its chars.
            if term.len() > (word.len() + limit) * 4 {
                continue;
            }
            scratch.term.clear();
            scratch.term.extend(term.chars());
            if let Some(d) = scratch.distance(&word, limit) {
                found.push((d, term));
            }
        }
        found.sort_by_key(|(d, _)| *d);
        found.into_iter().map(|(_, t)| t.to_owned()).collect()
    }
}

/// How many edits a typed word may be from a term: none below five letters, one up to eight, and
/// two from nine.
pub(super) fn allowed_edits(word: &str) -> usize {
    match word.chars().count() {
        n if n < MIN_TYPED_LETTERS => 0,
        n if n < TWO_EDITS_LETTERS => 1,
        _ => 2,
    }
}

/// Buffers for [`Scratch::distance`], so a scan over every term allocates once.
#[derive(Default)]
struct Scratch {
    term: Vec<char>,
    two_back: Vec<usize>,
    back: Vec<usize>,
    row: Vec<usize>,
}

impl Scratch {
    /// The optimal string alignment distance from `word` to the term in `self.term`, when it is
    /// at most `limit`. An edit adds, drops, or changes one letter, or swaps two neighbours. Stops
    /// at the first row whose every cell is over `limit`, because no cell of a later row is
    /// lower than the smallest of the row before it.
    fn distance(&mut self, word: &[char], limit: usize) -> Option<usize> {
        let term = &self.term;
        if word.len().abs_diff(term.len()) > limit {
            return None;
        }
        let m = term.len();
        self.two_back.clear();
        self.two_back.resize(m + 1, 0);
        self.back.clear();
        self.back.extend(0..=m);
        self.row.clear();
        self.row.resize(m + 1, 0);
        for i in 1..=word.len() {
            self.row[0] = i;
            let mut best = i;
            for j in 1..=m {
                let cost = usize::from(word[i - 1] != term[j - 1]);
                let mut v = (self.back[j] + 1)
                    .min(self.row[j - 1] + 1)
                    .min(self.back[j - 1] + cost);
                if i > 1 && j > 1 && word[i - 1] == term[j - 2] && word[i - 2] == term[j - 1] {
                    v = v.min(self.two_back[j - 2] + 1);
                }
                self.row[j] = v;
                best = best.min(v);
            }
            if best > limit {
                return None;
            }
            std::mem::swap(&mut self.two_back, &mut self.back);
            std::mem::swap(&mut self.back, &mut self.row);
        }
        let d = self.back[m];
        (d <= limit).then_some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: &str, b: &str, limit: usize) -> Option<usize> {
        let mut scratch = Scratch {
            term: b.chars().collect(),
            ..Scratch::default()
        };
        scratch.distance(&a.chars().collect::<Vec<_>>(), limit)
    }

    /// The full matrix, with no early exit.
    fn reference(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in d[0].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                let mut v = (d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1)
                    .min(d[i - 1][j - 1] + cost);
                if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    v = v.min(d[i - 2][j - 2] + 1);
                }
                d[i][j] = v;
            }
        }
        d[a.len()][b.len()]
    }

    #[test]
    fn distance_table() {
        let cases = [
            ("roadmpa", "roadmap", 1, Some(1)),
            ("budgte", "budget", 1, Some(1)),
            ("roadmap", "roadmap", 1, Some(0)),
            ("kilimanjro", "kilimanjaro", 1, Some(1)),
            ("roadmapp", "roadmap", 1, Some(1)),
            ("roadmop", "roadmap", 1, Some(1)),
            ("mueller", "muller", 1, Some(1)),
            ("antinatoinal", "antinational", 2, Some(1)),
            ("antinatinal", "antinational", 2, Some(1)),
            ("recieveing", "receiving", 2, Some(2)),
            ("roadmpa", "roadmaps", 1, None),
            ("roadmpa", "roadmaps", 2, Some(2)),
            ("atlsa", "atlas", 1, Some(1)),
            ("budget", "gadget", 1, None),
            ("abcdef", "abcdefghi", 2, None),
        ];
        for (typed, term, limit, want) in cases {
            assert_eq!(distance(typed, term, limit), want, "{typed} to {term}");
        }
    }

    #[test]
    fn distance_agrees_with_the_full_matrix() {
        let letters = ['a', 'b', 'c'];
        let mut seed = 0x2545_f491_u32;
        let mut next = move |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize % bound
        };
        for _ in 0..3000 {
            let make = |next: &mut dyn FnMut(usize) -> usize| -> String {
                (0..next(8) + 1).map(|_| letters[next(3)]).collect()
            };
            let (a, b) = (make(&mut next), make(&mut next));
            for limit in 0..=2 {
                let full = reference(&a, &b);
                assert_eq!(
                    distance(&a, &b, limit),
                    (full <= limit).then_some(full),
                    "{a} to {b} within {limit}"
                );
            }
        }
    }

    #[test]
    fn edits_grow_with_the_word() {
        for (word, edits) in [
            ("atls", 0),
            ("budge", 1),
            ("roadmap", 1),
            ("kilimanj", 1),
            ("antinatio", 2),
            ("antinational", 2),
            ("ångström", 1),
        ] {
            assert_eq!(allowed_edits(word), edits, "{word}");
        }
    }

    fn vocabulary(terms: &[&str]) -> Vocabulary {
        Vocabulary::new(7, terms.iter().map(|t| t.to_string()))
    }

    #[test]
    fn terms_are_sorted_distinct_and_four_letters_or_more() {
        let v = vocabulary(&["zeta", "the", "alpha", "zeta", "ab", "über", "alph"]);
        assert_eq!(v.terms(), ["alph", "alpha", "zeta", "über"]);
        assert_eq!(v.generation(), 7);
    }

    #[test]
    fn starts_any_finds_a_term_with_that_start() {
        let v = vocabulary(&["roadmap", "roadmaps", "budget"]);
        assert!(v.starts_any("roadm"));
        assert!(v.starts_any("roadmaps"));
        assert!(v.starts_any("budget"));
        assert!(!v.starts_any("roadmpa"));
        assert!(!v.starts_any("roadmapss"));
        assert!(!v.starts_any("zzzzz"));
        assert!(!vocabulary(&[]).starts_any("roadm"));
    }

    #[test]
    fn near_lists_terms_within_the_edits_nearest_first() {
        let v = vocabulary(&[
            "roadmap",
            "roadmaps",
            "budget",
            "gadget",
            "antinational",
            "antinationalist",
            "muller",
            "atlas",
        ]);
        assert_eq!(v.near("roadmpa"), ["roadmap"], "a swap");
        assert_eq!(v.near("budgte"), ["budget"]);
        assert_eq!(v.near("roadma"), ["roadmap"], "a drop");
        assert_eq!(v.near("roadmapz"), ["roadmap", "roadmaps"], "nearest first");
        assert_eq!(v.near("roadmxp"), ["roadmap"], "a change");
        assert_eq!(v.near("mueller"), ["muller"], "an insert");
        assert_eq!(
            v.near("antinatoinal"),
            ["antinational"],
            "two edits at nine"
        );
        assert_eq!(v.near("atlsa"), ["atlas"], "five letters");
        assert_eq!(v.near("atls"), Vec::<String>::new(), "four letters");
        assert_eq!(v.near("zzzzzz"), Vec::<String>::new());
    }

    #[test]
    fn two_edits_only_from_nine_letters() {
        let v = vocabulary(&["antinational", "roadmap"]);
        assert_eq!(v.near("antinatoinl"), ["antinational"], "eleven letters");
        assert_eq!(
            v.near("roadmpas"),
            Vec::<String>::new(),
            "eight letters, two edits"
        );
    }
}
