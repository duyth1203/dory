//! The application layer: the `Dory` facade over the recorder actor, the pipeline worker, the
//! models service, and the event hub. Everything here talks to the world through the ports only.
//!
//! Threads and ownership:
//!
//! - The recorder actor owns the recorder state, the device handle, the open take, and the grace
//!   deadline. While the device runs, a take writer thread owns the `TakeWriter` and returns it
//!   when the device stops, so the writer always has exactly one owner.
//! - The pipeline worker owns the running job and the loaded engines.
//! - Each provisioning and each sealing runs on its own short-lived thread.
//! - Every persisted fact goes through one `Db::apply`, which also announces it.

mod events;
mod models;
mod pipeline;
mod recorder;
mod stages;
mod writer;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

pub use events::{Event, Subscription, change_events};
pub use models::{AppKind, ModelRow};
pub use recorder::{RecorderGone, RecorderStatus, StartOptions};

use events::{Db, EventHub};
use models::{Provisions, Registries};
use pipeline::{Nudge, PipelineHandle};
use recorder::{Msg, RecorderHandle, commit_sealed};

use crate::domain::{
    Goal, InvalidInput, Job, JobEvent, JobId, JobState, LanguagePref, Priority, Recording,
    RecordingEdit, RecordingId, RecordingQuery, RecordingRow, Settings, SpeakerId, SpeakerNames,
    Summary, TakeContainer, Transcript,
};
use crate::models::{CatalogError, Choice, Diar, KindTag, Llm, ModelId, ModelRef, Stt};
use crate::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, InputDevice, Source,
};
use crate::ports::{
    Change, Clock, ModelProvider, ProvisionError, Store, StoreError, TakeError, Takes,
};
use crate::recorder::{Input, Limits};

/// Every adapter the core needs. Only the composition root builds this.
pub struct Ports {
    pub store: Arc<dyn Store>,
    pub takes: Arc<dyn Takes>,
    pub capture: Arc<dyn CaptureBackend>,
    pub clock: Arc<dyn Clock>,
    pub stt: Vec<Arc<dyn ModelProvider<Stt>>>,
    pub llm: Vec<Arc<dyn ModelProvider<Llm>>>,
    pub diar: Vec<Arc<dyn ModelProvider<Diar>>>,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub limits: Limits,
    /// How much audio the frame ring holds before overruns are padded with silence.
    pub ring: Duration,
    pub drain_every: Duration,
    pub level_every: Duration,
    pub checkpoint_every: Duration,
    /// The longest a timer waits before reading the injected `Clock` again.
    pub clock_poll: Duration,
    /// A loaded engine unused this long is dropped.
    pub engine_idle_ms: u64,
    /// The first retry waits this long, doubling per attempt.
    pub retry_base_ms: u64,
    /// Events each subscriber can fall behind by before it loses some and gets `Resync`.
    pub event_capacity: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            ring: Duration::from_secs(2),
            drain_every: Duration::from_millis(10),
            level_every: Duration::from_millis(100),
            checkpoint_every: Duration::from_secs(5),
            clock_poll: Duration::from_secs(1),
            engine_idle_ms: 5 * 60 * 1000,
            retry_base_ms: 10_000,
            event_capacity: 256,
        }
    }
}

