//! Running a recording's transcript, summary, or summary translation again, with the model the
//! user picks for that one run or with their Settings choice.
//!
//! The pick rides on the job ([`Picks`]), so a retry, an unblock, or a restart runs the same model,
//! and Settings never change. The list a re-run offers and the check on a pick both measure fit
//! with the needs the stage itself resolves against, so a model the list offers is one the stage
//! accepts.

use super::models::{AppKind, ModelRow, Registries};
use super::stages::{speech_needs, summary_needs};
use crate::domain::{InvalidInput, LangTag, LanguagePref, Ms, Picks, Recording, Settings};
use crate::models::{
    Choice, Fit, Llm, LlmNeeds, ModelId, ModelStatus, Resolution, Stt, SttNeeds, auto_order,
    resolve,
};
use crate::ports::{StoreError, StoreRead};

/// A re-run the user asked for. `None` runs the Settings choice, as every run did before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rerun {
    /// Transcribes again in `language`, then does the rest of the pipeline with Settings. The
    /// recording's waiting and running jobs are cancelled, so the transcript that lands is this one.
    Transcribe {
        language: LanguagePref,
        stt: Option<ModelId<Stt>>,
    },
    /// Writes the summary again from the stored transcript. The current summary is kept, so the
    /// user can restore it.
    Summarize { llm: Option<ModelId<Llm>> },
    /// The summary in `target`, beside the original. Refused like
    /// [`crate::domain::SummaryTranslation::refusal`] says, and while a translation runs.
    TranslateSummary {
        target: LangTag,
        llm: Option<ModelId<Llm>>,
    },
}

impl Rerun {
    pub(crate) fn picks(&self) -> Picks {
        match self {
            Rerun::Transcribe { stt, .. } => Picks {
                stt: stt.clone(),
                llm: None,
            },
            Rerun::Summarize { llm } | Rerun::TranslateSummary { llm, .. } => Picks {
                stt: None,
                llm: llm.clone(),
            },
        }
    }
}

/// The models a re-run can use for one recording.
#[derive(Clone, Debug)]
pub struct RerunMenu<K: AppKind> {
    /// The Settings choice, which a re-run without a pick follows.
    pub setting: Choice<K>,
    /// What the Settings choice resolves to now, which a re-run without a pick runs.
    pub default: Resolution<K>,
    /// Every model that can do this recording's job, in the order `Auto` tries them, each with
    /// its availability. A model whose fit is `No` is left out. `chosen` marks the model Settings
    /// pins, if any.
    pub models: Vec<ModelRow<K>>,
}

pub(crate) fn menu<K: AppKind>(
    statuses: &[ModelStatus<K>],
    settings: &Settings,
    needs: &K::Needs,
) -> RerunMenu<K> {
    let choice = K::choice(settings);
    RerunMenu {
        setting: choice.clone(),
        default: resolve(choice, statuses, needs),
        models: auto_order(statuses, needs)
            .into_iter()
            .map(|status| ModelRow {
                fit: K::fit(&status.entry.caps, needs),
                chosen: matches!(choice, Choice::Pinned(id) if *id == status.entry.id),
                status: status.clone(),
            })
            .collect(),
    }
}

/// Refuses a pick the catalog does not hold or that cannot do this recording's job, which the
/// menu never offers. Availability is left to the stage, which blocks the job with the fix.
pub(crate) fn check<K: AppKind>(
    models: &Registries,
    pick: &ModelId<K>,
    needs: &K::Needs,
) -> Result<(), InvalidInput> {
    let entry = models
        .get::<K>()
        .entry(pick)
        .ok_or_else(|| InvalidInput::new("model", format!("{pick} is not in the catalog")))?;
    match K::fit(&entry.caps, needs) {
        Fit::No(why) => Err(InvalidInput::new(
            "model",
            format!(
                "{} cannot do this recording's job: {why:?}",
                entry.display_name
            ),
        )),
        Fit::Yes | Fit::Degraded(_) => Ok(()),
    }
}

