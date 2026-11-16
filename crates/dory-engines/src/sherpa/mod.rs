//! Speech to text in process through sherpa-onnx.

mod catalog;
mod engine;
mod fetch;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use dory_core::domain::{LangSet, LangTag, Ms};
use dory_core::models::{
    Availability, DiarizationSupport, LangSupport, ModelAbout, ModelEntry, ModelId, ProviderId,
    Stt, SttCaps, Unavailable, UnavailableReason,
};
use dory_core::ports::{
    CancelToken, EngineError, ModelProvider, ProvisionError, ProvisionSink, Transcriber,
};

use catalog::Model;
use engine::SherpaEngine;

/// `ModelProvider<Stt>` over sherpa-onnx offline recognizers.
pub struct Sherpa {
    dir: PathBuf,
    base_url: String,
    models: Vec<Model>,
}

impl Sherpa {
    /// Where the archives are published.
    pub const RELEASES: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";

    /// `dir` holds one directory per installed model. `base_url` is where archives are fetched
    /// from, normally [`Sherpa::RELEASES`].
    pub fn new(dir: impl Into<PathBuf>, base_url: impl Into<String>) -> Sherpa {
        Sherpa {
            dir: dir.into(),
            base_url: base_url.into(),
            models: catalog::models(),
        }
    }

    fn model(&self, id: &ModelId<Stt>) -> Option<&Model> {
        (id.provider().as_str() == "sherpa")
            .then(|| self.models.iter().find(|m| m.name == id.name()))
            .flatten()
    }
}

fn provider_id() -> ProviderId {
    ProviderId::parse("sherpa").expect("a valid provider id")
}

fn entry(model: &Model) -> ModelEntry<Stt> {
    let tags = model
        .languages
        .iter()
        .map(|(tag, _)| LangTag::parse(tag).expect("catalog tags are valid"))
        .collect();
    let strings = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect();
    ModelEntry {
        id: ModelId::new(provider_id(), model.name).expect("catalog names are valid"),
        display_name: model.display_name.into(),
        about: ModelAbout {
            summary: model.summary.into(),
            pros: strings(model.pros),
            cons: strings(model.cons),
        },
        caps: SttCaps {
            languages: LangSupport::Multilingual {
                auto_detect: true,
                tags: LangSet::Only(tags),
            },
            diarization: DiarizationSupport::None,
            max_input: None,
            window: Some(Ms(model.window_secs * 1000)),
        },
        recommended: model.recommended,
        rank: model.rank,
        disk_bytes: Some(model.disk_bytes),
    }
}

impl ModelProvider<Stt> for Sherpa {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn catalog(&self) -> Vec<ModelEntry<Stt>> {
        self.models.iter().map(entry).collect()
    }

    fn availability(&self, id: &ModelId<Stt>) -> Availability {
        let Some(model) = self.model(id) else {
            return Availability::Unavailable(Unavailable {
                reason: UnavailableReason::NotEligible {
                    detail: format!("{id} is not a sherpa-onnx model"),
                },
                fix: None,
            });
        };
        if fetch::is_installed(&fetch::model_dir(&self.dir, model), model.layout) {
            Availability::Ready
        } else {
            Availability::NeedsProvision {
                bytes: Some(fetch::remaining(&self.dir, model)),
            }
        }
    }

    fn provision(
        &self,
        id: &ModelId<Stt>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError> {
        let model = self.model(id).ok_or_else(|| {
            ProvisionError::NotProvisionable(format!("{id} is not in the catalog"))
        })?;
        std::fs::create_dir_all(&self.dir).map_err(|e| ProvisionError::Other(e.to_string()))?;
        fetch::provision(&self.dir, &self.base_url, model, progress, cancel)
    }

    fn open(
        &self,
        id: &ModelId<Stt>,
        cancel: &CancelToken,
    ) -> Result<Arc<dyn Transcriber>, EngineError> {
        let model = self
            .model(id)
            .ok_or_else(|| EngineError::ModelMissing(format!("{id} is not in the catalog")))?;
        let dir = fetch::model_dir(&self.dir, model);
        if !fetch::is_installed(&dir, model.layout) {
            return Err(EngineError::ModelMissing(format!(
                "{} is not downloaded",
                model.display_name
            )));
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        Ok(Arc::new(SherpaEngine::open(model.clone(), dir)?))
    }
}
