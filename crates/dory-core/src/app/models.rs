//! Model registries, one per kind, and the engines the pipeline worker keeps loaded.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use super::events::{Db, Event};
use super::pipeline::Nudge;
use crate::domain::{LanguagePref, Ms, Settings};
use crate::models::{
    Catalog, CatalogError, Choice, Diar, DiarNeeds, DiarizationNeed, Fit, KindTag, Llm, LlmNeeds,
    ModelEntry, ModelId, ModelKind, ModelRef, ModelStatus, Resolution, Stt, SttNeeds, resolve,
};
use crate::ports::{CancelToken, EngineError, ModelProvider, ProvisionError, ProvisionSink};

/// A kind of model the app manages: speech, summary, or speakers. Says where its choice lives in
/// [`Settings`] and what a listing measures fit against.
pub trait AppKind: ModelKind {
    fn choice(settings: &Settings) -> &Choice<Self>;
    fn set_choice(settings: &mut Settings, choice: Choice<Self>);
    /// The needs a model list shows fit against, before any recording is involved.
    fn listing_needs(settings: &Settings) -> Self::Needs;
}

impl AppKind for Stt {
    fn choice(settings: &Settings) -> &Choice<Stt> {
        &settings.stt
    }
    fn set_choice(settings: &mut Settings, choice: Choice<Stt>) {
        settings.stt = choice;
    }
    fn listing_needs(settings: &Settings) -> SttNeeds {
        SttNeeds {
            language: settings.language.clone(),
            diarization: DiarizationNeed::NotNeeded,
            input: Ms(0),
        }
    }
}

impl AppKind for Llm {
    fn choice(settings: &Settings) -> &Choice<Llm> {
        &settings.llm
    }
    fn set_choice(settings: &mut Settings, choice: Choice<Llm>) {
        settings.llm = choice;
    }
    fn listing_needs(settings: &Settings) -> LlmNeeds {
        LlmNeeds {
            language: match &settings.language {
                LanguagePref::Pinned(tag) => Some(tag.clone()),
                LanguagePref::Auto => None,
            },
        }
    }
}

impl AppKind for Diar {
    fn choice(settings: &Settings) -> &Choice<Diar> {
        &settings.diar
    }
    fn set_choice(settings: &mut Settings, choice: Choice<Diar>) {
        settings.diar = choice;
    }
    fn listing_needs(_: &Settings) -> DiarNeeds {
        DiarNeeds
    }
}

/// One row of a model list.
#[derive(Clone, Debug)]
pub struct ModelRow<K: ModelKind> {
    pub status: ModelStatus<K>,
    pub fit: Fit,
    pub chosen: bool,
}

pub(crate) struct Registry<K: ModelKind> {
    providers: Vec<Arc<dyn ModelProvider<K>>>,
    catalog: Catalog<K>,
    owner: HashMap<ModelId<K>, usize>,
}

impl<K: ModelKind> Registry<K> {
    fn new(providers: Vec<Arc<dyn ModelProvider<K>>>) -> Result<Self, CatalogError> {
        let mut entries = Vec::new();
        let mut owner = HashMap::new();
        for (index, provider) in providers.iter().enumerate() {
            for entry in provider.catalog() {
                owner.insert(entry.id.clone(), index);
                entries.push(entry);
            }
        }
        Ok(Self {
            catalog: Catalog::new(entries)?,
            providers,
            owner,
        })
    }

    pub fn catalog(&self) -> &Catalog<K> {
        &self.catalog
    }

    pub fn entry(&self, id: &ModelId<K>) -> Option<&ModelEntry<K>> {
        self.catalog.get(id)
    }

    pub fn provider(&self, id: &ModelId<K>) -> Option<&Arc<dyn ModelProvider<K>>> {
        self.owner.get(id).and_then(|i| self.providers.get(*i))
    }

    /// Probes every model. Can be slow, so never on a UI thread.
    pub fn statuses(&self) -> Vec<ModelStatus<K>> {
        self.catalog
            .entries()
            .iter()
            .filter_map(|entry| {
                Some(ModelStatus {
                    availability: self.provider(&entry.id)?.availability(&entry.id),
                    entry: entry.clone(),
                })
            })
            .collect()
    }

    pub fn resolve(&self, choice: &Choice<K>, needs: &K::Needs) -> Resolution<K> {
        resolve(choice, &self.statuses(), needs)
    }

