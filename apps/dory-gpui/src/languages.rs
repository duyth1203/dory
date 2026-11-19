//! The languages Dory knows by name, and the one rule that turns the meeting languages into the
//! Language choices (`docs/design/settings-icons-blur.md` section 4.1). Settings > Transcription >
//! Language, Re-transcribe in…, Translate…, and the language combobox all read this module, so
//! they never disagree. Everything here is pure.

use dory_core::domain::{LangTag, LanguagePref, Settings};

pub use dory_core::domain::languages::{LANGUAGES, Language, find, name};

/// "French" for both `fr` and `fr-FR`: the language without its region, as the detail pane
/// names a detected language.
pub fn primary_name(tag: &LangTag) -> String {
    find(tag.primary()).map_or_else(|| name(tag), |lang| lang.english.to_owned())
}

/// "Tagalog", "Tagalog and English", "Tagalog, English, and French", with `and` or `or`.
pub fn name_list(names: &[String], conjunction: &str) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} {conjunction} {b}"),
        [rest @ .., last] => format!("{}, {conjunction} {last}", rest.join(", ")),
    }
}

/// What the Language popup offers, what it is in effect, and the line that says why it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    /// Automatic, then each meeting language in the list's order.
    pub options: Vec<LanguagePref>,
    pub effective: LanguagePref,
    /// Set when the stored language is no longer a meeting language.
    pub note: Option<String>,
}

/// The one rule (section 4.1): the Language options are Automatic plus the meeting languages,
/// and a stored language outside them falls back to Automatic and says so.
pub fn choices(meeting: &[LangTag], stored: &LanguagePref) -> Choices {
    let mut options = vec![LanguagePref::Auto];
    for tag in meeting {
        let option = LanguagePref::Pinned(tag.clone());
        if !options.contains(&option) {
            options.push(option);
        }
    }
    let (effective, note) = if options.contains(stored) {
        (stored.clone(), None)
    } else {
        let removed = match stored {
            LanguagePref::Pinned(tag) => name(tag),
            LanguagePref::Auto => unreachable!("Automatic is always an option"),
        };
        (
            LanguagePref::Auto,
            Some(format!(
                "{removed} was removed from meeting languages, so Language is back to Automatic."
            )),
        )
    };
    Choices {
        options,
        effective,
        note,
    }
}

/// "Automatic" or the language's name, as a popup or menu item reads.
pub fn label(choice: &LanguagePref) -> String {
    match choice {
        LanguagePref::Auto => "Automatic".into(),
        LanguagePref::Pinned(tag) => name(tag),
    }
}

/// The line under Language: what the choice does with these meeting languages.
pub fn description(meeting: &[LangTag], choice: &LanguagePref) -> String {
    match choice {
        LanguagePref::Pinned(tag) => {
            format!(
                "Transcribes every meeting as {}, with no detection.",
                name(tag)
            )
        }
        LanguagePref::Auto => {
            let names: Vec<String> = meeting.iter().map(name).collect();
            match names.as_slice() {
                [] => "Automatic detects the language and follows switches mid-meeting.".into(),
                [one] => format!("Uses {one}."),
                _ => format!(
                    "Automatic detects which of {} is spoken, and follows switches mid-meeting.",
                    name_list(&names, "or")
                ),
            }
        }
    }
}

/// Settings > Transcription > the output language: the popup's first item, and what the setting
/// holds when there is no target.
pub const SAME_AS_SPOKEN: &str = "Same as spoken";

/// "Same as spoken" or the language's name, as the popup and the menu read.
pub fn output_label(choice: Option<&LangTag>) -> String {
    choice.map_or_else(|| SAME_AS_SPOKEN.to_owned(), name)
}

/// What the output-language popup offers after "Same as spoken": every language Dory names, by
/// its English name.
pub fn output_choices() -> Vec<LangTag> {
    let mut all: Vec<&Language> = LANGUAGES.iter().collect();
    all.sort_by_key(|lang| fold(lang.english));
    all.into_iter()
        .filter_map(|lang| LangTag::parse(lang.code).ok())
        .collect()
}

/// The line under the output-language row: what the choice does, and what it leaves alone.
pub fn output_description(choice: Option<&LangTag>) -> String {
    match choice {
        None => "Each transcript and summary stays in the language it was spoken in.".into(),
        Some(tag) => format!(
            "Dory translates new recordings into {} and writes their summaries in {}, using \
             your summary model on this Mac. The spoken transcript is kept, and recordings \
             you already have do not change.",
            name(tag),
            name(tag)
        ),
    }
}