/// The speech needs of transcribing `recording` again in `language`.
pub(crate) fn rerun_speech_needs(
    recording: &Recording,
    language: &LanguagePref,
    settings: &Settings,
) -> SttNeeds {
    let recording = Recording {
        language: language.clone(),
        ..recording.clone()
    };
    let input = recording.take.as_ref().map_or(Ms(0), |t| t.duration);
    speech_needs(&recording, settings, input)
}

/// The summary needs of writing `recording`'s summary again. A recording with no transcript yet
/// is measured as Settings lists models, since its summary stage has nothing to read.
pub(crate) fn rerun_summary_needs(
    store: &dyn StoreRead,
    models: &Registries,
    recording: &Recording,
    settings: &Settings,
) -> Result<LlmNeeds, StoreError> {
    Ok(match store.transcript(&recording.id)? {
        Some(transcript) => summary_needs(models, recording, &transcript, settings),
        None => Llm::listing_needs(settings),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::LangSet;
    use crate::models::{Availability, LlmCaps, Mismatch, ModelEntry};

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn llm(
        id: &str,
        languages: LangSet,
        recommended: bool,
        availability: Availability,
    ) -> ModelStatus<Llm> {
        ModelStatus {
            entry: ModelEntry {
                id: id.parse().unwrap(),
                display_name: id.into(),
                about: Default::default(),
                caps: LlmCaps {
                    context_tokens: 4096,
                    languages,
                },
                recommended,
                rank: 0,
                disk_bytes: None,
                built_in: false,
            },
            availability,
        }
    }

    fn catalog() -> Vec<ModelStatus<Llm>> {
        vec![
            llm(
                "apple/foundation-model",
                LangSet::Only(vec![tag("en"), tag("fr")]),
                false,
                Availability::Ready,
            ),
            llm(
                "ollama/big",
                LangSet::Any,
                false,
                Availability::NeedsProvision { bytes: Some(9) },
            ),
            llm("ollama/small", LangSet::Any, true, Availability::Ready),
        ]
    }

    fn needs(spoken: &[&str]) -> LlmNeeds {
        LlmNeeds {
            main: spoken.first().map(|t| tag(t)),
            spoken: spoken.iter().map(|t| tag(t)).collect(),
        }
    }

    fn ids(menu: &RerunMenu<Llm>) -> Vec<String> {
        menu.models
            .iter()
            .map(|r| r.status.entry.id.to_string())
            .collect()
    }

    #[test]
    fn a_menu_leaves_out_models_that_cannot_read_the_meeting() {
        let settings = Settings::default();
        let tagalog = menu(&catalog(), &settings, &needs(&["tl"]));
        assert_eq!(
            ids(&tagalog),
            ["ollama/small", "ollama/big"],
            "Apple Intelligence cannot read Tagalog, so it is not offered"
        );
        let english = menu(&catalog(), &settings, &needs(&["en"]));
        assert_eq!(
            ids(&english),
            ["ollama/small", "apple/foundation-model", "ollama/big"],
            "in Auto's order (recommended, then rank, then id), and a model that needs a download stays listed"
        );
        assert!(matches!(
            english.models[2].status.availability,
            Availability::NeedsProvision { bytes: Some(9) }
        ));
    }

    #[test]
    fn the_default_is_what_settings_resolves_to_now() {
        let auto = Settings::default();
        assert_eq!(
            menu(&catalog(), &auto, &needs(&["en"])).default,
            Resolution::Use("ollama/small".parse().unwrap())
        );
        let pinned = Settings {
            llm: Choice::Pinned("apple/foundation-model".parse().unwrap()),
            ..Settings::default()
        };
        let english = menu(&catalog(), &pinned, &needs(&["en"]));
        assert_eq!(
            english.default,
            Resolution::Use("apple/foundation-model".parse().unwrap())
        );
        assert_eq!(
            english
                .models
                .iter()
                .filter(|r| r.chosen)
                .map(|r| r.status.entry.id.to_string())
                .collect::<Vec<_>>(),
            ["apple/foundation-model"]
        );
        assert!(
            matches!(
                menu(&catalog(), &pinned, &needs(&["tl"])).default,
                Resolution::Blocked(crate::domain::Blocker::Incompatible {
                    why: Mismatch::LanguageUnsupported(_),
                    ..
                })
            ),
            "a Settings pin that cannot read the meeting says so on the default item"
        );
    }
}
