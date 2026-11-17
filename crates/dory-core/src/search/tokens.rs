//! What a word is. The index, the matcher, and the highlight all split text here, so a hit always
//! has something to highlight.

use std::ops::Range;

use unicode_normalization::UnicodeNormalization as _;
use unicode_normalization::char::is_combining_mark;

/// Bump this whenever [`tokens`] or [`fold`] changes what they return. A store that indexed with
/// another version rebuilds its index when it opens.
pub const TOKENS_VERSION: u32 = 2;

/// One word of a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// [`fold`] of the word. Never empty, and only letters and digits.
    pub folded: String,
    /// Bytes of the original text.
    pub span: Range<usize>,
}

/// The words of `text`, in order. A word is a run of letters and digits, with any combining marks
/// that follow them. Han, Hiragana, and Katakana have no spaces between words, so each of those
/// characters is a word of its own.
pub fn tokens(text: &str) -> impl Iterator<Item = Token> + '_ {
    Tokens { text, at: 0 }
}

struct Tokens<'a> {
    text: &'a str,
    at: usize,
}

impl Iterator for Tokens<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        loop {
            let rest = &self.text[self.at..];
            let (offset, first) = rest.char_indices().find(|(_, c)| in_word(*c))?;
            let start = self.at + offset;
            let mut end = start + first.len_utf8();
            let ends_word = |c: char| spaceless(c) || !(in_word(c) || is_combining_mark(c));
            for c in self.text[end..].chars() {
                let part_of_this = if spaceless(first) {
                    is_combining_mark(c)
                } else {
                    !ends_word(c)
                };
                if !part_of_this {
                    break;
                }
                end += c.len_utf8();
            }
            self.at = end;
            let folded = fold(&self.text[start..end]);
            if !folded.is_empty() {
                return Some(Token {
                    folded,
                    span: start..end,
                });
            }
        }
    }
}

/// Whether `c` can be inside a word. Rust calls some characters alphanumeric that SQLite's
/// unicode61 tokenizer, which splits the index again, reads as separators: the circled and squared
/// capital letters, some New Tai Lue and Vedic signs. Treating them as separators here keeps every
/// word the index holds a word of the matcher too. A test compares this with the bundled SQLite
/// over every code point.
fn in_word(c: char) -> bool {
    c.is_alphanumeric()
        && !matches!(c,
            '\u{19B0}'..='\u{19C0}'
            | '\u{19C8}'..='\u{19C9}'
            | '\u{1CF2}'..='\u{1CF3}'
            | '\u{1F150}'..='\u{1F169}'
            | '\u{1F170}'..='\u{1F189}'
        )
}

/// Scripts written without spaces between words.
fn spaceless(c: char) -> bool {
    matches!(c,
        '\u{3005}'..='\u{3007}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{31F0}'..='\u{31FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF66}'..='\u{FF9F}'
        | '\u{20000}'..='\u{323AF}'
    )
}

