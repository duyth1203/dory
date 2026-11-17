//! What the user typed, as clauses a store can look up and a view can highlight.

use super::tokens::tokens;
use super::vocabulary::{Vocabulary, allowed_edits};

/// Typed search text, parsed. Every clause must match somewhere in a recording for it to be a hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    clauses: Vec<Clause>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clause {
    /// Any word that starts with one of `alts`, or equals one when `prefix` is false. Each alt is
    /// one folded token.
    Word { alts: Vec<String>, prefix: bool },
    /// Whole folded words, next to each other, in this order.
    Phrase(Vec<String>),
}

/// A typed word that [`Query::corrected`] replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// The word as typed, folded.
    pub typed: String,
    /// The terms that stand in for it, nearest first.
    pub used: Vec<String>,
}

/// Straight and curly double quotes both open and close a phrase.
fn is_quote(c: char) -> bool {
    matches!(c, '"' | '“' | '”' | '„')
}

impl Query {
    /// `None` when the text holds no word. Unquoted words become [`Clause::Word`] prefixes. A
    /// quoted run becomes a [`Clause::Phrase`], and so does a typed word that splits into several
    /// tokens, such as "e-mail", "don't", or "東京". A quote without its pair runs to the end.
    pub fn parse(text: &str) -> Option<Query> {
        let mut clauses = Vec::new();
        for (index, part) in text.split(is_quote).enumerate() {
            let quoted = index % 2 == 1;
            if quoted {
                let words: Vec<String> = tokens(part).map(|t| t.folded).collect();
                if !words.is_empty() {
                    clauses.push(Clause::Phrase(words));
                }
                continue;
            }
            for typed in part.split_whitespace() {
                let mut words: Vec<String> = tokens(typed).map(|t| t.folded).collect();
                match words.len() {
                    0 => {}
                    1 => clauses.push(Clause::Word {
                        alts: vec![words.remove(0)],
                        prefix: true,
                    }),
                    _ => clauses.push(Clause::Phrase(words)),
                }
            }
        }
        (!clauses.is_empty()).then_some(Query { clauses })
    }

