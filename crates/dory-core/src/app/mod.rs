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
//!
//! [`LibraryReader`] is the read side alone: no actors, no recovery, and no writes, so it can run
//! beside a process that is recording into the same data folder.

mod assistant_host;
mod assistants;
mod events;
mod models;
mod pace;
mod pipeline;
mod recorder;
pub mod stages;
mod writer;

use std::any::Any;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use thiserror::Error;

pub use assistant_host::{
    AssistantHost, CONSENT_WAIT, ChannelConsent, ConsentRequest, consent_channel,
};
pub use assistants::Assistants;
pub use events::{Event, Subscription, change_events};
pub use models::{AppKind, ModelRow};
pub use pace::{Budget, HOLD_MS, LIGHT_THREADS, LightReason, Pacer, light_condition};
pub use recorder::{RecorderGone, RecorderStatus, StartOptions};

use events::{Db, EventHub};
use models::{Provisions, Registries};
use pipeline::{Nudge, PipelineHandle};
use recorder::{Msg, RecorderHandle, commit_sealed};

use crate::domain::{
    AssistantAccess, Goal, InvalidInput, Job, JobEvent, JobId, JobState, LanguagePref, Priority,
    Processing, Recording, RecordingEdit, RecordingId, RecordingQuery, RecordingRow, Settings,
    SpeakerId, SpeakerNames, Summary, TakeContainer, Transcript, Translation, Waveform,
};
use crate::models::{CatalogError, Choice, Diar, KindTag, Llm, ModelId, ModelRef, Stt};
use crate::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, InputDevice, Source,
};
use crate::ports::{
    Change, Clock, Machine, ModelProvider, Playback, PlaybackError, PlaybackSource, Player,
    ProvisionError, Store, StoreError, StoreRead, TakeError, Takes, Trash, TrashError,
    TrashReceipt,
};
use crate::recorder::{Input, Limits};
use crate::search::{self, Correction, Excerpt, Query, SearchResults, Vocabulary};

/// Every adapter the core needs. Only the composition root builds this.
pub struct Ports {
    pub store: Arc<dyn Store>,
    pub takes: Arc<dyn Takes>,
    pub trash: Arc<dyn Trash>,
    pub capture: Arc<dyn CaptureBackend>,
    pub playback: Arc<dyn Playback>,
    pub clock: Arc<dyn Clock>,
    pub machine: Arc<dyn Machine>,
    pub stt: Vec<Arc<dyn ModelProvider<Stt>>>,
    pub llm: Vec<Arc<dyn ModelProvider<Llm>>>,
    pub diar: Vec<Arc<dyn ModelProvider<Diar>>>,
    /// Kept until `Dory` has shut down, then dropped. The composition root puts its exclusive
    /// lock on the data folder here, so no second process can open the folder and run recovery
    /// on a take this one is still writing or sealing.
    pub guard: Option<Box<dyn Any + Send + Sync>>,
}