/// The one rule for comparing words: lowercase, compatibility decomposition (so the `ﬁ` ligature
/// is "fi"), no accents, and a table for the letters decomposition leaves alone, so "muller" finds
/// "Müller", "strasse" finds "Straße", and "orsted" finds "Ørsted". The Greek final sigma is a
/// plain sigma, as it is in the index. Keeps only letters and digits, and Hangul stays in
/// syllables.
pub fn fold(word: &str) -> String {
    if word.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return word.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(word.len());
    for c in word.nfkd().flat_map(char::to_lowercase) {
        if is_combining_mark(c) || !in_word(c) {
            continue;
        }
        match c {
            'ß' => out.push_str("ss"),
            'ς' => out.push('σ'),
            'ø' => out.push('o'),
            'ł' => out.push('l'),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'đ' => out.push('d'),
            'þ' => out.push_str("th"),
            c => out.push(c),
        }
    }
    // Decomposition splits each Hangul syllable into its letters. Composing again keeps a Korean
    // word as the syllables it was written in.
    if out.chars().any(|c| ('\u{1100}'..='\u{11FF}').contains(&c)) {
        return out.nfc().collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<(String, &str)> {
        tokens(text)
            .map(|t| (t.folded, &text[t.span]))
            .collect::<Vec<_>>()
    }

    #[test]
    fn token_table() {
        let cases: &[(&str, &[(&str, &str)])] = &[
            ("", &[]),
            ("  \n\t ", &[]),
            ("Hello, world!", &[("hello", "Hello"), ("world", "world")]),
            ("don't", &[("don", "don"), ("t", "t")]),
            (
                "Q3.5 in 2026",
                &[("q3", "Q3"), ("5", "5"), ("in", "in"), ("2026", "2026")],
            ),
            ("e-mail", &[("e", "e"), ("mail", "mail")]),
            (
                "ship it 🚀 now",
                &[("ship", "ship"), ("it", "it"), ("now", "now")],
            ),
            ("Müller", &[("muller", "Müller")]),
            ("Mu\u{308}ller", &[("muller", "Mu\u{308}ller")]),
            ("Straße", &[("strasse", "Straße")]),
            ("Ørsted", &[("orsted", "Ørsted")]),
            ("ﬁnance", &[("finance", "ﬁnance")]),
            (
                "Łódź œuvre Þór Đorđe ÆON",
                &[
                    ("lodz", "Łódź"),
                    ("oeuvre", "œuvre"),
                    ("thor", "Þór"),
                    ("dorde", "Đorđe"),
                    ("aeon", "ÆON"),
                ],
            ),
            ("Tiếng Việt", &[("tieng", "Tiếng"), ("viet", "Việt")]),
            (
                "東京タワーへ",
                &[
                    ("東", "東"),
                    ("京", "京"),
                    ("タ", "タ"),
                    ("ワ", "ワ"),
                    ("ー", "ー"),
                    ("へ", "へ"),
                ],
            ),
            (
                "会議はbudget",
                &[
                    ("会", "会"),
                    ("議", "議"),
                    ("は", "は"),
                    ("budget", "budget"),
                ],
            ),
            ("ｶﾞ", &[("カ", "ｶ"), ("", "ﾞ")]),
            ("½ cup", &[("12", "½"), ("cup", "cup")]),
            ("서울 회의", &[("서울", "서울"), ("회의", "회의")]),
            (
                "δρόμος ΔΡΟΜΟΣ δρομοϲ",
                &[
                    ("δρομοσ", "δρόμος"),
                    ("δρομοσ", "ΔΡΟΜΟΣ"),
                    ("δρομοσ", "δρομοϲ"),
                ],
            ),
            ("🅐 budget", &[("budget", "budget")]),
            ("a🅐b ᦰk", &[("a", "a"), ("b", "b"), ("k", "k")]),
        ];
        for (text, want) in cases {
            let want: Vec<(String, &str)> = want
                .iter()
                .filter(|(folded, _)| !folded.is_empty())
                .map(|(f, s)| (f.to_string(), *s))
                .collect();
            assert_eq!(words(text), want, "{text:?}");
        }
    }

    #[test]
    fn spans_are_bytes_of_the_original_text() {
        let text = "Grüße aus Köln";
        let spans: Vec<_> = tokens(text).map(|t| t.span).collect();
        assert_eq!(spans, [0..7, 8..11, 12..17]);
    }

    #[test]
    fn folded_words_hold_only_letters_and_digits() {
        let text = "Ⅻ ① ℌello ﬀ ǅ İstanbul ẞ ¼ x\u{2044}y";
        for token in tokens(text) {
            assert!(!token.folded.is_empty());
            assert!(token.folded.chars().all(char::is_alphanumeric), "{token:?}");
            assert_eq!(fold(&token.folded), token.folded, "fold is idempotent");
        }
    }

    /// Any change to what [`tokens`] returns must bump [`TOKENS_VERSION`], or stores keep an
    /// index built by the old rule. Update the digest together with the version.
    #[test]
    fn the_token_rule_has_not_changed_without_a_version_bump() {
        let probe = "Hello, world! don't Q3.5 e-mail 🚀 Müller Mu\u{308}ller Straße Ørsted ﬁnance \
                     Łódź œuvre Þór Đorđe ÆON Tiếng Việt 東京タワーへ 会議はbudget ｶﾞ ½ 서울 회의 \
                     Ⅻ ① ℌello ﬀ ǅ İstanbul ẞ ¼ x\u{2044}y ΟΔΟΣ ﬃ ŉ δρόμος 🅐b ᦰk";
        let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
        for token in tokens(probe) {
            let line = format!("{}:{}:{};", token.folded, token.span.start, token.span.end);
            for byte in line.bytes() {
                digest ^= u64::from(byte);
                digest = digest.wrapping_mul(0x0100_0000_01b3);
            }
        }
        assert_eq!(
            (TOKENS_VERSION, digest),
            (2, 0x7cac_f337_bae3_a3a8),
            "bump TOKENS_VERSION"
        );
    }
}