    /// The typed word of a clause that [`Query::corrected`] may replace: one unquoted word of five
    /// letters or more. A number or a code is never a typo of another.
    fn typed_word(clause: &Clause) -> Option<&str> {
        match clause {
            Clause::Word { alts, prefix: true } => match alts.as_slice() {
                [word] if word.chars().all(char::is_alphabetic) && allowed_edits(word) > 0 => {
                    Some(word)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether any clause is a word [`Query::corrected`] could replace, so a caller can skip
    /// reading the vocabulary when none is.
    pub fn is_correctable(&self) -> bool {
        self.clauses.iter().any(|c| Self::typed_word(c).is_some())
    }

    /// The query with each long word that starts no term of `vocabulary` replaced by the terms
    /// within one edit of it, or two edits from nine letters. An edit adds, drops, or changes one
    /// letter, or swaps two neighbours. A word that starts a term, a short word, and a phrase stay
    /// as typed. `None` when no word changed. A replaced clause matches whole terms.
    pub fn corrected(&self, vocabulary: &Vocabulary) -> Option<(Query, Vec<Correction>)> {
        let mut corrections = Vec::new();
        let clauses = self
            .clauses
            .iter()
            .map(|clause| {
                let Some(typed) = Self::typed_word(clause) else {
                    return clause.clone();
                };
                if vocabulary.starts_any(typed) {
                    return clause.clone();
                }
                let used = vocabulary.near(typed);
                if used.is_empty() {
                    return clause.clone();
                }
                corrections.push(Correction {
                    typed: typed.to_owned(),
                    used: used.clone(),
                });
                Clause::Word {
                    alts: used,
                    prefix: false,
                }
            })
            .collect();
        (!corrections.is_empty()).then_some((Query { clauses }, corrections))
    }

    /// For store adapters, which compile the clauses to their own lookup.
    pub fn clauses(&self) -> &[Clause] {
        &self.clauses
    }

    /// Each word of each phrase as a whole word of its own, for finding where a phrase that no
    /// single item holds still has its words. `None` without a phrase.
    pub(super) fn phrase_words(&self) -> Option<Query> {
        let mut clauses: Vec<Clause> = Vec::new();
        for clause in &self.clauses {
            if let Clause::Phrase(words) = clause {
                for word in words {
                    let loose = Clause::Word {
                        alts: vec![word.clone()],
                        prefix: false,
                    };
                    if !clauses.contains(&loose) {
                        clauses.push(loose);
                    }
                }
            }
        }
        (!clauses.is_empty()).then_some(Query { clauses })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(w: &str) -> Clause {
        Clause::Word {
            alts: vec![w.into()],
            prefix: true,
        }
    }

    fn phrase(words: &[&str]) -> Clause {
        Clause::Phrase(words.iter().map(|w| w.to_string()).collect())
    }

    #[test]
    fn clause_table() {
        let cases: Vec<(&str, Option<Vec<Clause>>)> = vec![
            ("", None),
            ("   \t", None),
            ("!!! -- ?", None),
            ("\"\"", None),
            ("budget", Some(vec![word("budget")])),
            (
                "  Budget   REVIEW ",
                Some(vec![word("budget"), word("review")]),
            ),
            ("Müller", Some(vec![word("muller")])),
            (
                "\"budget review\"",
                Some(vec![phrase(&["budget", "review"])]),
            ),
            ("\"budget\"", Some(vec![phrase(&["budget"])])),
            (
                "kickoff \"budget review\" q3",
                Some(vec![
                    word("kickoff"),
                    phrase(&["budget", "review"]),
                    word("q3"),
                ]),
            ),
            (
                "“budget review” atlas",
                Some(vec![phrase(&["budget", "review"]), word("atlas")]),
            ),
            (
                "atlas \"budget review",
                Some(vec![word("atlas"), phrase(&["budget", "review"])]),
            ),
            ("e-mail", Some(vec![phrase(&["e", "mail"])])),
            ("don't", Some(vec![phrase(&["don", "t"])])),
            ("Q3.5", Some(vec![phrase(&["q3", "5"])])),
            (
                "or near NOT",
                Some(vec![word("or"), word("near"), word("not")]),
            ),
            ("title:atlas", Some(vec![phrase(&["title", "atlas"])])),
            ("budget*", Some(vec![word("budget")])),
            (
                "東京 budget",
                Some(vec![phrase(&["東", "京"]), word("budget")]),
            ),
            ("会議budget", Some(vec![phrase(&["会", "議", "budget"])])),
            ("会", Some(vec![word("会")])),
        ];
        for (text, want) in cases {
            assert_eq!(
                Query::parse(text).map(|q| q.clauses().to_vec()),
                want,
                "{text:?}"
            );
        }
    }

    fn vocabulary(terms: &[&str]) -> Vocabulary {
        Vocabulary::new(1, terms.iter().map(|t| t.to_string()))
    }

    fn parsed(text: &str) -> Query {
        Query::parse(text).unwrap()
    }

    fn whole(terms: &[&str]) -> Clause {
        Clause::Word {
            alts: terms.iter().map(|t| t.to_string()).collect(),
            prefix: false,
        }
    }

    #[test]
    fn a_misspelt_word_becomes_the_terms_near_it() {
        let v = vocabulary(&["roadmap", "budget", "atlas"]);
        let (query, corrections) = parsed("Roadmpa").corrected(&v).unwrap();
        assert_eq!(query.clauses(), [whole(&["roadmap"])]);
        assert_eq!(
            corrections,
            [Correction {
                typed: "roadmpa".into(),
                used: vec!["roadmap".into()]
            }]
        );
        let (query, _) = parsed("atlsa").corrected(&v).unwrap();
        assert_eq!(query.clauses(), [whole(&["atlas"])], "five letters");
    }

    #[test]
    fn a_word_that_starts_a_term_stays_exact() {
        let v = vocabulary(&["roadmap", "roadmaps", "budget"]);
        assert_eq!(parsed("roadmap").corrected(&v), None);
        assert_eq!(parsed("roadm").corrected(&v), None);
        assert_eq!(parsed("budget").corrected(&v), None);
    }

    #[test]
    fn a_phrase_a_short_word_and_a_number_are_never_corrected() {
        let v = vocabulary(&["roadmap", "atlas", "2024"]);
        assert_eq!(parsed("\"roadmpa\"").corrected(&v), None);
        assert_eq!(
            parsed("e-mailz roadmpa").corrected(&v).map(|c| c.1.len()),
            Some(1)
        );
        assert_eq!(parsed("atls").corrected(&v), None, "four letters");
        assert_eq!(parsed("20244").corrected(&v), None, "digits");
        assert!(!parsed("\"roadmpa\" atls 20244").is_correctable());
        assert!(parsed("atlas roadmpa").is_correctable());
    }

    #[test]
    fn only_the_misspelt_word_of_a_query_changes() {
        let v = vocabulary(&["roadmap", "budget", "kilimanjaro"]);
        let (query, corrections) = parsed("budgte kilimanjaro \"road map\"")
            .corrected(&v)
            .unwrap();
        assert_eq!(
            query.clauses(),
            [
                whole(&["budget"]),
                word("kilimanjaro"),
                Clause::Phrase(vec!["road".into(), "map".into()])
            ]
        );
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].typed, "budgte");
    }

    #[test]
    fn a_word_with_no_term_near_it_stays() {
        let v = vocabulary(&["roadmap", "budget"]);
        assert_eq!(parsed("qqqqqqq").corrected(&v), None);
        assert_eq!(parsed("qqqqqqq").corrected(&vocabulary(&[])), None);
    }

    #[test]
    fn two_edits_from_nine_letters() {
        let v = vocabulary(&["antinational"]);
        let (query, _) = parsed("antinatoinal").corrected(&v).unwrap();
        assert_eq!(query.clauses(), [whole(&["antinational"])]);
    }
}
