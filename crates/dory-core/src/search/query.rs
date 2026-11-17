//! What the user typed, as clauses a store can look up and a view can highlight.

use super::tokens::tokens;

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
}
