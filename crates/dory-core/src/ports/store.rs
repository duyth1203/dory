//! Persistence of every fact except audio bytes.
//!
//! One write method. A stage's output and its job transition commit together, so a crash never
//! leaves a transcript written while the job still claims to be running. Adding a persisted fact
//! means adding a [`Change`] variant, and the compiler then points at every adapter that must
//! handle it. Adapters apply `Change::Job` with [`crate::domain::Job::on`] instead of
//! re-implementing the state machine.

use super::StoreError;
use crate::domain::{
    Goal, Job, JobEvent, JobId, Priority, Recording, RecordingEdit, RecordingId, RecordingQuery,
    RecordingRow, Settings, SpeakerNames, Summary, Timestamp, Transcript,
};

/// Reads are cheap and safe from any thread, including a UI thread.
pub trait StoreRead: Send + Sync {
    fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError>;
    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError>;
    fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError>;
    fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError>;
    /// Empty names for a recording that has none.
    fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError>;
    fn job(&self, id: JobId) -> Result<Option<Job>, StoreError>;
    fn jobs_for(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError>;
    /// The next Queued job with `not_before <= now`: Front priority first, then oldest.
    fn next_runnable_job(&self, now: Timestamp) -> Result<Option<Job>, StoreError>;
    /// The earliest `not_before` among Queued jobs still in the future, so the worker can sleep
    /// exactly that long.
    fn next_wakeup(&self, now: Timestamp) -> Result<Option<Timestamp>, StoreError>;
    fn settings(&self) -> Result<Settings, StoreError>;
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
}