    pub fn rows(&self, choice: &Choice<K>, needs: &K::Needs) -> Vec<ModelRow<K>> {
        self.statuses()
            .into_iter()
            .map(|status| ModelRow {
                fit: K::fit(&status.entry.caps, needs),
                chosen: matches!(choice, Choice::Pinned(id) if *id == status.entry.id),
                status,
            })
            .collect()
    }
}

pub(crate) struct Registries {
    stt: Registry<Stt>,
    llm: Registry<Llm>,
    diar: Registry<Diar>,
}

impl Registries {
    pub fn new(
        stt: Vec<Arc<dyn ModelProvider<Stt>>>,
        llm: Vec<Arc<dyn ModelProvider<Llm>>>,
        diar: Vec<Arc<dyn ModelProvider<Diar>>>,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            stt: Registry::new(stt)?,
            llm: Registry::new(llm)?,
            diar: Registry::new(diar)?,
        })
    }

    pub fn get<K: ModelKind>(&self) -> &Registry<K> {
        let registry: &dyn Any = match K::TAG {
            KindTag::Stt => &self.stt,
            KindTag::Llm => &self.llm,
            KindTag::Diar => &self.diar,
        };
        registry
            .downcast_ref()
            .expect("each kind's tag names its own registry")
    }

    /// Settings that pin a model no provider offers go back to `Auto`. Returns what was dropped.
    pub fn reconcile(&self, settings: &mut Settings) -> Vec<Event> {
        let mut events = Vec::new();
        reconcile_kind::<Stt>(self, settings, &mut events);
        reconcile_kind::<Llm>(self, settings, &mut events);
        reconcile_kind::<Diar>(self, settings, &mut events);
        events
    }
}

fn reconcile_kind<K: AppKind>(
    registries: &Registries,
    settings: &mut Settings,
    events: &mut Vec<Event>,
) {
    let (choice, removed) = registries
        .get::<K>()
        .catalog()
        .reconcile(K::choice(settings).clone());
    if let Some(removed) = removed {
        K::set_choice(settings, choice);
        events.push(Event::ChoiceReset {
            kind: K::TAG,
            removed: removed.erase(),
        });
    }
}

struct Loaded<K: ModelKind> {
    id: ModelId<K>,
    engine: Arc<K::Engine>,
    last_used: u64,
    in_use: bool,
}

struct Slot<K: ModelKind> {
    loaded: Option<Loaded<K>>,
}

impl<K: ModelKind> Slot<K> {
    fn get(
        &mut self,
        registry: &Registry<K>,
        id: &ModelId<K>,
        cancel: &CancelToken,
        now: u64,
    ) -> Result<Arc<K::Engine>, EngineError> {
        if let Some(loaded) = self.loaded.as_mut().filter(|l| l.id == *id) {
            loaded.last_used = now;
            loaded.in_use = true;
            return Ok(Arc::clone(&loaded.engine));
        }
        // Release the old model before loading another, so two never sit in memory together.
        self.loaded = None;
        let provider = registry
            .provider(id)
            .ok_or_else(|| EngineError::ModelMissing(id.to_string()))?;
        let engine = provider.open(id, cancel)?;
        self.loaded = Some(Loaded {
            id: id.clone(),
            engine: Arc::clone(&engine),
            last_used: now,
            in_use: true,
        });
        Ok(engine)
    }

    fn release(&mut self, now: u64) {
        if let Some(loaded) = self.loaded.as_mut().filter(|l| l.in_use) {
            loaded.in_use = false;
            loaded.last_used = now;
        }
    }

    fn sweep(&mut self, now: u64, idle_ms: u64) {
        if self.expiry(idle_ms).is_some_and(|at| at <= now) {
            self.loaded = None;
        }
    }

    fn expiry(&self, idle_ms: u64) -> Option<u64> {
        self.loaded
            .as_ref()
            .filter(|l| !l.in_use)
            .map(|l| l.last_used.saturating_add(idle_ms))
    }
}

/// The loaded engines, one per kind. Owned by the pipeline worker alone.
pub(crate) struct Engines {
    stt: Slot<Stt>,
    llm: Slot<Llm>,
    diar: Slot<Diar>,
    idle_ms: u64,
}

impl Engines {
    pub fn new(idle_ms: u64) -> Self {
        Self {
            stt: Slot { loaded: None },
            llm: Slot { loaded: None },
            diar: Slot { loaded: None },
            idle_ms,
        }
    }

