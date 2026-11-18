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
    AssistantAccess, AssistantChange, AssistantClient, AssistantQuery, Channel, ClientKey, Fields,
    Goal, Job, JobEvent, JobId, LangTag, LanguagePref, Priority, Recording, RecordingEdit,
    RecordingId, RecordingQuery, RecordingRow, Revision, Settings, SpeakerNames, Summary,
    SummaryTranslation, TimeRange, Timestamp, Transcript, Translation,
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
    /// Whether assistants may read, and the assistants the user added.
    fn assistant_access(&self) -> Result<AssistantAccess, StoreError>;
    /// The assistant that holds `key`, if any.
    fn assistant_client(&self, key: &ClientKey) -> Result<Option<AssistantClient>, StoreError>;
    /// Whether the user hid this recording from every assistant. False for an unknown id.
    fn hidden_from_assistants(&self, id: &RecordingId) -> Result<bool, StoreError>;
    /// Newest first, as [`StoreRead::search`] matches but only inside the query's fields, and
    /// never a recording hidden from assistants. `in_title` is false for a listing.
    fn assistant_recordings(&self, query: &AssistantQuery) -> Result<Vec<Found>, StoreError>;
    /// The words [`StoreRead::assistant_recordings`] can find with `fields`: only from those
    /// fields, and never from a recording hidden from assistants, so a correction drawn from it
    /// names no word the assistant could not read.
    fn assistant_vocabulary(&self, fields: Fields) -> Result<Vocabulary, StoreError>;
    /// The summary the last replacement pushed aside, kept so the user can restore it.
    fn previous_summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError>;
    /// What the user asked the summary to focus on, if anything.
    fn summary_instruction(&self, id: &RecordingId) -> Result<Option<String>, StoreError>;
    /// The transcript written in the output language, if the recording has one. It may belong to
    /// an older transcript revision: check [`Translation::applies_to`] before using it.
    fn translation(&self, id: &RecordingId) -> Result<Option<Translation>, StoreError>;
    /// The segments one translation run has finished, in the order they were saved.
    fn translation_batches(
        &self,
        key: &TranslationKey,
    ) -> Result<Vec<TranslationBatch>, StoreError>;
    /// The summary translated on request. Any translation read describes the summary now
    /// stored: the store drops it whenever the summary is written or restored.
    fn summary_translation(
        &self,
        id: &RecordingId,
    ) -> Result<Option<SummaryTranslation>, StoreError>;
    /// The language the user last asked the summary to be translated into, until a translation
    /// into it is stored or the summary changes. The translate-summary stage reads its target here.
    fn summary_translation_wanted(&self, id: &RecordingId) -> Result<Option<LangTag>, StoreError>;
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
    /// Keeps the summary it replaces as the one previous summary, and drops the summary's
    /// translation and any wish for one.
    SummaryReplaced {
        id: RecordingId,
        summary: Summary,
    },
    /// Swaps the current and the previous summary, so restoring twice undoes itself. Like
    /// `SummaryReplaced`, it drops the summary's translation and any wish for one.
    SummaryRestored(RecordingId),
    /// Records the language the user asked the summary to be translated into, replacing an
    /// earlier wish. A translation already stored stays until the new one replaces it.
    SummaryTranslationWanted {
        id: RecordingId,
        target: LangTag,
    },
    /// Stores the translation and ends the wish. It applies only while a wish for its target
    /// stands: a summary written meanwhile took the wish with it, and this change then does
    /// nothing, so a translation never sits under a summary it was not made from.
    SummaryTranslated {
        id: RecordingId,
        translation: SummaryTranslation,
    },
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
    /// Keeps the spoken transcript as it is and stores this beside it, replacing the recording's
    /// earlier translation, whatever revision or target that was for.
    TranslationReplaced {
        id: RecordingId,
        translation: Translation,
    },
    /// Appends to the batches of the same run.
    TranslationBatchSaved {
        key: TranslationKey,
        batch: TranslationBatch,
    },
    /// Every batch of the recording, whatever run made it. Committed with the translation they
    /// built.
    TranslationBatchesCleared(RecordingId),
}

/// The translation run a saved batch belongs to. A rerun reuses a batch only when every field
/// matches, so a new transcript revision, another target, or another model starts over.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TranslationKey {
    pub recording: RecordingId,
    pub revision: Revision,
    pub target: LangTag,
    pub model: ModelRef,
}

/// What one finished batch of a translation run produced, so a rerun after a quit skips it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationBatch {
    /// `(segment index, translated text)`.
    pub texts: Vec<(usize, String)>,
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
