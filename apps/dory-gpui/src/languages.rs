//! The languages Dory knows by name, and the one rule that turns the meeting languages into the
//! Language choices (`docs/design/settings-icons-blur.md` section 4.1). Settings > Transcription >
//! Language, Re-transcribe in…, and the meeting-languages combobox all read this module, so they
//! never disagree. Everything here is pure.

use dory_core::domain::{LangTag, LanguagePref, Settings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Language {
    /// A BCP-47 tag.
    pub code: &'static str,
    pub english: &'static str,
    /// The language's own name for itself.
    pub native: &'static str,
}

const fn l(code: &'static str, english: &'static str, native: &'static str) -> Language {
    Language {
        code,
        english,
        native,
    }
}

/// Every language a catalog row lists today, plus the German variants search must find. Sorted
/// by code.
pub const LANGUAGES: &[Language] = &[
    l("af", "Afrikaans", "Afrikaans"),
    l("am", "Amharic", "አማርኛ"),
    l("ar", "Arabic", "العربية"),
    l("as", "Assamese", "অসমীয়া"),
    l("az", "Azerbaijani", "Azərbaycan"),
    l("ba", "Bashkir", "Башҡорт"),
    l("be", "Belarusian", "Беларуская"),
    l("bg", "Bulgarian", "Български"),
    l("bn", "Bengali", "বাংলা"),
    l("bo", "Tibetan", "བོད་སྐད"),
    l("br", "Breton", "Brezhoneg"),
    l("bs", "Bosnian", "Bosanski"),
    l("ca", "Catalan", "Català"),
    l("cs", "Czech", "Čeština"),
    l("cy", "Welsh", "Cymraeg"),
    l("da", "Danish", "Dansk"),
    l("de", "German", "Deutsch"),
    l("el", "Greek", "Ελληνικά"),
    l("en", "English", "English"),
    l("es", "Spanish", "Español"),
    l("et", "Estonian", "Eesti"),
    l("eu", "Basque", "Euskara"),
    l("fa", "Persian", "فارسی"),
    l("fi", "Finnish", "Suomi"),
    l("fil", "Filipino", "Filipino"),
    l("fo", "Faroese", "Føroyskt"),
    l("fr", "French", "Français"),
    l("gl", "Galician", "Galego"),
    l("gsw", "Swiss German", "Schwiizertüütsch"),
    l("gu", "Gujarati", "ગુજરાતી"),
    l("ha", "Hausa", "Hausa"),
    l("haw", "Hawaiian", "ʻŌlelo Hawaiʻi"),
    l("he", "Hebrew", "עברית"),
    l("hi", "Hindi", "हिन्दी"),
    l("hr", "Croatian", "Hrvatski"),
    l("ht", "Haitian Creole", "Kreyòl ayisyen"),
    l("hu", "Hungarian", "Magyar"),
    l("hy", "Armenian", "Հայերեն"),
    l("id", "Indonesian", "Bahasa Indonesia"),
    l("is", "Icelandic", "Íslenska"),
    l("it", "Italian", "Italiano"),
    l("ja", "Japanese", "日本語"),
    l("jv", "Javanese", "Basa Jawa"),
    l("ka", "Georgian", "ქართული"),
    l("kk", "Kazakh", "Қазақ"),
    l("km", "Khmer", "ខ្មែរ"),
    l("kn", "Kannada", "ಕನ್ನಡ"),
    l("ko", "Korean", "한국어"),
    l("la", "Latin", "Latina"),
    l("lb", "Luxembourgish", "Lëtzebuergesch"),
    l("ln", "Lingala", "Lingála"),
    l("lo", "Lao", "ລາວ"),
    l("lt", "Lithuanian", "Lietuvių"),
    l("lv", "Latvian", "Latviešu"),
    l("mg", "Malagasy", "Malagasy"),
    l("mi", "Māori", "Te Reo Māori"),
    l("mk", "Macedonian", "Македонски"),
    l("ml", "Malayalam", "മലയാളം"),
    l("mn", "Mongolian", "Монгол"),
    l("mr", "Marathi", "मराठी"),
    l("ms", "Malay", "Bahasa Melayu"),
    l("mt", "Maltese", "Malti"),
    l("my", "Burmese", "မြန်မာ"),
    l("nb", "Norwegian Bokmål", "Norsk bokmål"),
    l("nds", "Low German", "Plattdüütsch"),
    l("ne", "Nepali", "नेपाली"),
    l("nl", "Dutch", "Nederlands"),
    l("nn", "Norwegian Nynorsk", "Norsk nynorsk"),
    l("no", "Norwegian", "Norsk"),
    l("oc", "Occitan", "Occitan"),
    l("pa", "Punjabi", "ਪੰਜਾਬੀ"),
    l("pl", "Polish", "Polski"),
    l("ps", "Pashto", "پښتو"),
    l("pt", "Portuguese", "Português"),
    l("ro", "Romanian", "Română"),
    l("ru", "Russian", "Русский"),
    l("sa", "Sanskrit", "संस्कृतम्"),
    l("sd", "Sindhi", "سنڌي"),
    l("si", "Sinhala", "සිංහල"),
    l("sk", "Slovak", "Slovenčina"),
    l("sl", "Slovenian", "Slovenščina"),
    l("sn", "Shona", "chiShona"),
    l("so", "Somali", "Soomaali"),
    l("sq", "Albanian", "Shqip"),
    l("sr", "Serbian", "Српски"),
    l("su", "Sundanese", "Basa Sunda"),
    l("sv", "Swedish", "Svenska"),
    l("sw", "Swahili", "Kiswahili"),
    l("ta", "Tamil", "தமிழ்"),
    l("te", "Telugu", "తెలుగు"),
    l("tg", "Tajik", "Тоҷикӣ"),
    l("th", "Thai", "ไทย"),
    l("tk", "Turkmen", "Türkmen"),
    l("tl", "Tagalog", "Tagalog"),
    l("tr", "Turkish", "Türkçe"),
    l("tt", "Tatar", "Татар"),
    l("uk", "Ukrainian", "Українська"),
    l("ur", "Urdu", "اردو"),
    l("uz", "Uzbek", "Oʻzbek"),
    l("vi", "Vietnamese", "Tiếng Việt"),
    l("yi", "Yiddish", "ייִדיש"),
    l("yo", "Yoruba", "Yorùbá"),
    l("yue", "Cantonese", "粵語"),
    l("zh", "Chinese", "中文"),
];

pub fn find(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|lang| lang.code == code)
}