/// What **Translate…** offers for a summary written in `written`: the meeting languages, a
/// separator (`None`), then every other language Dory names by its English name. The summary's
/// own language is never offered, whichever list it is in, and no language is offered twice.
pub fn translate_choices(meeting: &[LangTag], written: Option<&LangTag>) -> Vec<Option<LangTag>> {
    let skip = |tag: &LangTag| written.is_some_and(|w| w.primary() == tag.primary());
    let mut first: Vec<LangTag> = Vec::new();
    for tag in meeting {
        if !skip(tag) && !first.iter().any(|t| t.primary() == tag.primary()) {
            first.push(tag.clone());
        }
    }
    let rest: Vec<LangTag> = output_choices()
        .into_iter()
        .filter(|tag| !skip(tag) && !first.iter().any(|t| t.primary() == tag.primary()))
        .collect();
    let separator = (!first.is_empty() && !rest.is_empty()).then_some(None);
    first
        .into_iter()
        .map(Some)
        .chain(separator)
        .chain(rest.into_iter().map(Some))
        .collect()
}

pub const KEEP_ONE: &str = "Keep at least one meeting language.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Added,
    Removed {
        note: Option<String>,
    },
    /// The list keeps at least one language, so the last one stays.
    KeptLast,
    /// The last language was only guessed from the Mac's, and picking it chose it.
    Confirmed,
}

/// Adds `tag` at the end of the meeting languages, or removes it. Removing the language that
/// Language is set to puts Language back on Automatic and returns the note that says so. Any edit
/// makes the list the user's choice, so picking the one guessed language confirms it.
pub fn toggle(settings: &mut Settings, tag: &LangTag) -> Edit {
    let guessed = std::mem::take(&mut settings.meeting_languages_guessed);
    let Some(ix) = settings.meeting_languages.iter().position(|t| t == tag) else {
        settings.meeting_languages.push(tag.clone());
        return Edit::Added;
    };
    if settings.meeting_languages.len() <= 1 {
        return if guessed {
            Edit::Confirmed
        } else {
            Edit::KeptLast
        };
    }
    settings.meeting_languages.remove(ix);
    let after = choices(&settings.meeting_languages, &settings.language);
    settings.language = after.effective;
    Edit::Removed { note: after.note }
}

/// Lower case with the accents taken off, so "francais" finds "Français".
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => out.push('a'),
            'ç' | 'ć' | 'č' => out.push('c'),
            'ď' => out.push('d'),
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' | 'ə' => out.push('e'),
            'ğ' => out.push('g'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => out.push('i'),
            'ł' => out.push('l'),
            'ñ' | 'ń' | 'ň' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => out.push('o'),
            'ř' => out.push('r'),
            'ś' | 'š' | 'ş' | 'ș' => out.push('s'),
            'ß' => out.push_str("ss"),
            'ť' | 'ț' => out.push('t'),
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'ź' | 'ż' | 'ž' => out.push('z'),
            'ʻ' | '\u{301}' | '\u{300}' | '\u{308}' => {}
            c => out.push(c),
        }
    }
    out
}

/// One row of a language combobox: the tag a pick asks for, by the names a search matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub tag: LangTag,
    /// "French", or "French (CA)" for `fr-CA`.
    pub english: String,
    /// The language's own name, when Dory knows the language.
    pub native: Option<&'static str>,
}

impl Row {
    pub fn of(tag: LangTag) -> Row {
        Row {
            english: name(&tag),
            native: find(tag.as_str())
                .or_else(|| find(tag.primary()))
                .map(|lang| lang.native),
            tag,
        }
    }

    /// The own name, when it says something the English name does not.
    pub fn native_hint(&self) -> Option<&'static str> {
        self.native.filter(|native| *native != self.english)
    }
}

/// What a language combobox lists before a search, in order. The first `lead` rows, the meeting
/// languages on **Translate…**, sit above a divider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub rows: Vec<Row>,
    pub lead: usize,
}

impl Offer {
    /// Every language Dory names, alphabetical by English name: the meeting-languages combobox.
    pub fn every() -> Offer {
        Offer {
            rows: output_choices().into_iter().map(Row::of).collect(),
            lead: 0,
        }
    }

    /// [`translate_choices`], with its separator as the divider.
    pub fn translate(meeting: &[LangTag], written: Option<&LangTag>) -> Offer {
        let choices = translate_choices(meeting, written);
        let lead = choices.iter().position(Option::is_none).unwrap_or(0);
        Offer {
            rows: choices.into_iter().flatten().map(Row::of).collect(),
            lead,
        }
    }