impl Options {
    fn ring_frames(&self) -> usize {
        let frames = u128::from(self.limits.rate_hz.get()) * self.ring.as_millis() / 1000;
        usize::try_from(frames).unwrap_or(usize::MAX).max(1)
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Take(#[from] TakeError),
    #[error("could not start a thread")]
    Thread(#[from] std::io::Error),
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DoryError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Take(#[from] TakeError),
    #[error(transparent)]
    Invalid(#[from] InvalidInput),
    #[error("{0} is being recorded")]
    Recording(RecordingId),
    #[error(transparent)]
    RecorderGone(#[from] RecorderGone),
}

/// What startup found and repaired after the last run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Takes left open by a crash, now sealed and listed.
    pub sealed: Vec<RecordingId>,
    /// Audio whose recording was deleted before the crash.
    pub deleted_orphans: Vec<RecordingId>,
    /// Jobs that were running when the process died.
    pub interrupted: Vec<JobId>,
    pub choices_reset: Vec<(KindTag, ModelRef)>,
    /// Takes that could not be repaired, with why. They are tried again on the next start.
    pub problems: Vec<(RecordingId, String)>,
}

pub struct Dory {
    db: Db,
    takes: Arc<dyn Takes>,
    capture: Arc<dyn CaptureBackend>,
    clock: Arc<dyn Clock>,
    models: Arc<Registries>,
    provisions: Arc<Provisions>,
    recorder: RecorderHandle,
    pipeline: PipelineHandle,
    recovery: Recovery,
}

impl Dory {
    /// Validates the catalogs, repairs what a crash left behind, and starts the actors.
    pub fn open(ports: Ports, options: Options) -> Result<Dory, OpenError> {
        let db = Db {
            store: ports.store,
            hub: EventHub::new(options.event_capacity),
        };
        let models = Arc::new(Registries::new(ports.stt, ports.llm, ports.diar)?);
        let mut recovery = Recovery::default();

        let mut settings = db.store.settings()?;
        let resets = models.reconcile(&mut settings);
        if !resets.is_empty() {
            db.apply(vec![Change::SettingsReplaced(settings)])?;
        }
        for event in resets {
            if let Event::ChoiceReset { kind, removed } = &event {
                recovery.choices_reset.push((*kind, removed.clone()));
            }
            db.publish(event);
        }
        recover(
            &db,
            ports.takes.as_ref(),
            ports.clock.as_ref(),
            &mut recovery,
        )?;
        // A model installed or a server started while the app was closed must free its jobs.
        pipeline::unblock_all(&db);

        let (nudge, wake) = pipeline::nudge_channel();
        let pipeline = pipeline::spawn(
            pipeline::Deps {
                db: db.clone(),
                takes: Arc::clone(&ports.takes),
                clock: Arc::clone(&ports.clock),
                models: Arc::clone(&models),
            },
            &options,
            nudge.clone(),
            wake,
        )?;
        let recorder = recorder::spawn(
            recorder::Deps {
                db: db.clone(),
                takes: Arc::clone(&ports.takes),
                capture: Arc::clone(&ports.capture),
                clock: Arc::clone(&ports.clock),
                nudge,
            },
            &options,
        )?;
        Ok(Dory {
            db,
            takes: ports.takes,
            capture: ports.capture,
            clock: ports.clock,
            models,
            provisions: Arc::new(Provisions::default()),
            recorder,
            pipeline,
            recovery,
        })
    }

    pub fn recorder(&self) -> Recorder<'_> {
        Recorder { dory: self }
    }

    pub fn library(&self) -> Library<'_> {
        Library { dory: self }
    }

    pub fn pipeline(&self) -> Pipeline<'_> {
        Pipeline { dory: self }
    }

    pub fn models(&self) -> Models<'_> {
        Models { dory: self }
    }

    pub fn settings(&self) -> Result<Settings, StoreError> {
        self.db.store.settings()
    }

    /// Validates and saves `settings`. Blocked jobs are tried again, since the change may fix them.
    pub fn update_settings(&self, settings: Settings) -> Result<(), DoryError> {
        settings.validate()?;
        self.check_pinned::<Stt>(&settings.stt)?;
        self.check_pinned::<Llm>(&settings.llm)?;
        self.check_pinned::<Diar>(&settings.diar)?;
        self.db.apply(vec![Change::SettingsReplaced(settings)])?;
        self.unblock();
        Ok(())
    }

    pub fn events(&self) -> Subscription {
        self.db.hub.subscribe()
    }

    /// What startup repaired.
    pub fn recovery(&self) -> &Recovery {
        &self.recovery
    }

    /// Saves whatever is being recorded, stops the pipeline after its current stage (that job
    /// goes back to the queue), and stops downloads. Safe to call more than once.
    pub fn shutdown(&self) {
        self.recorder.shutdown();
        self.pipeline.shutdown();
        self.provisions.shutdown();
    }

    fn check_pinned<K: AppKind>(&self, choice: &Choice<K>) -> Result<(), InvalidInput> {
        match choice {
            Choice::Pinned(id) if self.models.get::<K>().entry(id).is_none() => Err(
                InvalidInput::new("model choice", format!("{id} is not in the catalog")),
            ),
            _ => Ok(()),
        }
    }

    fn unblock(&self) {
        pipeline::unblock_all(&self.db);
        self.pipeline.nudge.wake();
    }

    fn nudge(&self) -> Nudge {
        self.pipeline.nudge.clone()
    }
}

impl Drop for Dory {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Runs `work` on its own thread and resolves with its result, so a probe never blocks the
/// caller's executor.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = T> + Send + 'static {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new()
        .name("dory-probe".into())
        .spawn(move || {
            let _ = tx.send_blocking(work());
        });
    async move { rx.recv().await.expect("the probe thread died") }
}

pub struct Recorder<'a> {
    dory: &'a Dory,
}

