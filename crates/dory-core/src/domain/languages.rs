//! The languages Dory knows by name. Pure data, shared by the summary prompt, which names the
//! meeting's language in words, and by every picker in the apps.

use super::LangTag;

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