/// "Tagalog" for `tl`, "English (US)" for `en-US`, and the tag itself when the table lacks it.
pub fn name(tag: &LangTag) -> String {
    match (find(tag.as_str()), find(tag.primary())) {
        (Some(lang), _) => lang.english.to_owned(),
        (None, Some(lang)) => format!(
            "{} ({})",
            lang.english,
            &tag.as_str()[tag.primary().len() + 1..]
        ),
        (None, None) => tag.to_string(),
    }
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

pub const KEEP_ONE: &str = "Keep at least one meeting language.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Added,
    Removed {
        note: Option<String>,
    },
    /// The list keeps at least one language, so the last one stays.
    KeptLast,
}

/// Adds `tag` at the end of the meeting languages, or removes it. Removing the language that
/// Language is set to puts Language back on Automatic and returns the note that says so.
pub fn toggle(settings: &mut Settings, tag: &LangTag) -> Edit {
    let Some(ix) = settings.meeting_languages.iter().position(|t| t == tag) else {
        settings.meeting_languages.push(tag.clone());
        return Edit::Added;
    };
    if settings.meeting_languages.len() <= 1 {
        return Edit::KeptLast;
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

/// How well `query` names a language, best first; `None` when it does not.
fn rank(query: &str, lang: &Language) -> Option<u8> {
    let english = fold(lang.english);
    let native = fold(lang.native);
    let starts_a_word = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| word.starts_with(query))
    };
    if lang.code == query {
        Some(0)
    } else if english.starts_with(query) {
        Some(1)
    } else if native.starts_with(query) {
        Some(2)
    } else if starts_a_word(&english) || starts_a_word(&native) {
        Some(3)
    } else if lang.code.starts_with(query) {
        Some(4)
    } else {
        None
    }
}

/// The combobox's list for a search: every match in alphabetical order, so the list never
/// jumps while the user picks, and the best match to highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matches {
    pub rows: Vec<&'static Language>,
    pub best: Option<usize>,
}

/// Matches the English name, the language's own name, and the ISO code, ignoring case and
/// accents. "ge" finds German, Georgian, Swiss German, and Low German; "de" finds German.
pub fn search(query: &str) -> Matches {
    let query = fold(query.trim());
    let mut ranked: Vec<(&'static Language, u8)> = LANGUAGES
        .iter()
        .filter_map(|lang| {
            if query.is_empty() {
                Some((lang, 1))
            } else {
                rank(&query, lang).map(|r| (lang, r))
            }
        })
        .collect();
    ranked.sort_by(|a, b| fold(a.0.english).cmp(&fold(b.0.english)));
    let best = ranked
        .iter()
        .enumerate()
        .min_by_key(|(ix, (_, r))| (*r, *ix))
        .map(|(ix, _)| ix);
    Matches {
        rows: ranked.into_iter().map(|(lang, _)| lang).collect(),
        best,
    }
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

    fn found(query: &str) -> Vec<&'static str> {
        search(query).rows.iter().map(|l| l.english).collect()
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
        let names: Vec<&str> = matches.rows.iter().map(|l| l.english).collect();
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
    fn the_table_is_sorted_and_has_no_duplicates() {
        assert!(LANGUAGES.windows(2).all(|w| w[0].code < w[1].code));
        for lang in LANGUAGES {
            assert!(LangTag::parse(lang.code).is_ok(), "{}", lang.code);
        }
    }
}
