//! Persistence of every fact except audio bytes.
//!
//! One write method. A stage's output and its job transition commit together, so a crash never
//! leaves a transcript written while the job still claims to be running. Adding a persisted fact
//! means adding a [`Change`] variant, and the compiler then points at every adapter that must
//! handle it. Adapters apply `Change::Job` with [`crate::domain::Job::on`] instead of
//! re-implementing the state machine.

use std::sync::Arc;

use super::{RawSegment, StoreError};
use crate::domain::{
    Channel, Goal, Job, JobEvent, JobId, LangTag, LanguagePref, Priority, Recording, RecordingEdit,
    RecordingId, RecordingQuery, RecordingRow, Settings, SpeakerNames, Summary, TimeRange,
    Timestamp, Transcript,
};
use crate::models::ModelRef;
use crate::search::{Found, Query, Vocabulary};

/// Reads are cheap and safe from any thread, including a UI thread.
pub trait StoreRead: Send + Sync {
    fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError>;
    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError>;
    /// Recordings where every clause matches the title, a speaker name of the current transcript
    /// revision, the summary, or the transcript, as [`crate::search::matches`] decides. Newest
    /// first, ties by id, unranked. An adapter keeps its index in the same commit as the text.
    fn search(&self, query: &Query) -> Result<Vec<Found>, StoreError>;
    /// Every word the search finds, for [`Query::corrected`]. An adapter may serve a cached copy
    /// as long as it rebuilds it after any write that changed searchable text.
    fn vocabulary(&self) -> Result<Arc<Vocabulary>, StoreError>;
    fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError>;
    fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError>;
    /// Empty names for a recording that has none.
    fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError>;
    fn job(&self, id: JobId) -> Result<Option<Job>, StoreError>;
    fn jobs_for(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError>;
    /// The next Queued job with `not_before <= now`: Front priority first, then lowest `place`
    /// (the enqueue time until the user reorders), then lowest id.
    fn next_runnable_job(&self, now: Timestamp) -> Result<Option<Job>, StoreError>;
    /// The earliest `not_before` among Queued jobs still in the future, so the worker can sleep
    /// exactly that long.
    fn next_wakeup(&self, now: Timestamp) -> Result<Option<Timestamp>, StoreError>;
    fn settings(&self) -> Result<Settings, StoreError>;
    /// The finished windows of one transcription run, earliest first.
    fn checkpoints(&self, key: &CheckpointKey) -> Result<Vec<Checkpoint>, StoreError>;
}

pub trait Store: StoreRead {
    /// All or nothing.
    fn apply(&self, changes: Vec<Change>) -> Result<(), StoreError>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    RecordingCreated(Recording),
    RecordingEdited {
        id: RecordingId,
        edit: RecordingEdit,
    },
    /// Cascades to the transcript, summary, names, and jobs. The caller deletes audio after this
    /// commits, so a crash in between leaves orphan files that startup sweeps.
    RecordingDeleted(RecordingId),
    TranscriptReplaced {
        id: RecordingId,
        transcript: Transcript,
    },
    SpeakerNamesReplaced {
        id: RecordingId,
        names: SpeakerNames,
    },
    SummaryReplaced {
        id: RecordingId,
        summary: Summary,
    },
    /// The store assigns the job id.
    JobEnqueued {
        id: RecordingId,
        goal: Goal,
        priority: Priority,
        at: Timestamp,
    },
    Job {
        job: JobId,
        event: JobEvent,
    },
    SettingsReplaced(Settings),
    /// Replaces the checkpoint of the same run and span.
    CheckpointSaved {
        key: CheckpointKey,
        checkpoint: Checkpoint,
    },
    /// Every checkpoint of the recording, whatever run made it. Committed with the transcript
    /// they built.
    CheckpointsCleared(RecordingId),
}

/// The transcription run a checkpoint belongs to. A rerun reuses a checkpoint only when every
/// field matches, so switching the model or the language starts over.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CheckpointKey {
    pub recording: RecordingId,
    pub model: ModelRef,
    pub channel: Channel,
    pub language: LanguagePref,
}

/// What one finished speech window produced, so a rerun after a crash skips it.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// The window on the take's time axis.
    pub span: TimeRange,
    /// On the take's time axis, not the window's.
    pub segments: Vec<RawSegment>,
    pub detected_language: Option<LangTag>,
}