/// What [`LibraryReader`] needs: store reads and the model providers, nothing that writes.
pub struct ReadPorts {
    pub store: Arc<dyn StoreRead>,
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
    /// Overrides `Settings::processing` for this process, for benches.
    pub processing: Option<Processing>,
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
            processing: None,
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
    Playback(#[from] PlaybackError),
    #[error(transparent)]
    Take(#[from] TakeError),
    #[error(transparent)]
    Invalid(#[from] InvalidInput),
    #[error(transparent)]
    Trash(#[from] TrashError),
    #[error("{0} is already in the library")]
    Exists(RecordingId),
    #[error("{0} is being recorded")]
    Recording(RecordingId),
    #[error("{0} is still being saved, try again in a moment")]
    Saving(RecordingId),
    #[error(transparent)]
    RecorderGone(#[from] RecorderGone),
    #[error("an assistant named {0} already exists")]
    AssistantExists(String),
}

/// A recording in the system trash, with everything [`Library::restore`] needs to bring it back.
/// It lives in memory only, so undo works until the process ends. Apps see its id and title.
#[derive(Debug)]
pub struct Trashed {
    recording: Recording,
    transcript: Option<Transcript>,
    summary: Option<Summary>,
    translation: Option<Translation>,
    names: SpeakerNames,
    receipt: TrashReceipt,
}

impl Trashed {
    pub fn id(&self) -> &RecordingId {
        &self.recording.id
    }

    pub fn title(&self) -> Option<&str> {
        self.recording.title.as_deref()
    }
}

/// An audio file of the old app that a recording still reads in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Original {
    pub id: RecordingId,
    pub path: PathBuf,
    pub bytes: u64,
}

/// Old-app originals in the system trash, with what [`Library::restore_originals`] needs to put
/// them back. It lives in memory only, so undo works until the process ends.
#[derive(Debug)]
pub struct TrashedOriginals {
    receipt: TrashReceipt,
    count: usize,
    bytes: u64,
    skipped: usize,
}

impl TrashedOriginals {
    /// Files moved to the trash.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Their total size.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Recordings left alone because a job was queued, running, or blocked on them.
    pub fn skipped(&self) -> usize {
        self.skipped
    }
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
    reader: LibraryReader,
    takes: Arc<dyn Takes>,
    trash: Arc<dyn Trash>,
    playback: Arc<dyn Playback>,
    capture: Arc<dyn CaptureBackend>,
    clock: Arc<dyn Clock>,
    models: Arc<Registries>,
    provisions: Arc<Provisions>,
    recorder: RecorderHandle,
    pipeline: PipelineHandle,
    recovery: Recovery,
    /// Serializes settings read-modify-write, so two edits never lose one another.
    settings_edit: Mutex<()>,
    /// Dropped after `Drop::drop` has shut everything down.
    _guard: Option<Box<dyn Any + Send + Sync>>,
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
        // The recorder starts first, so the worker can see whether it is busy.
        let recorder = recorder::spawn(
            recorder::Deps {
                db: db.clone(),
                takes: Arc::clone(&ports.takes),
                capture: Arc::clone(&ports.capture),
                clock: Arc::clone(&ports.clock),
                nudge: nudge.clone(),
            },
            &options,
        )?;
        let pipeline = match pipeline::spawn(
            pipeline::Deps {
                db: db.clone(),
                takes: Arc::clone(&ports.takes),
                clock: Arc::clone(&ports.clock),
                models: Arc::clone(&models),
                machine: ports.machine,
                recorder: recorder.watch(),
            },
            &options,
            nudge,
            wake,
        ) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                recorder.shutdown();
                return Err(error.into());
            }
        };
        Ok(Dory {
            reader: LibraryReader {
                store: db.store.clone(),
                models: Arc::clone(&models),
            },
            db,
            takes: ports.takes,
            trash: ports.trash,
            playback: ports.playback,
            capture: ports.capture,
            clock: ports.clock,
            models,
            provisions: Arc::new(Provisions::default()),
            recorder,
            pipeline,
            recovery,
            settings_edit: Mutex::new(()),
            _guard: ports.guard,
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

    /// Which assistants may read the library, and how much.
    pub fn assistants(&self) -> Assistants<'_> {
        Assistants { dory: self }
    }

    pub fn settings(&self) -> Result<Settings, StoreError> {
        self.db.store.settings()
    }

    /// Reads the settings, applies `edit`, validates, and saves, with no other edit in between.
    /// Blocked jobs are tried again, since the change may fix them.
    pub fn update_settings(&self, edit: impl FnOnce(&mut Settings)) -> Result<(), DoryError> {
        let _one_at_a_time = self
            .settings_edit
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut settings = self.db.store.settings()?;
        edit(&mut settings);
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
/// caller's executor. If the thread cannot start or panics, it resolves with `lost()` instead.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    lost: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = T> + Send + 'static {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new()
        .name("dory-probe".into())
        .spawn(move || {
            let _ = tx.send_blocking(work());
        });
    async move { rx.recv().await.unwrap_or_else(|_| lost()) }
}

const PROBE_LOST: &str = "the background probe stopped before it answered";

fn capture_lost() -> CaptureError {
    CaptureError::Platform {
        message: PROBE_LOST.into(),
        source: None,
    }
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

    pub fn probe(
        &self,
    ) -> impl Future<Output = Result<CaptureCaps, CaptureError>> + Send + 'static {
        let capture = Arc::clone(&self.dory.capture);
        off_thread(move || Ok(capture.probe()), || Err(capture_lost()))
    }

    pub fn input_devices(
        &self,
    ) -> impl Future<Output = Result<Vec<InputDevice>, CaptureError>> + Send + 'static {
        let capture = Arc::clone(&self.dory.capture);
        off_thread(move || capture.input_devices(), || Err(capture_lost()))
    }

    /// Shows the OS prompt if needed and resolves once the user answers.
    pub fn request_access(
        &self,
        source: Source,
    ) -> impl Future<Output = Result<Access, CaptureError>> + Send + 'static {
        let capture = Arc::clone(&self.dory.capture);
        off_thread(
            move || capture.request_access(source),
            || Err(capture_lost()),
        )
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
        self.dory.reader.list(query)
    }

    /// See [`LibraryReader::search`].
    pub fn search(&self, text: &str) -> Result<Option<SearchResults>, StoreError> {
        self.dory.reader.search(text)
    }

    /// See [`LibraryReader::snippets`].
    pub fn snippets(
        &self,
        query: &Query,
        ids: &[RecordingId],
    ) -> Result<Vec<Option<Excerpt>>, StoreError> {
        self.dory.reader.snippets(query, ids)
    }

    pub fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        self.dory.reader.recording(id)
    }