impl Recorder<'_> {
    pub fn start(&self, options: StartOptions) -> Result<(), RecorderGone> {
        self.dory.recorder.send(Msg::Start(options))
    }

    pub fn pause(&self) -> Result<(), RecorderGone> {
        self.command(Input::Pause)
    }

    pub fn resume(&self) -> Result<(), RecorderGone> {
        self.command(Input::Resume)
    }

    /// Stops capture. The stop can be undone until the grace period ends.
    pub fn stop(&self) -> Result<(), RecorderGone> {
        self.command(Input::Stop)
    }

    pub fn undo_stop(&self) -> Result<(), RecorderGone> {
        self.command(Input::Undo)
    }

    pub fn status(&self) -> RecorderStatus {
        self.dory.recorder.status()
    }

    pub fn probe(&self) -> impl Future<Output = CaptureCaps> + Send + 'static {
        let capture = Arc::clone(&self.dory.capture);
        off_thread(move || capture.probe())
    }

    pub fn input_devices(
        &self,
    ) -> impl Future<Output = Result<Vec<InputDevice>, CaptureError>> + Send + 'static {
        let capture = Arc::clone(&self.dory.capture);
        off_thread(move || capture.input_devices())
    }

    /// Shows the OS prompt if needed and resolves once the user answers.
    pub fn request_access(
        &self,
        source: Source,
    ) -> impl Future<Output = Result<Access, CaptureError>> + Send + 'static {
        let capture = Arc::clone(&self.dory.capture);
        off_thread(move || capture.request_access(source))
    }

    fn command(&self, input: Input) -> Result<(), RecorderGone> {
        self.dory.recorder.send(Msg::Command(input))
    }
}

pub struct Library<'a> {
    dory: &'a Dory,
}

impl Library<'_> {
    pub fn list(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError> {
        self.dory.db.store.recordings(query)
    }

    pub fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        self.dory.db.store.recording(id)
    }

    pub fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError> {
        self.dory.db.store.transcript(id)
    }

    pub fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.dory.db.store.summary(id)
    }

    pub fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError> {
        self.dory.db.store.speaker_names(id)
    }

    pub fn jobs(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError> {
        self.dory.db.store.jobs_for(id)
    }

    pub fn set_title(&self, id: &RecordingId, title: Option<String>) -> Result<(), DoryError> {
        self.edit(id, RecordingEdit::Title(title))
    }

    pub fn set_language(&self, id: &RecordingId, language: LanguagePref) -> Result<(), DoryError> {
        self.edit(id, RecordingEdit::Language(language))
    }

    /// Names a speaker of the current transcript revision. `None` or blank clears the name.
    pub fn rename_speaker(
        &self,
        id: &RecordingId,
        speaker: SpeakerId,
        name: Option<String>,
    ) -> Result<(), DoryError> {
        let store = &self.dory.db.store;
        let revision = store
            .transcript(id)?
            .ok_or_else(|| StoreError::NotFound(format!("transcript of {id}")))?
            .revision;
        let mut names = store.speaker_names(id)?;
        names.rename(revision, speaker, name);
        self.dory.db.apply(vec![Change::SpeakerNamesReplaced {
            id: id.clone(),
            names,
        }])?;
        Ok(())
    }

    /// Deletes the recording and its audio. Refused while it is being recorded.
    pub fn delete(&self, id: &RecordingId) -> Result<(), DoryError> {
        if self.dory.recorder.status().id() == Some(id) {
            return Err(DoryError::Recording(id.clone()));
        }
        self.dory
            .db
            .apply(vec![Change::RecordingDeleted(id.clone())])?;
        self.dory
            .pipeline
            .current
            .cancel_if(|_, recording| recording == id);
        match self.dory.takes.delete(id) {
            Ok(()) | Err(TakeError::Missing(_)) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        self.dory.takes.playback_path(id)
    }

    fn edit(&self, id: &RecordingId, edit: RecordingEdit) -> Result<(), DoryError> {
        self.dory.db.apply(vec![Change::RecordingEdited {
            id: id.clone(),
            edit,
        }])?;
        Ok(())
    }
}

pub struct Pipeline<'a> {
    dory: &'a Dory,
}

