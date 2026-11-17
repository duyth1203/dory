//! Persistence of every fact except audio bytes.
//!
//! One write method. A stage's output and its job transition commit together, so a crash never
//! leaves a transcript written while the job still claims to be running. Adding a persisted fact
//! means adding a [`Change`] variant, and the compiler then points at every adapter that must
//! handle it. Adapters apply `Change::Job` with [`crate::domain::Job::on`] instead of
//! re-implementing the state machine.

use super::{RawSegment, StoreError};
use crate::domain::{
    AssistantAccess, AssistantChange, AssistantClient, AssistantQuery, Channel, ClientKey, Goal,
    Job, JobEvent, JobId, LangTag, LanguagePref, Priority, Recording, RecordingEdit, RecordingId,
    RecordingQuery, RecordingRow, Settings, SpeakerNames, Summary, TimeRange, Timestamp,
    Transcript,
};
use crate::models::ModelRef;

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
    /// The finished windows of one transcription run, earliest first.
    fn checkpoints(&self, key: &CheckpointKey) -> Result<Vec<Checkpoint>, StoreError>;
    /// Whether assistants may read, and the assistants the user added.
    fn assistant_access(&self) -> Result<AssistantAccess, StoreError>;
    /// The assistant that holds `key`, if any.
    fn assistant_client(&self, key: &ClientKey) -> Result<Option<AssistantClient>, StoreError>;
    /// Whether the user hid this recording from every assistant. False for an unknown id.
    fn hidden_from_assistants(&self, id: &RecordingId) -> Result<bool, StoreError>;
    /// Newest first, matching only inside the query's fields and never a hidden recording.
    fn assistant_recordings(&self, query: &AssistantQuery)
    -> Result<Vec<RecordingRow>, StoreError>;
    /// The summary the last replacement pushed aside, kept so the user can restore it.
    fn previous_summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError>;
    /// What the user asked the summary to focus on, if anything.
    fn summary_instruction(&self, id: &RecordingId) -> Result<Option<String>, StoreError>;
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
    /// Keeps the summary it replaces as the one previous summary.
    SummaryReplaced {
        id: RecordingId,
        summary: Summary,
    },
    /// Swaps the current and the previous summary, so restoring twice undoes itself.
    SummaryRestored(RecordingId),
    /// `None` clears it. Used by every later summary of the recording.
    SummaryInstructionSet {
        id: RecordingId,
        instruction: Option<String>,
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
    Assistant(AssistantChange),
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
