//! Model selection. One lifecycle serves every kind of model: a provider lists static catalog
//! entries, probes availability, provisions on request, and opens an engine. The kind (speech,
//! summary, speakers) is a type parameter, so a speech id cannot be passed where a summary id is
//! expected, and one pure resolver serves all three.

mod catalog;
mod fit;
mod resolve;

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;

use crate::domain::InvalidInput;

pub use catalog::{Catalog, CatalogError};
pub use fit::{
    Diar, DiarCaps, DiarNeeds, DiarizationNeed, DiarizationSupport, Fit, KindTag, LangSupport, Llm,
    LlmCaps, LlmNeeds, Mismatch, ModelKind, Stt, SttCaps, SttNeeds, run_language,
};
pub use resolve::{Choice, ModelStatus, Resolution, resolve};

const MAX_ID_LEN: usize = 128;

/// The technology behind a group of models, such as `sona` or `ollama`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderId(Arc<str>);

impl ProviderId {
    pub fn parse(s: &str) -> Result<Self, InvalidInput> {
        let valid = s
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
            && s.len() <= MAX_ID_LEN;
        if valid {
            Ok(Self(s.into()))
        } else {
            Err(InvalidInput::new("provider id", format!("{s:?}")))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `provider/name`, such as `sona/whisper-large-v3-turbo` or `ollama/gemma4:e4b`. The kind makes
/// passing a speech id where a summary id belongs a compile error. Whether the model exists is
/// the catalog's question, not the parser's.
///
/// An id of one kind does not unify with another kind:
///
/// ```compile_fail,E0308
/// use dory_core::models::{Llm, ModelId, Stt};
///
/// fn takes_llm(_: &ModelId<Llm>) {}
/// let speech: ModelId<Stt> = "sona/x".parse().unwrap();
/// takes_llm(&speech);
/// ```
///
/// The same call with the right kind compiles, so the failure above is the kind and nothing else:
///
/// ```
/// use dory_core::models::{Llm, ModelId};
///
/// fn takes_llm(_: &ModelId<Llm>) {}
/// let summary: ModelId<Llm> = "ollama/x".parse().unwrap();
/// takes_llm(&summary);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId<K: ModelKind> {
    provider: ProviderId,
    name: Arc<str>,
    kind: PhantomData<fn() -> K>,
}

impl<K: ModelKind> ModelId<K> {
    pub fn new(provider: ProviderId, name: &str) -> Result<Self, InvalidInput> {
        let valid = !name.is_empty()
            && name.len() <= MAX_ID_LEN
            && name.bytes().all(|b| b.is_ascii_graphic());
        if !valid {
            return Err(InvalidInput::new("model name", format!("{name:?}")));
        }
        Ok(Self {
            provider,
            name: name.into(),
            kind: PhantomData,
        })
    }

    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Drops the kind, for provenance and blockers where the kind no longer matters.
    pub fn erase(&self) -> ModelRef {
        ModelRef(format!("{}/{}", self.provider, self.name).into())
    }
}

impl<K: ModelKind> FromStr for ModelId<K> {
    type Err = InvalidInput;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, name) = s
            .split_once('/')
            .ok_or_else(|| InvalidInput::new("model id", format!("{s:?} is not provider/name")))?;
        Self::new(ProviderId::parse(provider)?, name)
    }
}

impl<K: ModelKind> fmt::Display for ModelId<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.name)
    }
}

/// A model id with its kind erased.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelRef(Arc<str>);

impl ModelRef {
    pub fn parse(s: &str) -> Result<Self, InvalidInput> {
        // The syntax is the same for every kind, so any kind can parse it.
        s.parse::<ModelId<Stt>>().map(|id| id.erase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One model a provider can offer. Cheap to build, no IO.
#[derive(Clone, Debug)]
pub struct ModelEntry<K: ModelKind> {
    pub id: ModelId<K>,
    pub display_name: String,
    /// What the model is good and bad at, for the model picker's explainer.
    pub about: ModelAbout,
    pub caps: K::Caps,
    /// The pick when several models fit equally.
    pub recommended: bool,
    /// Tie-break among equals. Higher wins. The provider sets it.
    pub rank: i16,
    pub disk_bytes: Option<u64>,
    /// Comes with the operating system: Dory never downloads it, and the system's own settings
    /// turn it on or off.
    pub built_in: bool,
}

/// Plain-language strengths and weaknesses of a model, written by its provider.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelAbout {
    /// One sentence on what the model is.
    pub summary: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
}

/// What a probe says about this machine right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// Absent, but the provider can fetch it when the user asks.
    NeedsProvision {
        bytes: Option<u64>,
    },
    Unavailable(Unavailable),
}

impl Availability {
    /// Lower is better, for `resolve`.
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Availability::Ready => 0,
            Availability::NeedsProvision { .. } => 1,
            Availability::Unavailable(_) => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: UnavailableReason,
    /// What the user can do about it, shown as the fix.
    pub fix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    UnsupportedPlatform,
    OsTooOld {
        required: String,
    },
    /// For example the region is not supported, or the Mac cannot run the model.
    NotEligible {
        detail: String,
    },
    /// A built-in model the user turned off in the system's settings. `settings_url` opens the
    /// page that turns it back on.
    TurnedOff {
        detail: String,
        settings_url: String,
    },
    ServerUnreachable {
        endpoint: String,
    },
    /// The operating system is still getting the model ready. Jobs blocked on it are tried again
    /// by the same triggers that free a job blocked on a stopped server.
    Preparing {
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_parse_table() {
        let cases: &[(&str, bool)] = &[
            ("sona/whisper-large-v3-turbo", true),
            ("ollama/gemma4:e4b", true),
            ("ollama/hf.co/user/model:q4", true),
            ("sherpa/qwen3-asr-0.6b", true),
            ("apple/speech", true),
            ("nodelimiter", false),
            ("/name", false),
            ("provider/", false),
            ("Upper/name", false),
            ("-lead/name", false),
            ("pro vider/name", false),
            ("provider/na me", false),
            ("provider/näme", false),
        ];
        for (input, ok) in cases {
            assert_eq!(input.parse::<ModelId<Stt>>().is_ok(), *ok, "{input:?}");
        }
    }

    #[test]
    fn id_round_trips_and_erases_to_the_same_text() {
        let id: ModelId<Llm> = "ollama/gemma4:e4b".parse().unwrap();
        assert_eq!(id.to_string(), "ollama/gemma4:e4b");
        assert_eq!(id.erase().as_str(), "ollama/gemma4:e4b");
        assert_eq!(
            (id.provider().as_str(), id.name()),
            ("ollama", "gemma4:e4b")
        );
    }
}
