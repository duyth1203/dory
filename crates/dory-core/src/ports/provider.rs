use std::sync::Arc;

use super::{CancelToken, EngineError, ProvisionError};
use crate::models::{Availability, ModelEntry, ModelId, ModelKind, ProviderId};

/// One technology offering models of one kind: Sona, Ollama, sherpa-onnx, Apple Speech. Every
/// kind goes through this one lifecycle: catalog, availability, provision, open.
pub trait ModelProvider<K: ModelKind>: Send + Sync {
    fn id(&self) -> ProviderId;

    /// Static rows. No IO, no probing.
    fn catalog(&self) -> Vec<ModelEntry<K>>;

    /// Probes this machine now. Does not mutate anything.
    fn availability(&self, model: &ModelId<K>) -> Availability;

    /// Turns `NeedsProvision` into `Ready`: a download, an `ollama pull`, an OS asset. Resumable
    /// and idempotent. Only an explicit user action calls it, never a job.
    fn provision(
        &self,
        model: &ModelId<K>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError>;

    /// Loads the engine, which can take seconds. Dropping the last `Arc` must release the model
    /// and any child process.
    fn open(&self, model: &ModelId<K>, cancel: &CancelToken)
    -> Result<Arc<K::Engine>, EngineError>;
}

pub trait ProvisionSink {
    /// A human-readable phase such as "Downloading" or "Verifying".
    fn step(&mut self, label: &str);
    fn bytes(&mut self, done: u64, total: Option<u64>);
}