impl Pipeline<'_> {
    pub fn enqueue(&self, id: &RecordingId, goal: Goal) -> Result<(), DoryError> {
        self.enqueue_with(id, goal, Priority::Normal)
    }

    pub fn enqueue_front(&self, id: &RecordingId, goal: Goal) -> Result<(), DoryError> {
        self.enqueue_with(id, goal, Priority::Front)
    }

    /// Cancels a queued, running, or blocked job. A running stage stops at its next check.
    pub fn cancel(&self, job: JobId) -> Result<(), DoryError> {
        // Commit first: the worker then finds the job cancelled whatever its stage returns, and
        // a cancel that loses a race with completion fails here instead of being reported done.
        self.event(job, JobEvent::Cancelled)?;
        self.dory
            .pipeline
            .current
            .cancel_if(|running, _| running == job);
        Ok(())
    }

    /// Tries a failed, cancelled, or blocked job again from its first unfinished stage.
    pub fn retry(&self, job: JobId) -> Result<(), DoryError> {
        self.event(job, JobEvent::Retake)
    }

    pub fn unblock(&self, job: JobId) -> Result<(), DoryError> {
        self.event(job, JobEvent::Unblock)
    }

    pub fn promote(&self, job: JobId) -> Result<(), DoryError> {
        self.event(job, JobEvent::Promote)
    }

    pub fn job(&self, job: JobId) -> Result<Option<Job>, StoreError> {
        self.dory.db.store.job(job)
    }

    fn enqueue_with(
        &self,
        id: &RecordingId,
        goal: Goal,
        priority: Priority,
    ) -> Result<(), DoryError> {
        self.dory.db.apply(vec![Change::JobEnqueued {
            id: id.clone(),
            goal,
            priority,
            at: self.dory.clock.now(),
        }])?;
        self.dory.pipeline.nudge.wake();
        Ok(())
    }

    fn event(&self, job: JobId, event: JobEvent) -> Result<(), DoryError> {
        self.dory.db.apply(vec![Change::Job { job, event }])?;
        self.dory.pipeline.nudge.wake();
        Ok(())
    }
}

pub struct Models<'a> {
    dory: &'a Dory,
}

impl Models<'_> {
    /// Every model of kind `K`, probed now, with how it fits the default settings.
    pub fn list<K: AppKind>(
        &self,
    ) -> impl Future<Output = Result<Vec<ModelRow<K>>, StoreError>> + Send + 'static {
        let models = Arc::clone(&self.dory.models);
        let store = Arc::clone(&self.dory.db.store);
        off_thread(move || {
            let settings = store.settings()?;
            Ok(models
                .get::<K>()
                .rows(K::choice(&settings), &K::listing_needs(&settings)))
        })
    }

    pub fn choose<K: AppKind>(&self, choice: Choice<K>) -> Result<(), DoryError> {
        let mut settings = self.dory.settings()?;
        K::set_choice(&mut settings, choice);
        self.dory.update_settings(settings)
    }

    /// Downloads or installs a model, publishing progress events. Blocked jobs are tried again
    /// when it succeeds.
    pub fn provision<K: AppKind>(
        &self,
        id: ModelId<K>,
    ) -> impl Future<Output = Result<(), ProvisionError>> + Send + 'static {
        let (done, rx) = async_channel::bounded(1);
        match self.dory.models.get::<K>().provider(&id) {
            Some(provider) => self.dory.provisions.start(
                Arc::clone(provider),
                id,
                self.dory.db.clone(),
                self.dory.nudge(),
                done,
            ),
            None => {
                let _ = done.try_send(Err(ProvisionError::NotProvisionable(format!(
                    "{id} is not in the catalog"
                ))));
            }
        }
        async move {
            rx.recv()
                .await
                .unwrap_or_else(|_| Err(ProvisionError::Other("provisioning stopped".into())))
        }
    }

    pub fn cancel_provision(&self, model: &ModelRef) {
        self.dory.provisions.cancel(model);
    }
}

/// Repairs what a crash left: seals open takes and lists them, deletes audio whose recording
/// was deleted, and requeues jobs that were running. Each step reads the current state, so
/// running it again finds nothing to do.
fn recover(
    db: &Db,
    takes: &dyn Takes,
    clock: &dyn Clock,
    report: &mut Recovery,
) -> Result<(), OpenError> {
    for id in takes.orphans()? {
        if db.store.recording(&id)?.is_none() {
            match takes.delete(&id) {
                Ok(()) => report.deleted_orphans.push(id),
                Err(error) => report.problems.push((id, error.to_string())),
            }
            continue;
        }
        match takes.seal(&id) {
            Ok(info) => {
                commit_sealed(db, &id, info, clock)?;
                report.sealed.push(id);
            }
            Err(error) => report.problems.push((id, error.to_string())),
        }
    }

    let rows = db.store.recordings(&RecordingQuery::default())?;
    for row in &rows {
        let id = &row.recording.id;
        // Sealed but not recorded as sealed: the crash came between the two.
        if row.recording.take.is_none()
            && let Some(info) = takes.inspect(id)?
            && info.container != TakeContainer::Wav
        {
            commit_sealed(db, id, info, clock)?;
            report.sealed.push(id.clone());
        }
    }
    for job in pipeline::jobs_where(db, |state| matches!(state, JobState::Running { .. }))? {
        db.apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Interrupted,
        }])?;
        report.interrupted.push(job.id);
    }
    Ok(())
}
