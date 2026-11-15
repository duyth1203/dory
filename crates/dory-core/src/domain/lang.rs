use std::fmt;

use super::InvalidInput;

/// A BCP-47 language tag, normalized: lowercase primary subtag (`en`), uppercase two-letter region
/// (`en-US`), title-case script. Variants and extensions are not supported.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LangTag(String);

impl LangTag {
    pub fn parse(s: &str) -> Result<Self, InvalidInput> {
        let bad = || InvalidInput::new("language tag", format!("{s:?}"));
        let mut parts = s.split(['-', '_']);
        let primary = parts.next().unwrap_or_default();
        if !(2..=3).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(bad());
        }
        let mut out = primary.to_ascii_lowercase();
        for sub in parts {
            if !(2..=8).contains(&sub.len()) || !sub.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(bad());
            }
            out.push('-');
            let alpha = sub.bytes().all(|b| b.is_ascii_alphabetic());
            match (sub.len(), alpha) {
                (2, true) => out.push_str(&sub.to_ascii_uppercase()),
                (4, true) => {
                    out.push_str(&sub[..1].to_ascii_uppercase());
                    out.push_str(&sub[1..].to_ascii_lowercase());
                }
                _ => out.push_str(&sub.to_ascii_lowercase()),
            }
        }
        Ok(Self(out))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn primary(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }

    /// Whether a model that lists `self` as supported can serve `wanted`. A bare `en` on either
    /// side matches every English locale. Two regional tags must be equal.
    pub fn covers(&self, wanted: &LangTag) -> bool {
        self.primary() == wanted.primary()
            && (self == wanted
                || self.0.len() == self.primary().len()
                || wanted.0.len() == wanted.primary().len())
    }
}

impl fmt::Display for LangTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for LangTag {
    type Err = InvalidInput;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// The language a recording is transcribed in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum LanguagePref {
    #[default]
    Auto,
    Pinned(LangTag),
}

/// Languages a model handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LangSet {
    Any,
    Only(Vec<LangTag>),
}

impl LangSet {
    pub fn supports(&self, wanted: &LangTag) -> bool {
        match self {
            LangSet::Any => true,
            LangSet::Only(tags) => tags.iter().any(|t| t.covers(wanted)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    #[test]
    fn parse_normalizes_or_rejects() {
        let cases = [
            ("en", Some("en")),
            ("EN", Some("en")),
            ("en-us", Some("en-US")),
            ("en_US", Some("en-US")),
            ("zh-hans-cn", Some("zh-Hans-CN")),
            ("fil", Some("fil")),
            ("e", None),
            ("english", None),
            ("", None),
            ("en-", None),
            ("en-x", None),
            ("e1", None),
            ("en-US-toolongsubtag", None),
        ];
        for (input, want) in cases {
            assert_eq!(
                LangTag::parse(input).ok().as_ref().map(LangTag::as_str),
                want,
                "{input:?}"
            );
        }
    }

    #[test]
    fn supported_tag_covers_table() {
        let cases = [
            ("en", "en", true),
            ("en", "en-GB", true),
            ("en-US", "en", true),
            ("en-US", "en-US", true),
            ("en-US", "en-GB", false),
            ("en", "fr", false),
            ("fil", "fi", false),
        ];
        for (supported, wanted, want) in cases {
            assert_eq!(
                tag(supported).covers(&tag(wanted)),
                want,
                "{supported} covers {wanted}"
            );
        }
    }

    #[test]
    fn lang_set_supports() {
        assert!(LangSet::Any.supports(&tag("vi")));
        let only = LangSet::Only(vec![tag("en"), tag("fr")]);
        assert!(only.supports(&tag("fr-CA")));
        assert!(!only.supports(&tag("vi")));
        assert!(!LangSet::Only(vec![]).supports(&tag("en")));
    }
}
