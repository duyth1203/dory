use std::collections::HashSet;

use super::{InvalidInput, Settings};
use crate::search::tokens;

/// The most terms a user can keep.
pub const MAX_TERMS: usize = 200;
/// The longest term, in characters.
pub const MAX_TERM_CHARS: usize = 80;
/// The most speaker names `speech_vocabulary` adds.
pub const MAX_SPEAKER_NAMES: usize = 100;

/// How hard the speech engine leans toward a term. The engine owns what each level means in its
/// own units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Boost {
    #[default]
    Normal,
    /// For a term the engine keeps missing on `Normal`. Too much of it can pull a different word
    /// into the term, so it is a choice per term.
    Strong,
}

/// A name or term the speech engine should get right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VocabularyTerm {
    pub text: String,
    pub boost: Boost,
}

impl VocabularyTerm {
    pub fn new(text: impl Into<String>, boost: Boost) -> Self {
        Self {
            text: text.into(),
            boost,
        }
    }
}

/// The words of `text` folded for comparison, so "Ana  María" and "ana maria" are one term.
pub fn folded_words(text: &str) -> Vec<String> {
    tokens(text).map(|t| t.folded).collect()
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

/// `/` and `:` are separators in the engines' hotword format, so a term holding one would split
/// in two.
fn has_separator(text: &str) -> bool {
    text.contains(['/', ':'])
}

pub(super) fn validate_terms(terms: &[VocabularyTerm]) -> Result<(), InvalidInput> {
    let bad = |detail: String| InvalidInput::new("vocabulary term", detail);
    if terms.len() > MAX_TERMS {
        return Err(bad(format!(
            "at most {MAX_TERMS} terms, got {}",
            terms.len()
        )));
    }
    let mut seen = HashSet::new();
    for term in terms {
        let text = term.text.as_str();
        if text.trim().is_empty() {
            return Err(bad("a term is empty".into()));
        }
        if text != text.trim() {
            return Err(bad(format!("{text:?} has spaces at its ends")));
        }
        if text.chars().count() > MAX_TERM_CHARS {
            return Err(bad(format!(
                "{text:?} is longer than {MAX_TERM_CHARS} characters"
            )));
        }
        if has_control(text) {
            return Err(bad(format!(
                "{text:?} has a line break, tab, or other control character"
            )));
        }
        if has_separator(text) {
            return Err(bad(format!(
                "{text:?} has a / or :, which the speech engine reads as separators"
            )));
        }
        let words = folded_words(text);
        if words.is_empty() {
            return Err(bad(format!("{text:?} has no letter or digit")));
        }
        if !seen.insert(words) {
            return Err(bad(format!("{text:?} is listed twice")));
        }
    }
    Ok(())
}

pub(super) fn validate_hidden_names(names: &[String]) -> Result<(), InvalidInput> {
    let bad = |detail: String| InvalidInput::new("hidden speaker name", detail);
    for name in names {
        if name.trim().is_empty() {
            return Err(bad("a name is empty".into()));
        }
        if name != name.trim() {
            return Err(bad(format!("{name:?} has spaces at its ends")));
        }
        if has_control(name) {
            return Err(bad(format!(
                "{name:?} has a line break, tab, or other control character"
            )));
        }
    }
    Ok(())
}

/// What the speech engine is asked to favor: the user's terms in order, then the names the user
/// gave speakers, `speaker_names` newest first. A name drops out when it is hidden, when it reads
/// the same as a term or an earlier name once folded, or when it could not be a term (a
/// separator, a control character, nothing to say, too long), because renames are free text.
/// At most [`MAX_SPEAKER_NAMES`] names are added, and names are never `Strong`.
pub fn speech_vocabulary(settings: &Settings, speaker_names: &[String]) -> Vec<VocabularyTerm> {
    let mut out = settings.vocabulary.clone();
    if !settings.vocabulary_from_speakers {
        return out;
    }
    let hidden: HashSet<Vec<String>> = settings
        .vocabulary_hidden_names
        .iter()
        .map(|n| folded_words(n))
        .collect();
    let mut seen: HashSet<Vec<String>> = out.iter().map(|t| folded_words(&t.text)).collect();
    let mut added = 0;
    for name in speaker_names {
        if added == MAX_SPEAKER_NAMES {
            break;
        }
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_TERM_CHARS
            || has_control(name)
            || has_separator(name)
        {
            continue;
        }
        let words = folded_words(name);
        if words.is_empty() || hidden.contains(&words) || !seen.insert(words) {
            continue;
        }
        out.push(VocabularyTerm::new(name, Boost::Normal));
        added += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(terms: &[(&str, Boost)]) -> Settings {
        Settings {
            vocabulary: terms
                .iter()
                .map(|(t, b)| VocabularyTerm::new(*t, *b))
                .collect(),
            ..Settings::default()
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn texts(terms: &[VocabularyTerm]) -> Vec<&str> {
        terms.iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn terms_come_first_in_order_then_speaker_names_newest_first() {
        let s = settings(&[("Kubernetes", Boost::Normal), ("Priya", Boost::Strong)]);
        let got = speech_vocabulary(&s, &names(&["Lina", "Omar"]));
        assert_eq!(texts(&got), ["Kubernetes", "Priya", "Lina", "Omar"]);
        assert_eq!(got[1].boost, Boost::Strong);
        assert_eq!(got[2].boost, Boost::Normal);
    }

    #[test]
    fn a_name_already_a_term_or_an_earlier_name_is_dropped_by_its_folded_words() {
        let s = settings(&[("Priya", Boost::Strong)]);
        let got = speech_vocabulary(&s, &names(&["priya", "Lina", "LINA", "Zoë  Ng", "zoe ng"]));
        assert_eq!(texts(&got), ["Priya", "Lina", "Zoë  Ng"]);
        assert_eq!(got[0].boost, Boost::Strong, "the user's term wins");
    }

    #[test]
    fn hidden_names_are_left_out_however_they_are_cased() {
        let s = Settings {
            vocabulary_hidden_names: names(&["lina"]),
            ..Settings::default()
        };
        let got = speech_vocabulary(&s, &names(&["Lina", "Omar"]));
        assert_eq!(texts(&got), ["Omar"]);
    }

    #[test]
    fn a_hidden_name_does_not_hide_a_term_the_user_typed() {
        let s = Settings {
            vocabulary: vec![VocabularyTerm::new("Lina", Boost::Normal)],
            vocabulary_hidden_names: names(&["Lina"]),
            ..Settings::default()
        };
        assert_eq!(texts(&speech_vocabulary(&s, &[])), ["Lina"]);
    }

    #[test]
    fn turning_speaker_names_off_leaves_only_the_terms() {
        let s = Settings {
            vocabulary_from_speakers: false,
            ..settings(&[("Priya", Boost::Normal)])
        };
        assert_eq!(texts(&speech_vocabulary(&s, &names(&["Lina"]))), ["Priya"]);
    }

    #[test]
    fn speaker_names_are_capped_and_the_cap_counts_only_names_added() {
        let many: Vec<String> = (0..150).map(|i| format!("Person {i}")).collect();
        let mut with_skips = names(&["", "a/b", "x:y", "---"]);
        with_skips.extend(many.iter().cloned());
        let got = speech_vocabulary(&Settings::default(), &with_skips);
        assert_eq!(got.len(), MAX_SPEAKER_NAMES);
        assert_eq!(got[0].text, "Person 0");
        assert_eq!(got[MAX_SPEAKER_NAMES - 1].text, "Person 99");
    }

    #[test]
    fn free_text_renames_that_cannot_be_terms_are_skipped() {
        let long = "a".repeat(MAX_TERM_CHARS + 1);
        let got = speech_vocabulary(
            &Settings::default(),
            &names(&[
                "AC/DC",
                "Re: plan",
                "two\nlines",
                "tab\there",
                &long,
                "  Lina  ",
            ]),
        );
        assert_eq!(texts(&got), ["Lina"]);
    }

    #[test]
    fn the_cap_does_not_trim_the_users_own_terms() {
        let terms: Vec<VocabularyTerm> = (0..MAX_TERMS)
            .map(|i| VocabularyTerm::new(format!("Term {i}"), Boost::Normal))
            .collect();
        let s = Settings {
            vocabulary: terms,
            ..Settings::default()
        };
        assert_eq!(
            speech_vocabulary(&s, &names(&["Lina"])).len(),
            MAX_TERMS + 1
        );
    }

    fn invalid(terms: &[&str]) -> bool {
        validate_terms(
            &terms
                .iter()
                .map(|t| VocabularyTerm::new(*t, Boost::Normal))
                .collect::<Vec<_>>(),
        )
        .is_err()
    }

    #[test]
    fn a_term_must_be_one_clean_phrase() {
        assert!(!invalid(&["Priya", "Acme Corp", "Zoë"]));
        assert!(invalid(&[""]));
        assert!(invalid(&["   "]));
        assert!(invalid(&[" Priya"]));
        assert!(invalid(&["Priya "]));
        assert!(invalid(&["two\nlines"]));
        assert!(invalid(&["a\tb"]));
        assert!(invalid(&["AC/DC"]));
        assert!(invalid(&["Re: plan"]));
        assert!(invalid(&["---"]));
        assert!(invalid(&[&"a".repeat(MAX_TERM_CHARS + 1)]));
        assert!(!invalid(&[&"a".repeat(MAX_TERM_CHARS)]));
    }

    #[test]
    fn two_terms_that_fold_to_the_same_words_are_a_duplicate() {
        assert!(invalid(&["Priya", "priya"]));
        assert!(invalid(&["Zoë Ng", "zoe  ng"]));
        assert!(!invalid(&["Zoe", "Zoe Ng"]));
    }

    #[test]
    fn at_most_200_terms() {
        let ok: Vec<VocabularyTerm> = (0..MAX_TERMS)
            .map(|i| VocabularyTerm::new(format!("t{i}"), Boost::Normal))
            .collect();
        assert_eq!(validate_terms(&ok), Ok(()));
        let mut over = ok;
        over.push(VocabularyTerm::new("extra", Boost::Normal));
        assert!(validate_terms(&over).is_err());
    }

    #[test]
    fn hidden_names_are_trimmed_single_line_text() {
        assert_eq!(validate_hidden_names(&names(&["Lina", "AC/DC"])), Ok(()));
        for bad in ["", " ", " Lina", "a\nb", "a\tb"] {
            assert!(validate_hidden_names(&names(&[bad])).is_err(), "{bad:?}");
        }
    }
}