    /// Matches the English name, the language's own name, and the tag, ignoring case and
    /// accents, and keeps the offer's order so the list never jumps while the user picks.
    pub fn search(&self, query: &str) -> Matches {
        let query = fold(query.trim());
        let ranked: Vec<(usize, u8)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(ix, row)| {
                if query.is_empty() {
                    Some((ix, 1))
                } else {
                    rank(&query, row).map(|r| (ix, r))
                }
            })
            .collect();
        let best = ranked
            .iter()
            .enumerate()
            .min_by_key(|(at, (_, r))| (*r, *at))
            .map(|(at, _)| at);
        Matches {
            lead: ranked.iter().filter(|(ix, _)| *ix < self.lead).count(),
            rows: ranked
                .into_iter()
                .map(|(ix, _)| self.rows[ix].clone())
                .collect(),
            best,
        }
    }
}

/// How well `query` names a row, best first; `None` when it does not.
fn rank(query: &str, row: &Row) -> Option<u8> {
    let code = fold(row.tag.as_str());
    let english = fold(&row.english);
    let native = fold(row.native.unwrap_or_default());
    let starts_a_word = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| word.starts_with(query))
    };
    if code == query {
        Some(0)
    } else if english.starts_with(query) {
        Some(1)
    } else if !native.is_empty() && native.starts_with(query) {
        Some(2)
    } else if starts_a_word(&english) || starts_a_word(&native) {
        Some(3)
    } else if code.starts_with(query) {
        Some(4)
    } else {
        None
    }
}

/// The combobox's list for a search: every match in the offer's order, how many of them come
/// from its lead, and the best match to highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matches {
    pub rows: Vec<Row>,
    pub lead: usize,
    pub best: Option<usize>,
}