    pub fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError> {
        self.dory.reader.transcript(id)
    }

    pub fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.dory.reader.summary(id)
    }

    /// The transcript in the output language, if the recording has one. Check
    /// [`Translation::applies_to`] against the transcript before showing it.
    pub fn translation(&self, id: &RecordingId) -> Result<Option<Translation>, StoreError> {
        self.dory.reader.translation(id)
    }

    pub fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError> {
        self.dory.reader.speaker_names(id)
    }

    pub fn jobs(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError> {
        self.dory.reader.jobs(id)
    }

    pub fn set_title(&self, id: &RecordingId, title: Option<String>) -> Result<(), DoryError> {
        self.edit(id, RecordingEdit::Title(title))
    }

    /// Swaps the current summary with the one it replaced. Doing it again undoes it.
    pub fn restore_previous_summary(&self, id: &RecordingId) -> Result<(), DoryError> {
        self.dory
            .db
            .apply(vec![Change::SummaryRestored(id.clone())])?;
        Ok(())
    }

    /// Hides the recording from every assistant, or shows it again.
    pub fn set_hidden(&self, id: &RecordingId, hidden: bool) -> Result<(), DoryError> {
        assistants::set_hidden(self.dory, id, hidden)
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

    /// Deletes the recording and its audio. Refused while it is being recorded or sealed, so a
    /// seal never renames audio into place after its row is gone.
    pub fn delete(&self, id: &RecordingId) -> Result<(), DoryError> {
        self.dory.recorder.check_not_busy(id)?;
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

    /// Moves the recording to the system trash and returns what [`Library::restore`] needs to
    /// bring it back. Refused while it is being recorded or sealed, like [`Library::delete`].
    ///
    /// Queued, running, and blocked jobs are cancelled first, so no stage output lands after the
    /// transcript and summary are read. A restored recording has no job: enqueue one to process
    /// it again. Checkpoints of an unfinished transcription are dropped, not restored.
    ///
    /// The files move before the row goes. A crash in between leaves a row whose audio is in the
    /// trash, which the user can put back by hand, never audio that startup sweeps away.
    pub fn trash(&self, id: &RecordingId) -> Result<Trashed, DoryError> {
        let dory = self.dory;
        dory.recorder.check_not_busy(id)?;
        let store = &dory.db.store;
        let recording = store
            .recording(id)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        for job in store.jobs_for(id)?.into_iter().filter(|j| !j.is_terminal()) {
            match dory.pipeline().cancel(job.id) {
                // The job finished between the read and the cancel, which is as good.
                Ok(()) | Err(DoryError::Store(StoreError::IllegalTransition(_))) => {}
                Err(error) => return Err(error),
            }
        }
        let transcript = store.transcript(id)?;
        let summary = store.summary(id)?;
        let translation = store.translation(id)?;
        let names = store.speaker_names(id)?;
        let receipt = dory.trash.move_to_trash(&dory.takes.files(id)?)?;
        if let Err(error) = dory.db.apply(vec![Change::RecordingDeleted(id.clone())]) {
            let _ = dory.trash.put_back(&receipt);
            return Err(error.into());
        }
        Ok(Trashed {
            recording,
            transcript,
            summary,
            translation,
            names,
            receipt,
        })
    }

    /// Undoes [`Library::trash`]. The files go back first, then the row, the transcript, the
    /// summary, and the speaker names return in one commit.
    ///
    /// A crash between the two steps leaves the audio in place with no row, and the next start
    /// deletes it: startup removes every file whose recording is gone. The recording is then
    /// lost for good. The window is the time of one store commit. If the commit fails without a
    /// crash, the files go back to the trash, and the error says the restore failed.
    ///
    /// The trashed copy is spent whatever happens. Fails with [`DoryError::Exists`] if the id is
    /// in the library again, and with a [`TrashError`] if the files left the trash or their old
    /// place is taken. Nothing has moved in either case.
    pub fn restore(&self, trashed: Trashed) -> Result<RecordingId, DoryError> {
        let dory = self.dory;
        let Trashed {
            recording,
            transcript,
            summary,
            translation,
            names,
            receipt,
        } = trashed;
        let id = recording.id.clone();
        if dory.db.store.recording(&id)?.is_some() {
            return Err(DoryError::Exists(id));
        }
        dory.trash.put_back(&receipt)?;
        let mut changes = vec![Change::RecordingCreated(recording)];
        if let Some(transcript) = transcript {
            changes.push(Change::TranscriptReplaced {
                id: id.clone(),
                transcript,
            });
        }
        if let Some(summary) = summary {
            changes.push(Change::SummaryReplaced {
                id: id.clone(),
                summary,
            });
        }
        if let Some(translation) = translation {
            changes.push(Change::TranslationReplaced {
                id: id.clone(),
                translation,
            });
        }
        if names != SpeakerNames::default() {
            changes.push(Change::SpeakerNamesReplaced {
                id: id.clone(),
                names,
            });
        }
        if let Err(error) = dory.db.apply(changes) {
            if let Ok(files) = dory.takes.files(&id) {
                let _ = dory.trash.move_to_trash(&files);
            }
            return Err(error.into());
        }
        Ok(id)
    }

    /// The old app's audio files that recordings still read in place, and how big they are.
    /// A recording whose original is gone is not listed.
    pub fn originals(&self) -> Result<Vec<Original>, DoryError> {
        let mut found = Vec::new();
        for id in self.legacy_ids()? {
            if let Some(original) = self.original_of(id)? {
                found.push(original);
            }
        }
        Ok(found)
    }

    /// Copies every original in as Dory's own FLAC take, then moves the originals to the system
    /// trash in one receipt. Playback and re-transcription read the copies, so they keep working
    /// whether or not the originals come back.
    ///
    /// A recording with a queued, running, or blocked job is skipped and counted, so a stage
    /// never loses its audio mid-run. Nothing moves to the trash until every other original is
    /// copied, so a failed copy (the converter missing, a full disk) leaves every original where
    /// it was. Copies already made stay: they are real takes and harmless. Their originals are no
    /// longer listed, since the pointer is gone, and stay in the old app's folder.
    ///
    /// The copy and the row edit are two steps. A crash or a store failure between them leaves a
    /// row that still says WebM over a FLAC take. Nothing reads the label to find audio, and the
    /// next call corrects the row. A crash after the copies and before the move leaves the
    /// originals in place and unlisted, like a failed copy does. The trash move is all or
    /// nothing.
    pub fn trash_originals(&self) -> Result<TrashedOriginals, DoryError> {
        let dory = self.dory;
        let (mut paths, mut bytes, mut skipped) = (Vec::new(), 0, 0);
        for id in self.legacy_ids()? {
            let Some(original) = self.original_of(id.clone())? else {
                self.heal_label(&id)?;
                continue;
            };
            let busy = dory
                .db
                .store
                .jobs_for(&id)?
                .iter()
                .any(|job| !job.is_terminal());
            if busy {
                skipped += 1;
                continue;
            }
            let info = dory.takes.adopt(&id)?;
            self.edit(&id, RecordingEdit::Take(info))?;
            bytes += original.bytes;
            paths.push(original.path);
        }
        let receipt = dory.trash.move_to_trash(&paths)?;
        Ok(TrashedOriginals {
            receipt,
            count: paths.len(),
            bytes,
            skipped,
        })
    }

    /// Undoes [`Library::trash_originals`]: the originals go back where they were. The recordings
    /// keep their copies. The trashed set is spent whatever happens.
    ///
    /// Fails with a [`TrashError`] if a file left the trash or its old place is taken. Nothing
    /// has moved in that case.
    pub fn restore_originals(&self, trashed: TrashedOriginals) -> Result<(), DoryError> {
        self.dory.trash.put_back(&trashed.receipt)?;
        Ok(())
    }

    fn legacy_ids(&self) -> Result<Vec<RecordingId>, StoreError> {
        Ok(self
            .dory
            .reader
            .list(&RecordingQuery::default())?
            .into_iter()
            .filter(|row| {
                row.recording
                    .take
                    .is_some_and(|take| take.container == TakeContainer::LegacyWebm)
            })
            .map(|row| row.recording.id)
            .collect())
    }

    fn original_of(&self, id: RecordingId) -> Result<Option<Original>, DoryError> {
        Ok(self.dory.takes.legacy_original(&id)?.and_then(|file| {
            Some(Original {
                id,
                bytes: file.bytes?,
                path: file.path,
            })
        }))
    }

    /// Corrects a row that says WebM over a take that has been copied in.
    fn heal_label(&self, id: &RecordingId) -> Result<(), DoryError> {
        match self.dory.takes.inspect(id)? {
            Some(info) if info.container != TakeContainer::LegacyWebm => {
                self.edit(id, RecordingEdit::Take(info))
            }
            _ => Ok(()),
        }
    }

    /// Adds an audio file as a new recording with one mixed channel. The file is copied in, so
    /// the original can move. Processing waits for an explicit enqueue.
    pub fn import_audio(
        &self,
        source: &std::path::Path,
        title: Option<String>,
    ) -> Result<RecordingId, DoryError> {
        let now = self.dory.clock.now();
        let settings = self.dory.reader.store.settings()?;
        // The take's files are claimed first, so a recording started in the same second, or a
        // crash before the row commits, never shares or strands an id.
        let (id, info) = (0..=u16::MAX)
            .find_map(|suffix| {
                let id = RecordingId::generate(now, suffix);
                match self.dory.takes.import(&id, source) {
                    Err(TakeError::Exists(_)) => None,
                    result => Some(result.map(|info| (id, info))),
                }
            })
            .ok_or_else(|| StoreError::NotFound("a free recording id".into()))??;
        let created = self.dory.db.apply(vec![Change::RecordingCreated(Recording {
            id: id.clone(),
            title,
            created_at: now,
            language: settings.language,
            layout: crate::domain::ChannelLayout::Mixed,
            take: Some(info),
        })]);
        if let Err(error) = created {
            let _ = self.dory.takes.delete(&id);
            return Err(error.into());
        }
        Ok(id)
    }

    pub fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        self.dory.takes.playback_path(id)
    }

    /// A paused player for the recording's sealed audio. Refused while it is being recorded or
    /// sealed, because the file is still changing.
    pub fn player(&self, id: &RecordingId) -> Result<Box<dyn Player>, DoryError> {
        self.dory.recorder.check_not_busy(id)?;
        let recording = self
            .dory
            .reader
            .recording(id)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let take = recording
            .take
            .ok_or_else(|| TakeError::Missing(id.clone()))?;
        let source = PlaybackSource {
            path: self.dory.takes.playback_path(id)?,
            layout: recording.layout,
            duration: take.duration,
        };
        Ok(self.dory.playback.open(&source)?)
    }

    /// The take's loudness over time as bar heights for the player. Blocking: the first call on
    /// a take whose levels are not kept yet reads the whole take. Refused while it is being
    /// recorded or sealed, because the file is still changing.
    pub fn waveform(&self, id: &RecordingId) -> Result<Waveform, DoryError> {
        self.dory.recorder.check_not_busy(id)?;
        Ok(Waveform::from_levels(&self.dory.takes.levels(id)?))
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

    /// Moves a queued job to index `to` among the queued jobs, as [`Pipeline::queue`] lists them
    /// (the running job is not counted, and `to` past the end means the last place). Every job
    /// that changes place commits in one apply, so a reader never sees half a reorder. A job
    /// that is not queued (running, blocked, finished, or unknown) is left alone and this
    /// returns `Ok`, so a move that races the worker is not an error.
    pub fn move_job(&self, job: JobId, to: usize) -> Result<(), DoryError> {
        let queued: Vec<Job> = self
            .queue()?
            .into_iter()
            .filter(|j| j.state == JobState::Queued)
            .collect();
        let changes: Vec<Change> = pipeline::placed(&queued, job, to)
            .into_iter()
            .map(|(job, place)| Change::Job {
                job,
                event: JobEvent::Place { place },
            })
            .collect();
        if changes.is_empty() {
            return Ok(());
        }
        self.dory.db.apply(changes)?;
        self.dory.pipeline.nudge.wake();
        Ok(())
    }

    /// Use Fast This Time (UX-30): runs this one job fast from its next speech window, whatever
    /// the setting and the Mac's conditions, without changing the setting.
    pub fn run_fast(&self, job: JobId) -> Result<(), DoryError> {
        self.event(job, JobEvent::RunFast)
    }

    pub fn job(&self, job: JobId) -> Result<Option<Job>, StoreError> {
        self.dory.db.store.job(job)
    }

    /// Every job that is running, queued, or blocked, in the order the queue shows them: the
    /// running job, then queued jobs as the worker takes them, then blocked ones.
    pub fn queue(&self) -> Result<Vec<Job>, StoreError> {
        let mut jobs = pipeline::jobs_where(&self.dory.db, |state| {
            matches!(
                state,
                JobState::Queued | JobState::Running { .. } | JobState::Blocked { .. }
            )
        })?;
        pipeline::queue_order(&mut jobs);
        Ok(jobs)
    }

    fn enqueue_with(
        &self,
        id: &RecordingId,
        goal: Goal,
        priority: Priority,
    ) -> Result<(), DoryError> {
        let goal = self.dory.db.store.settings()?.goal_for(goal);
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
        self.dory.reader.models::<K>()
    }

    pub fn choose<K: AppKind>(&self, choice: Choice<K>) -> Result<(), DoryError> {
        self.dory
            .update_settings(|settings| K::set_choice(settings, choice))
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

/// Runs `query` exactly through `search`. Only when that finds nothing does it read `vocabulary`
/// and run the corrected query. A correction that finds nothing either is dropped, so the results
/// never name a correction that shows no recording. Returns the query that found the hits.
pub(crate) fn find<T>(
    query: Query,
    search: impl Fn(&Query) -> Result<Vec<T>, StoreError>,
    vocabulary: impl FnOnce() -> Result<Arc<Vocabulary>, StoreError>,
) -> Result<(Query, Vec<T>, Vec<Correction>), StoreError> {
    let hits = search(&query)?;
    if hits.is_empty()
        && query.is_correctable()
        && let Some((corrected, corrections)) = query.corrected(&*vocabulary()?)
    {
        let hits = search(&corrected)?;
        if !hits.is_empty() {
            return Ok((corrected, hits, corrections));
        }
    }
    Ok((query, hits, Vec::new()))
}

/// What each speaker said in one go: consecutive segments of one speaker, joined by a space.
fn turn_texts(transcript: &Transcript) -> Vec<String> {
    let mut turns: Vec<(SpeakerId, String)> = Vec::new();
    for segment in &transcript.segments {
        match turns.last_mut() {
            Some((speaker, text)) if *speaker == segment.speaker => {
                text.push(' ');
                text.push_str(&segment.text);
            }
            _ => turns.push((segment.speaker, segment.text.clone())),
        }
    }
    turns.into_iter().map(|(_, text)| text).collect()
}

/// The read side of a data folder: the library and the model list. It starts no actors, repairs
/// nothing, and writes nothing, so it is safe beside a process recording into the same folder.
pub struct LibraryReader {
    store: Arc<dyn StoreRead>,
    models: Arc<Registries>,
}

impl LibraryReader {
    pub fn open(ports: ReadPorts) -> Result<LibraryReader, CatalogError> {
        Ok(LibraryReader {
            store: ports.store,
            models: Arc::new(Registries::new(ports.stt, ports.llm, ports.diar)?),
        })
    }

    pub fn list(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError> {
        self.store.recordings(query)
    }

    /// The recordings `text` finds, newest first, and the query that found them. When nothing
    /// matches what was typed, each long word that appears nowhere in the recordings is replaced
    /// by the words one typo away, and the results say which. `None` when the text holds no word,
    /// such as blank text or punctuation alone.
    pub fn search(&self, text: &str) -> Result<Option<SearchResults>, StoreError> {
        let Some(query) = Query::parse(text) else {
            return Ok(None);
        };
        let started = std::time::Instant::now();
        let (query, hits, corrections) =
            find(query, |q| self.store.search(q), || self.store.vocabulary())?;
        let results = SearchResults {
            query,
            hits,
            corrections,
        };
        eprintln!(
            "search ms={:.2} hits={} corrected={}",
            started.elapsed().as_secs_f64() * 1000.0,
            results.hits.len(),
            results.corrections.len()
        );
        Ok(Some(results))
    }

    /// The line to show under each of `ids`, a hit of `query`, in the same order: where in the
    /// transcript it first matches, else in the summary, else in a speaker name. `None` for a
    /// recording that has been deleted since the search, or that holds no word of the query.
    pub fn snippets(
        &self,
        query: &Query,
        ids: &[RecordingId],
    ) -> Result<Vec<Option<Excerpt>>, StoreError> {
        ids.iter()
            .map(|id| {
                let transcript = self.store.transcript(id)?;
                let names = self.store.speaker_names(id)?;
                let summary = self.store.summary(id)?;
                let turns = transcript.as_ref().map(turn_texts).unwrap_or_default();
                let speakers: Vec<&str> = transcript.as_ref().map_or_else(Vec::new, |t| {
                    t.speakers()
                        .into_iter()
                        .filter_map(|speaker| names.name_of(speaker, t.revision))
                        .collect()
                });
                let blocks = summary
                    .as_ref()
                    .map(|s| search::summary_blocks(&s.markdown))
                    .unwrap_or_default();
                let turns: Vec<&str> = turns.iter().map(String::as_str).collect();
                let blocks: Vec<&str> = blocks.iter().map(String::as_str).collect();
                Ok(search::excerpt(query, &turns, &blocks, &speakers))
            })
            .collect()
    }

    pub fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        self.store.recording(id)
    }

    pub fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError> {
        self.store.transcript(id)
    }

    pub fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.store.summary(id)
    }

    pub fn translation(&self, id: &RecordingId) -> Result<Option<Translation>, StoreError> {
        self.store.translation(id)
    }

    pub fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError> {
        self.store.speaker_names(id)
    }

    pub fn jobs(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError> {
        self.store.jobs_for(id)
    }

    /// Whether assistants may read, and the assistants the user added.
    pub fn assistant_access(&self) -> Result<AssistantAccess, StoreError> {
        self.store.assistant_access()
    }

    /// Every model of kind `K`, probed now, with how it fits the default settings.
    pub fn models<K: AppKind>(
        &self,
    ) -> impl Future<Output = Result<Vec<ModelRow<K>>, StoreError>> + Send + 'static {
        let models = Arc::clone(&self.models);
        let store = Arc::clone(&self.store);
        off_thread(
            move || {
                let settings = store.settings()?;
                Ok(models
                    .get::<K>()
                    .rows(K::choice(&settings), &K::listing_needs(&settings)))
            },
            || Err(StoreError::Unavailable(PROBE_LOST.into())),
        )
    }
}

/// Repairs what a crash left: deletes every file whose recording was deleted, seals open takes
/// and lists them, and requeues jobs that were running. Each step reads the current state, so
/// running it again finds nothing to do.
///
/// Only the process that owns the data folder may run this: to anyone else, a take being
/// recorded looks exactly like one a crash left open.
fn recover(
    db: &Db,
    takes: &dyn Takes,
    clock: &dyn Clock,
    report: &mut Recovery,
) -> Result<(), OpenError> {
    for id in takes.stored()? {
        if db.store.recording(&id)?.is_none() {
            match takes.delete(&id) {
                Ok(()) => report.deleted_orphans.push(id),
                Err(error) => report.problems.push((id, error.to_string())),
            }
        }
    }
    for id in takes.orphans()? {
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