    /// The loaded engine for `id`, loading it (and unloading another of its kind) if needed.
    pub fn get<K: ModelKind>(
        &mut self,
        registries: &Registries,
        id: &ModelId<K>,
        cancel: &CancelToken,
        now: u64,
    ) -> Result<Arc<K::Engine>, EngineError> {
        self.slot::<K>().get(registries.get::<K>(), id, cancel, now)
    }

    /// Marks the engines a stage used as idle from `now`.
    pub fn release(&mut self, now: u64) {
        self.stt.release(now);
        self.llm.release(now);
        self.diar.release(now);
    }

    pub fn sweep(&mut self, now: u64) {
        let idle = self.idle_ms;
        self.stt.sweep(now, idle);
        self.llm.sweep(now, idle);
        self.diar.sweep(now, idle);
    }

    pub fn next_expiry(&self) -> Option<u64> {
        let idle = self.idle_ms;
        [
            self.stt.expiry(idle),
            self.llm.expiry(idle),
            self.diar.expiry(idle),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn slot<K: ModelKind>(&mut self) -> &mut Slot<K> {
        let slot: &mut dyn Any = match K::TAG {
            KindTag::Stt => &mut self.stt,
            KindTag::Llm => &mut self.llm,
            KindTag::Diar => &mut self.diar,
        };
        slot.downcast_mut()
            .expect("each kind's tag names its own slot")
    }
}

/// Downloads in flight, one thread each.
#[derive(Default)]
pub(crate) struct Provisions {
    running: Mutex<HashMap<ModelRef, Download>>,
}

struct Download {
    cancel: CancelToken,
    /// Taken by `shutdown` to wait for it. A download that finishes on its own detaches.
    thread: Option<JoinHandle<()>>,
}

impl Provisions {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ModelRef, Download>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts provisioning on its own thread. `done` receives the result, and is dropped unsent
    /// if the thread cannot start. A model already being provisioned is not started twice.
    pub fn start<K: ModelKind>(
        self: &Arc<Self>,
        provider: Arc<dyn ModelProvider<K>>,
        id: ModelId<K>,
        db: Db,
        nudge: Nudge,
        done: async_channel::Sender<Result<(), ProvisionError>>,
    ) {
        let model = id.erase();
        let mut running = self.lock();
        if running.contains_key(&model) {
            let _ = done.try_send(Err(ProvisionError::Other(format!(
                "{model} is already being provisioned"
            ))));
            return;
        }
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let this = Arc::clone(self);
        let key = model.clone();
        let spawned = std::thread::Builder::new()
            .name("dory-provision".into())
            .spawn(move || {
                let mut sink = ProgressEvents {
                    db: &db,
                    model: &key,
                    step: String::new(),
                };
                // A panicking provider must still free its entry and answer its caller.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    provider.provision(&id, &mut sink, &token)
                }))
                .unwrap_or_else(|_| Err(ProvisionError::Other("the provider panicked".into())));
                db.publish(Event::ProvisionFinished {
                    model: key.clone(),
                    result: result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
                });
                if result.is_ok() {
                    super::pipeline::unblock_all(&db);
                    nudge.wake();
                }
                this.lock().remove(&key);
                let _ = done.try_send(result);
            });
        if let Ok(thread) = spawned {
            running.insert(
                model,
                Download {
                    cancel,
                    thread: Some(thread),
                },
            );
        }
    }

    pub fn cancel(&self, model: &ModelRef) {
        if let Some(download) = self.lock().get(model) {
            download.cancel.cancel();
        }
    }

    pub fn shutdown(&self) {
        let threads: Vec<_> = self
            .lock()
            .values_mut()
            .filter_map(|download| {
                download.cancel.cancel();
                download.thread.take()
            })
            .collect();
        for thread in threads {
            let _ = thread.join();
        }
    }
}

struct ProgressEvents<'a> {
    db: &'a Db,
    model: &'a ModelRef,
    step: String,
}

impl ProvisionSink for ProgressEvents<'_> {
    fn step(&mut self, label: &str) {
        label.clone_into(&mut self.step);
        self.bytes(0, None);
    }

    fn bytes(&mut self, done: u64, total: Option<u64>) {
        self.db.publish(Event::ProvisionProgress {
            model: self.model.clone(),
            step: self.step.clone(),
            done,
            total,
        });
    }
}