/// Searches [`Offer::every`]. "ge" finds German, Georgian, Swiss German, and Low German; "de"
/// finds German.
pub fn search(query: &str) -> Matches {
    Offer::every().search(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn tags(list: &[&str]) -> Vec<LangTag> {
        list.iter().map(|t| tag(t)).collect()
    }

    fn pinned(s: &str) -> LanguagePref {
        LanguagePref::Pinned(tag(s))
    }

    #[test]
    fn the_options_are_automatic_then_the_meeting_languages_in_order() {
        let got = choices(&tags(&["en", "de", "fr"]), &LanguagePref::Auto);
        assert_eq!(
            got.options,
            vec![LanguagePref::Auto, pinned("en"), pinned("de"), pinned("fr")]
        );
        assert_eq!(got.effective, LanguagePref::Auto);
        assert_eq!(got.note, None);
    }

    #[test]
    fn a_stored_meeting_language_stays_in_effect() {
        let got = choices(&tags(&["en", "de"]), &pinned("de"));
        assert_eq!(got.effective, pinned("de"));
        assert_eq!(got.note, None);
    }

    #[test]
    fn a_stored_language_outside_the_list_falls_back_to_automatic_and_says_so() {
        let got = choices(&tags(&["en", "fr"]), &pinned("de"));
        assert_eq!(got.effective, LanguagePref::Auto);
        assert_eq!(
            got.note.as_deref(),
            Some("German was removed from meeting languages, so Language is back to Automatic.")
        );
        assert!(
            !got.options.contains(&pinned("de")),
            "nothing else is offered"
        );
    }

    #[test]
    fn automatic_names_the_languages_it_detects_between() {
        let three = tags(&["en", "de", "fr"]);
        assert_eq!(
            description(&three, &LanguagePref::Auto),
            "Automatic detects which of English, German, or French is spoken, and follows switches mid-meeting."
        );
        assert_eq!(
            description(&tags(&["en", "de"]), &LanguagePref::Auto),
            "Automatic detects which of English or German is spoken, and follows switches mid-meeting."
        );
        assert_eq!(
            description(&tags(&["en"]), &LanguagePref::Auto),
            "Uses English."
        );
        assert_eq!(
            description(&three, &pinned("fr")),
            "Transcribes every meeting as French, with no detection."
        );
    }

    #[test]
    fn removing_the_selected_language_resets_language_with_the_note() {
        let mut settings = Settings {
            meeting_languages: tags(&["en", "de"]),
            language: pinned("de"),
            ..Settings::default()
        };
        let edit = toggle(&mut settings, &tag("de"));
        assert_eq!(
            edit,
            Edit::Removed {
                note: Some(
                    "German was removed from meeting languages, so Language is back to Automatic."
                        .into()
                )
            }
        );
        assert_eq!(settings.meeting_languages, tags(&["en"]));
        assert_eq!(settings.language, LanguagePref::Auto);
    }

    #[test]
    fn removing_another_language_keeps_the_choice_and_says_nothing() {
        let mut settings = Settings {
            meeting_languages: tags(&["en", "de", "fr"]),
            language: pinned("de"),
            ..Settings::default()
        };
        assert_eq!(
            toggle(&mut settings, &tag("fr")),
            Edit::Removed { note: None }
        );
        assert_eq!(settings.language, pinned("de"));
    }

    #[test]
    fn the_last_language_stays() {
        let mut settings = Settings {
            meeting_languages: tags(&["en"]),
            ..Settings::default()
        };
        assert_eq!(toggle(&mut settings, &tag("en")), Edit::KeptLast);
        assert_eq!(settings.meeting_languages, tags(&["en"]));
    }

    #[test]
    fn adding_appends_in_the_order_added() {
        let mut settings = Settings {
            meeting_languages: tags(&["fr"]),
            ..Settings::default()
        };
        assert_eq!(toggle(&mut settings, &tag("de")), Edit::Added);
        assert_eq!(toggle(&mut settings, &tag("af")), Edit::Added);
        assert_eq!(settings.meeting_languages, tags(&["fr", "de", "af"]));
    }

    fn found(query: &str) -> Vec<String> {
        search(query).rows.into_iter().map(|r| r.english).collect()
    }

    #[test]
    fn ge_finds_the_german_languages_and_georgian() {
        assert_eq!(
            found("ge"),
            ["Georgian", "German", "Low German", "Swiss German"]
        );
        assert_eq!(found("GE"), found("ge"), "case does not matter");
    }

    #[test]
    fn de_finds_german_by_its_code() {
        let matches = search("de");
        assert_eq!(found("de"), ["German"]);
        assert_eq!(matches.best, Some(0));
    }

    #[test]
    fn accents_and_native_names_match() {
        assert_eq!(found("francais"), ["French"]);
        assert_eq!(found("espanol"), ["Spanish"]);
        assert_eq!(found("deutsch"), ["German"]);
        assert!(found("xyz").is_empty());
        assert_eq!(search("xyz").best, None);
    }

    #[test]
    fn the_best_match_is_highlighted_in_an_alphabetical_list() {
        let matches = search("en");
        let names: Vec<&str> = matches.rows.iter().map(|r| r.english.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "the list stays alphabetical");
        assert_eq!(matches.rows[matches.best.unwrap()].english, "English");
    }

    #[test]
    fn an_empty_search_lists_every_language() {
        let matches = search("");
        assert_eq!(matches.rows.len(), LANGUAGES.len());
        assert_eq!(matches.best, Some(0));
    }

    #[test]
    fn names_read_as_plain_english() {
        assert_eq!(name(&tag("tl")), "Tagalog");
        assert_eq!(name(&tag("en-US")), "English (US)");
        assert_eq!(name(&tag("xx")), "xx");
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(name_list(&names(&["Tagalog"]), "and"), "Tagalog");
        assert_eq!(
            name_list(&names(&["Tagalog", "English"]), "and"),
            "Tagalog and English"
        );
        assert_eq!(
            name_list(&names(&["Tagalog", "English", "French"]), "and"),
            "Tagalog, English, and French"
        );
    }

    #[test]
    fn the_output_choices_are_every_language_by_english_name() {
        let choices = output_choices();
        assert_eq!(choices.len(), LANGUAGES.len());
        let names: Vec<String> = choices.iter().map(name).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| fold(n));
        assert_eq!(names, sorted);
        assert!(choices.contains(&tag("en")));
        assert_eq!(output_label(None), "Same as spoken");
        assert_eq!(output_label(Some(&tag("tl"))), "Tagalog");
    }

    #[test]
    fn translate_offers_the_meeting_languages_then_every_other_language_but_the_summarys_own() {
        let names = |choices: Vec<Option<LangTag>>| -> Vec<Option<String>> {
            choices.iter().map(|c| c.as_ref().map(name)).collect()
        };
        let got = names(translate_choices(&[tag("tl"), tag("en")], Some(&tag("tl"))));
        assert_eq!(got.first(), Some(&Some("English".to_owned())));
        assert_eq!(
            got.get(1),
            Some(&None),
            "a separator follows the meeting languages"
        );
        assert!(
            !got.contains(&Some("Tagalog".to_owned())),
            "not the summary's own language"
        );
        let others: Vec<&String> = got[2..].iter().flatten().collect();
        assert_eq!(others.len(), LANGUAGES.len() - 2);
        assert!(
            !others.contains(&&"English".to_owned()),
            "no language twice"
        );
        let mut sorted: Vec<String> = others.iter().map(|n| (*n).clone()).collect();
        sorted.sort_by_key(|n| fold(n));
        assert_eq!(
            others.iter().map(|n| (*n).clone()).collect::<Vec<_>>(),
            sorted
        );

        let all = translate_choices(&[tag("tl"), tag("en")], None);
        assert_eq!(all[..2], [Some(tag("tl")), Some(tag("en"))]);
        assert_eq!(all[2], None);
        assert_eq!(all.iter().flatten().count(), LANGUAGES.len());
    }

    #[test]
    fn the_translate_offer_searches_the_meeting_languages_first_and_keeps_regions() {
        let offer = Offer::translate(&[tag("fr-CA"), tag("tl"), tag("en")], Some(&tag("tl")));
        assert_eq!(
            offer.lead, 2,
            "French (CA) and English sit above the divider"
        );
        assert_eq!(offer.rows[0].tag, tag("fr-CA"));
        assert_eq!(offer.rows[0].english, "French (CA)");
        assert_eq!(offer.rows[0].native, Some("Français"));

        let all = offer.search("");
        assert_eq!(
            (all.rows.len(), all.lead, all.best),
            (offer.rows.len(), 2, Some(0))
        );

        let en = offer.search("en");
        assert_eq!(
            en.rows[0].english, "English",
            "the meeting language stays first"
        );
        assert_eq!(en.lead, 1, "French (CA) does not match");
        assert_eq!(en.rows[en.best.unwrap()].english, "English");
        let rest: Vec<&str> = en.rows[1..].iter().map(|r| r.english.as_str()).collect();
        let mut sorted = rest.clone();
        sorted.sort_by_key(|n| fold(n));
        assert_eq!(rest, sorted, "the rest stays alphabetical");

        let fr = offer.search("francais");
        assert_eq!(
            fr.rows.len(),
            1,
            "French is offered once, as the meeting language"
        );
        assert_eq!((fr.rows[0].tag.as_str(), fr.lead), ("fr-CA", 1));
        assert!(
            offer.search("tagalog").rows.is_empty(),
            "not the summary's own language"
        );
        assert_eq!(Offer::every().lead, 0);
    }

    #[test]
    fn translate_has_no_separator_without_meeting_languages_or_without_others() {
        let none = translate_choices(&[], Some(&tag("en-US")));
        assert!(none.iter().all(Option::is_some));
        assert_eq!(none.len(), LANGUAGES.len() - 1, "en-US is English");
        let only = translate_choices(&[tag("tl")], Some(&tag("tl")));
        assert!(
            only.iter().all(Option::is_some),
            "the meeting list gave nothing to separate"
        );
        assert_eq!(
            translate_choices(&[tag("fr-CA"), tag("fr")], None)[0],
            Some(tag("fr-CA"))
        );
        assert_eq!(
            translate_choices(&[tag("fr-CA"), tag("fr")], None)
                .iter()
                .flatten()
                .filter(|t| t.primary() == "fr")
                .count(),
            1
        );
    }

    #[test]
    fn the_description_says_what_stays_and_what_changes() {
        assert_eq!(
            output_description(None),
            "Each transcript and summary stays in the language it was spoken in."
        );
        let english = output_description(Some(&tag("en")));
        for want in [
            "into English",
            "summaries in English",
            "spoken transcript is kept",
        ] {
            assert!(english.contains(want), "{english}");
        }
    }

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        assert!(LANGUAGES.windows(2).all(|w| w[0].code < w[1].code));
        for lang in LANGUAGES {
            assert!(LangTag::parse(lang.code).is_ok(), "{}", lang.code);
        }
    }

    #[test]
    fn any_pick_chooses_a_guessed_list_and_picking_its_one_language_confirms_it() {
        let guessed = Settings {
            meeting_languages: tags(&["en"]),
            meeting_languages_guessed: true,
            ..Settings::default()
        };
        let mut settings = guessed.clone();
        assert_eq!(toggle(&mut settings, &tag("en")), Edit::Confirmed);
        assert_eq!(settings.meeting_languages, tags(&["en"]));
        assert!(!settings.meeting_languages_guessed);
        assert_eq!(
            toggle(&mut settings, &tag("en")),
            Edit::KeptLast,
            "once chosen, the last language just stays"
        );
        let mut settings = guessed;
        assert_eq!(toggle(&mut settings, &tag("tl")), Edit::Added);
        assert_eq!(settings.meeting_languages, tags(&["en", "tl"]));
        assert!(!settings.meeting_languages_guessed);
    }
}
