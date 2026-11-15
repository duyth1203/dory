//! Stands in for the speech engine until the real ones are registered.
//!
//! The core refuses to open with no speech model, because the user cannot turn speech off. So
//! until Sona and the others land, the root registers one model that is never available. A job
//! then blocks on it, naming why, and `dory-cli models` shows the gap. Delete this when the first
//! real speech provider is added to `providers()`.

use std::sync::Arc;

use dory_core::domain::LangSet;
use dory_core::models::{
    Availability, DiarizationSupport, LangSupport, ModelEntry, ModelId, ProviderId, Stt, SttCaps,
    Unavailable, UnavailableReason,
};
use dory_core::ports::{
    CancelToken, EngineError, ModelProvider, ProvisionError, ProvisionSink, Transcriber,
};

const MODEL: &str = "pending/speech-engine";

pub(crate) struct PendingSpeech;

impl ModelProvider<Stt> for PendingSpeech {
    fn id(&self) -> ProviderId {
        ProviderId::parse("pending").expect("a valid provider id")
    }

    fn catalog(&self) -> Vec<ModelEntry<Stt>> {
        vec![ModelEntry {
            id: MODEL.parse().expect("a valid model id"),
            display_name: "No speech engine installed yet".into(),
            caps: SttCaps {
                languages: LangSupport::Multilingual {
                    auto_detect: true,
                    tags: LangSet::Any,
                },
                diarization: DiarizationSupport::None,
                max_input: None,
            },
            recommended: false,
            rank: i16::MIN,
            disk_bytes: None,
        }]
    }

    fn availability(&self, _: &ModelId<Stt>) -> Availability {
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::NotEligible {
                detail: "this build has no speech engine yet".into(),
            },
            fix: Some("use --fake-engines to try the pipeline with fake engines".into()),
        })
    }

    fn provision(
        &self,
        _: &ModelId<Stt>,
        _: &mut dyn ProvisionSink,
        _: &CancelToken,
    ) -> Result<(), ProvisionError> {
        Err(ProvisionError::NotProvisionable(
            "this build has no speech engine yet".into(),
        ))
    }

    fn open(
        &self,
        model: &ModelId<Stt>,
        _: &CancelToken,
    ) -> Result<Arc<dyn Transcriber>, EngineError> {
        Err(EngineError::ModelMissing(model.to_string()))
    }
}
