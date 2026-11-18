//! In-memory `Store` and the store contract suite every `Store` adapter must pass.
//!
//! [`MemStore`] applies a change list to a copy of its state and swaps the copy in only when every
//! change succeeded, so `apply` is all or nothing under one mutex. Job events go through
//! [`Job::on`], never a second copy of the transition rules.
//!
//! [`run_store_contract`] pins the behavior the port leaves open, so a SQLite adapter and this fake
//! cannot drift apart:
//!
//! - `RecordingCreated` for an id that exists is a no-op that keeps the stored recording, so a
//!   retried create after a crash is safe.
//! - `RecordingDeleted` for a missing id is a no-op, for the same reason.
//! - Edits, transcript, summary, names, and enqueue for a missing recording fail with `NotFound`.
//! - `JobEnqueued` is idempotent per recording and goal. While a non-terminal job with the same
//!   goal exists, a second enqueue changes nothing, except that a `Front` request promotes it.
//!   Once that job is terminal, an enqueue creates a new job.
//! - Job ids are assigned in increasing order and are never reused, even after a delete.
//! - `recordings` lists newest first (ties broken by id). The row's job is the most recently
//!   enqueued one.
//! - `search` finds what [`crate::search::matches`] finds over the title, the speaker names of the
//!   current transcript revision, the summary, and the transcript, as soon as the change that
//!   wrote the text commits. It lists hits like `recordings`, and `in_title` is true when the
//!   title alone matches every clause. FTS syntax in typed text is never syntax.
//! - `next_runnable_job` is Front first, then lowest `place`, then lowest job id. A job's `place`
//!   starts as its enqueue time and changes only through `JobEvent::Place`.
//! - A translation is stored beside the transcript and replaced whole. A new transcript revision
//!   leaves it in place, still carrying the revision it was made from. Its batches are read per
//!   run (recording, revision, target, model) in save order, and clearing or deleting the
//!   recording drops them. Saving either for a missing recording is `NotFound`.
//! - A summary translation is stored beside the summary, at most one per recording. Asking for one
//!   records a wish and leaves any stored translation alone. Storing a translation applies only
//!   while a wish for its target stands, and ends the wish. Writing or restoring the summary drops
//!   the translation and the wish in the same change, and deleting the recording drops both.
//! - Checkpoints are read per run (recording, model, channel, language), earliest window first. A
//!   second save of the same run and span replaces the first. Clearing drops every run of one
//!   recording, and so does deleting the recording. Saving for a missing recording is `NotFound`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use crate::domain::{
    AssistantAccess, AssistantChange, AssistantClient, AssistantQuery, ClientKey, Fields, Job,
    JobEvent, JobId, JobState, LangTag, Ms, Priority, Recording, RecordingEdit, RecordingId,
    RecordingQuery, RecordingRow, Settings, SpeakerNames, Summary, SummaryFlag, SummaryTranslation,
    Timestamp, Transcript, Translation,
};
use crate::ports::{
    Change, Checkpoint, CheckpointKey, Store, StoreError, StoreRead, TranslationBatch,
    TranslationKey,
};
use crate::search::{self, Found, Query, Vocabulary};

#[derive(Clone, Debug)]
struct Entry {
    recording: Recording,
    transcript: Option<Transcript>,
    summary: Option<Summary>,
    names: SpeakerNames,
    hidden: bool,
    previous: Option<Summary>,
    instruction: Option<String>,
    translation: Option<Translation>,
    summary_translation: Option<SummaryTranslation>,
    summary_translation_wanted: Option<LangTag>,
}

impl Entry {
    /// A summary written or restored takes its translation and any wish for one with it.
    fn drop_summary_translation(&mut self) {
        self.summary_translation = None;
        self.summary_translation_wanted = None;
    }
}

#[derive(Clone, Debug)]
struct State {
    recordings: BTreeMap<RecordingId, Entry>,
    jobs: BTreeMap<JobId, Job>,
    last_job: u64,
    settings: Settings,
    checkpoints: HashMap<CheckpointKey, BTreeMap<(Ms, Ms), Checkpoint>>,
    translation_batches: HashMap<TranslationKey, Vec<TranslationBatch>>,
    assistants_enabled: bool,
    /// By label. The fake keeps keys whole; a real store keeps only a digest.
    assistants: BTreeMap<String, (AssistantClient, ClientKey)>,
}

impl State {
    fn entry_mut(&mut self, id: &RecordingId) -> Result<&mut Entry, StoreError> {
        self.recordings
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(format!("recording {id}")))
    }

    fn apply_one(&mut self, change: Change) -> Result<(), StoreError> {
        match change {
            Change::RecordingCreated(recording) => {
                self.recordings
                    .entry(recording.id.clone())
                    .or_insert_with(|| Entry {
                        recording,
                        transcript: None,
                        summary: None,
                        names: SpeakerNames::default(),
                        hidden: false,
                        previous: None,
                        instruction: None,
                        translation: None,
                        summary_translation: None,
                        summary_translation_wanted: None,
                    });
            }
            Change::RecordingEdited { id, edit } => {
                let entry = self.entry_mut(&id)?;
                match edit {
                    RecordingEdit::Title(title) => entry.recording.title = title,
                    RecordingEdit::Language(language) => entry.recording.language = language,
                    RecordingEdit::Take(take) => entry.recording.take = Some(take),
                }
            }
            Change::RecordingDeleted(id) => {
                self.recordings.remove(&id);
                self.jobs.retain(|_, job| job.recording != id);
                self.checkpoints.retain(|key, _| key.recording != id);
                self.translation_batches
                    .retain(|key, _| key.recording != id);
            }
            Change::TranscriptReplaced { id, transcript } => {
                self.entry_mut(&id)?.transcript = Some(transcript);
            }
            Change::SpeakerNamesReplaced { id, names } => self.entry_mut(&id)?.names = names,
            Change::SummaryReplaced { id, summary } => {
                let entry = self.entry_mut(&id)?;
                entry.previous = entry.summary.replace(summary).or(entry.previous.take());
                entry.drop_summary_translation();
            }
            Change::SummaryRestored(id) => {
                let entry = self.entry_mut(&id)?;
                let previous = entry
                    .previous
                    .take()
                    .ok_or_else(|| StoreError::NotFound(format!("previous summary of {id}")))?;
                entry.previous = entry.summary.replace(previous);
                entry.drop_summary_translation();
            }
            Change::SummaryTranslationWanted { id, target } => {
                self.entry_mut(&id)?.summary_translation_wanted = Some(target);
            }
            Change::SummaryTranslated { id, translation } => {
                let entry = self.entry_mut(&id)?;
                if entry.summary_translation_wanted.as_ref() == Some(&translation.target) {
                    entry.summary_translation_wanted = None;
                    entry.summary_translation = Some(translation);
                }
            }
            Change::SummaryInstructionSet { id, instruction } => {
                self.entry_mut(&id)?.instruction = instruction;
            }
            Change::JobEnqueued {
                id,
                goal,
                priority,
                at,
            } => {
                self.entry_mut(&id)?;
                let existing = self
                    .jobs
                    .values_mut()
                    .find(|j| j.recording == id && j.goal == goal && !j.is_terminal());
                match existing {
                    Some(job) => {
                        if priority == Priority::Front {
                            *job = job.on(&JobEvent::Promote)?;
                        }
                    }
                    None => {
                        self.last_job += 1;
                        let job_id = JobId(self.last_job);
                        self.jobs
                            .insert(job_id, Job::new(job_id, id, goal, priority, at));
                    }
                }
            }
            Change::Job { job, event } => {
                let current = self
                    .jobs
                    .get_mut(&job)
                    .ok_or_else(|| StoreError::NotFound(format!("job {}", job.0)))?;
                *current = current.on(&event)?;
            }
            Change::SettingsReplaced(settings) => self.settings = settings,
            Change::CheckpointSaved { key, checkpoint } => {
                self.entry_mut(&key.recording)?;
                let span = (checkpoint.span.start(), checkpoint.span.end());
                self.checkpoints
                    .entry(key)
                    .or_default()
                    .insert(span, checkpoint);
            }
            Change::CheckpointsCleared(id) => self.checkpoints.retain(|key, _| key.recording != id),
            Change::TranslationReplaced { id, translation } => {
                self.entry_mut(&id)?.translation = Some(translation);
            }
            Change::TranslationBatchSaved { key, batch } => {
                self.entry_mut(&key.recording)?;
                self.translation_batches.entry(key).or_default().push(batch);
            }
            Change::TranslationBatchesCleared(id) => {
                self.translation_batches
                    .retain(|key, _| key.recording != id);
            }
            Change::Assistant(change) => self.apply_assistant(change)?,
        }
        Ok(())
    }

    fn apply_assistant(&mut self, change: AssistantChange) -> Result<(), StoreError> {
        match change {
            AssistantChange::Enabled(on) => self.assistants_enabled = on,
            AssistantChange::ClientAdded { client, key } => {
                let taken = self.assistants.contains_key(&client.label)
                    || self.assistants.values().any(|(_, k)| *k == key);
                if taken {
                    return Err(StoreError::Corrupt(format!(
                        "an assistant named {} or with that key exists",
                        client.label
                    )));
                }
                self.assistants.insert(client.label.clone(), (client, key));
            }
            AssistantChange::ClientChanged(client) => {
                let (current, _) = self
                    .assistants
                    .get_mut(&client.label)
                    .ok_or_else(|| StoreError::NotFound(format!("assistant {}", client.label)))?;
                current.exposure = client.exposure;
                current.writes = client.writes;
            }
            AssistantChange::ClientRemoved { label } => {
                self.assistants
                    .remove(&label)
                    .ok_or_else(|| StoreError::NotFound(format!("assistant {label}")))?;
            }
            AssistantChange::RecordingHidden { id, hidden } => self.entry_mut(&id)?.hidden = hidden,
        }
        Ok(())
    }
}

/// A `Store` that keeps everything in memory. Safe to share across threads.
#[derive(Debug)]
pub struct MemStore {
    state: Mutex<State>,
}

impl MemStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                recordings: BTreeMap::new(),
                jobs: BTreeMap::new(),
                last_job: 0,
                settings: Settings::default(),
                checkpoints: HashMap::new(),
                translation_batches: HashMap::new(),
                assistants_enabled: false,
                assistants: BTreeMap::new(),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while the lock was held cannot leave half a change behind, because `apply`
        // swaps the state in only after every change succeeded.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Entry {
    /// The searchable text: title, speaker names, summary, transcript. Names count only for the
    /// transcript revision they were given for. The summary is what was written, then its
    /// translation. The transcript is what was spoken, then the translated segments of a
    /// translation that applies to it.
    fn fields(&self) -> [String; 4] {
        let revision = self.transcript.as_ref().map(|t| t.revision);
        let names = match revision {
            Some(revision) if self.names.for_revision == revision => self
                .names
                .names
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let translated = self.transcript.as_ref().and_then(|t| {
            self.translation
                .as_ref()
                .filter(|translation| translation.applies_to(t))
        });
        let transcript = self
            .transcript
            .iter()
            .flat_map(|t| t.segments.iter().map(|s| s.text.as_str()))
            .chain(
                translated
                    .into_iter()
                    .flat_map(|t| t.texts.iter().flatten().map(String::as_str)),
            )
            .collect::<Vec<_>>()
            .join("\n");
        [
            self.recording.title.clone().unwrap_or_default(),
            names,
            self.summary
                .iter()
                .map(|s| s.markdown.as_str())
                .chain(self.summary_translation.iter().map(|t| t.markdown.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            transcript,
        ]
    }

    /// [`Entry::fields`] with each field `fields` leaves out emptied, so the title stays first.
    fn granted(&self, fields: Fields) -> [String; 4] {
        let keep = [
            fields.title,
            fields.speakers,
            fields.summary,
            fields.transcript,
        ];
        let mut all = self.fields();
        for (text, keep) in all.iter_mut().zip(keep) {
            if !keep {
                text.clear();
            }
        }
        all
    }
}

impl State {
    fn row(&self, e: &Entry) -> RecordingRow {
        let job = self
            .jobs
            .values()
            .rfind(|j| j.recording == e.recording.id)
            .map(|j| j.state.clone());
        RecordingRow {
            recording: e.recording.clone(),
            job,
            has_transcript: e.transcript.is_some(),
            summary: SummaryFlag::derive(
                e.summary.as_ref().map(|s| s.transcript_revision),
                e.transcript.as_ref().map(|t| t.revision),
            ),
        }
    }

    fn newest_first(&self) -> Vec<&Entry> {
        let mut entries: Vec<&Entry> = self.recordings.values().collect();
        entries.sort_by(|a, b| {
            b.recording
                .created_at
                .cmp(&a.recording.created_at)
                .then_with(|| a.recording.id.cmp(&b.recording.id))
        });
        entries
    }
}

impl StoreRead for MemStore {
    fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        Ok(self.state().recordings.get(id).map(|e| e.recording.clone()))
    }

    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError> {
        let state = self.state();
        let mut entries = state.newest_first();
        if let Some(limit) = query.limit {
            entries.truncate(limit as usize);
        }
        Ok(entries.into_iter().map(|e| state.row(e)).collect())
    }

    fn search(&self, query: &Query) -> Result<Vec<Found>, StoreError> {
        let state = self.state();
        Ok(state
            .newest_first()
            .into_iter()
            .filter_map(|e| {
                let fields = e.fields();
                let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
                search::matches(query, &fields).then(|| Found {
                    row: state.row(e),
                    in_title: search::matches(query, &fields[..1]),
                })
            })
            .collect())
    }

    fn vocabulary(&self) -> Result<Arc<Vocabulary>, StoreError> {
        let state = self.state();
        let terms: Vec<String> = state
            .recordings
            .values()
            .flat_map(|e| e.fields())
            .flat_map(|field| search::tokens(&field).map(|t| t.folded).collect::<Vec<_>>())
            .collect();
        Ok(Arc::new(Vocabulary::new(0, terms)))
    }

    fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.transcript.clone()))
    }

    fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.summary.clone()))
    }

    fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .map(|e| e.names.clone())
            .unwrap_or_default())
    }

    fn job(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        Ok(self.state().jobs.get(&id).cloned())
    }

    fn jobs_for(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError> {
        Ok(self
            .state()
            .jobs
            .values()
            .filter(|j| &j.recording == id)
            .cloned()
            .collect())
    }

    fn next_runnable_job(&self, now: Timestamp) -> Result<Option<Job>, StoreError> {
        Ok(self
            .state()
            .jobs
            .values()
            .filter(|j| j.state == JobState::Queued && j.not_before.is_none_or(|t| t <= now))
            .min_by_key(|j| (std::cmp::Reverse(j.priority), j.place, j.id))
            .cloned())
    }

    fn next_wakeup(&self, now: Timestamp) -> Result<Option<Timestamp>, StoreError> {
        Ok(self
            .state()
            .jobs
            .values()
            .filter(|j| j.state == JobState::Queued)
            .filter_map(|j| j.not_before)
            .filter(|t| *t > now)
            .min())
    }

    fn settings(&self) -> Result<Settings, StoreError> {
        Ok(self.state().settings.clone())
    }

    fn checkpoints(&self, key: &CheckpointKey) -> Result<Vec<Checkpoint>, StoreError> {
        Ok(self
            .state()
            .checkpoints
            .get(key)
            .map(|windows| windows.values().cloned().collect())
            .unwrap_or_default())
    }

    fn assistant_access(&self) -> Result<AssistantAccess, StoreError> {
        let state = self.state();
        Ok(AssistantAccess {
            enabled: state.assistants_enabled,
            clients: state.assistants.values().map(|(c, _)| c.clone()).collect(),
        })
    }

    fn assistant_client(&self, key: &ClientKey) -> Result<Option<AssistantClient>, StoreError> {
        Ok(self
            .state()
            .assistants
            .values()
            .find(|(_, k)| k == key)
            .map(|(c, _)| c.clone()))
    }

    fn hidden_from_assistants(&self, id: &RecordingId) -> Result<bool, StoreError> {
        Ok(self.state().recordings.get(id).is_some_and(|e| e.hidden))
    }

    fn previous_summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.previous.clone()))
    }

    fn summary_instruction(&self, id: &RecordingId) -> Result<Option<String>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.instruction.clone()))
    }

    fn translation(&self, id: &RecordingId) -> Result<Option<Translation>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.translation.clone()))
    }

    fn summary_translation(
        &self,
        id: &RecordingId,
    ) -> Result<Option<SummaryTranslation>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.summary_translation.clone()))
    }

    fn summary_translation_wanted(&self, id: &RecordingId) -> Result<Option<LangTag>, StoreError> {
        Ok(self
            .state()
            .recordings
            .get(id)
            .and_then(|e| e.summary_translation_wanted.clone()))
    }

    fn translation_batches(
        &self,
        key: &TranslationKey,
    ) -> Result<Vec<TranslationBatch>, StoreError> {
        Ok(self
            .state()
            .translation_batches
            .get(key)
            .cloned()
            .unwrap_or_default())
    }

    fn assistant_recordings(&self, query: &AssistantQuery) -> Result<Vec<Found>, StoreError> {
        let state = self.state();
        let newer_than_cursor = |e: &Entry| {
            query.after.as_ref().is_none_or(|(at, id)| {
                (e.recording.created_at, std::cmp::Reverse(&e.recording.id))
                    < (*at, std::cmp::Reverse(id))
            })
        };
        Ok(state
            .newest_first()
            .into_iter()
            .filter(|e| {
                !e.hidden
                    && query.since.is_none_or(|t| e.recording.created_at >= t)
                    && query.until.is_none_or(|t| e.recording.created_at < t)
                    && newer_than_cursor(e)
            })
            .filter_map(|e| {
                let fields = e.granted(query.fields);
                let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
                match &query.query {
                    None => Some(Found {
                        row: state.row(e),
                        in_title: false,
                    }),
                    Some(q) => search::matches(q, &fields).then(|| Found {
                        row: state.row(e),
                        in_title: query.fields.title && search::matches(q, &fields[..1]),
                    }),
                }
            })
            .take(query.limit as usize)
            .collect())
    }

    fn assistant_vocabulary(&self, fields: Fields) -> Result<Vocabulary, StoreError> {
        let state = self.state();
        let terms: Vec<String> = state
            .recordings
            .values()
            .filter(|e| !e.hidden)
            .flat_map(|e| e.granted(fields))
            .flat_map(|field| search::tokens(&field).map(|t| t.folded).collect::<Vec<_>>())
            .collect();
        Ok(Vocabulary::new(0, terms))
    }
}

impl Store for MemStore {
    fn apply(&self, changes: Vec<Change>) -> Result<(), StoreError> {
        let mut state = self.state();
        let mut next = state.clone();
        for change in changes {
            next.apply_one(change)?;
        }
        *state = next;
        Ok(())
    }
}

pub use contract::{CASES, run_store_contract};

mod contract {
    use std::collections::BTreeMap;

    use super::*;
    use crate::domain::{
        Blocker, Channel, ChannelLayout, FailureCause, FailureClass, Goal, JobError, LangTag,
        LanguagePref, Ms, Processing, Recovery, Revision, Segment, SpeakerId, Stage, TakeContainer,
        TakeInfo, TimeRange,
    };
    use crate::models::{Choice, KindTag, ModelRef};
    use crate::ports::{Checkpoint, CheckpointKey, RawSegment};
    use crate::ports::{TranslationBatch, TranslationKey};

    type Case = fn(&dyn Store);

    /// Every case, by name. An adapter test can run one case on its own.
    pub const CASES: &[(&str, Case)] = &[
        ("fresh_store_is_empty_with_default_settings", fresh_store),
        ("recording_round_trips_every_field", recording_round_trip),
        (
            "creating_an_existing_recording_keeps_the_first",
            create_twice,
        ),
        ("edits_change_only_what_they_name", edits),
        ("edit_of_a_missing_recording_is_not_found", edit_missing),
        ("apply_is_all_or_nothing", atomic_apply),
        (
            "a_failed_apply_enqueues_nothing",
            failed_apply_leaves_no_job,
        ),
        (
            "transcript_and_summary_are_replaced_whole",
            replace_transcript_and_summary,
        ),
        (
            "transcript_for_a_missing_recording_is_not_found",
            derived_rows_need_a_recording,
        ),
        ("speaker_names_round_trip_and_default_empty", speaker_names),
        ("settings_round_trip", settings_round_trip),
        ("delete_cascades_and_is_idempotent", delete_cascades),
        (
            "list_is_newest_first_with_limit",
            list_order_limit_and_filter,
        ),
        (
            "search_words_any_order_and_prefix",
            search_words_any_order_and_prefix,
        ),
        (
            "search_folds_accents_both_ways",
            search_folds_accents_both_ways,
        ),
        (
            "search_reads_final_sigma_and_letters_unicode61_splits",
            search_reads_final_sigma_and_letters_unicode61_splits,
        ),
        (
            "search_prefixes_the_last_part_of_a_typed_split_word",
            search_prefixes_the_last_part_of_a_typed_split_word,
        ),
        ("search_phrase_needs_order", search_phrase_needs_order),
        ("search_sets_in_title", search_sets_in_title),
        (
            "search_follows_rename_retranscribe_names_and_summary",
            search_follows_rename_retranscribe_names_and_summary,
        ),
        (
            "search_forgets_deleted_and_finds_restored",
            search_forgets_deleted_and_finds_restored,
        ),
        ("search_quotes_fts_keywords", search_quotes_fts_keywords),
        (
            "search_corrects_only_when_nothing_matches",
            search_corrects_only_when_nothing_matches,
        ),
        ("vocabulary_follows_writes", vocabulary_follows_writes),
        (
            "list_rows_carry_job_transcript_and_summary_flag",
            list_row_fields,
        ),
        ("job_events_follow_job_on", job_transitions),
        (
            "illegal_job_event_is_rejected_and_changes_nothing",
            illegal_job_event,
        ),
        ("every_job_state_round_trips", job_states_round_trip),
        (
            "next_runnable_is_front_first_then_lowest_place",
            runnable_ordering,
        ),
        (
            "a_placed_job_is_picked_in_its_new_spot",
            runnable_follows_place,
        ),
        ("not_before_defers_a_job_and_sets_the_wakeup", not_before),
        (
            "enqueue_is_idempotent_per_recording_and_goal",
            enqueue_idempotent,
        ),
        (
            "enqueue_after_a_terminal_job_makes_a_new_one",
            enqueue_after_terminal,
        ),
        ("job_ids_are_never_reused", job_ids_unique),
        ("concurrent_applies_all_land", concurrent_applies),
        (
            "checkpoints_are_kept_per_run_in_window_order",
            checkpoints_per_run,
        ),
        (
            "saving_a_window_again_replaces_its_checkpoint",
            checkpoint_replace,
        ),
        (
            "clearing_checkpoints_drops_every_run_of_that_recording_only",
            checkpoints_cleared,
        ),
        (
            "checkpoints_need_a_recording_and_go_with_it",
            checkpoints_follow_the_recording,
        ),
        (
            "a_segment_keeps_the_language_it_was_heard_in",
            segment_language_round_trips,
        ),
        (
            "a_translation_sits_beside_the_transcript_and_is_replaced_whole",
            translation_round_trips,
        ),
        (
            "a_translation_outlives_a_new_transcript_revision_and_says_which_it_is_for",
            translation_goes_stale_with_the_revision,
        ),
        (
            "search_finds_the_translation_while_it_applies",
            search_finds_the_translation_while_it_applies,
        ),
        (
            "assistant_search_finds_the_translation_only_under_a_transcript_grant",
            assistant_search_reads_the_translation_as_transcript,
        ),
        (
            "search_finds_the_summary_translation_until_the_summary_changes",
            search_finds_the_summary_translation,
        ),
        (
            "assistant_search_finds_the_summary_translation_under_a_summaries_grant",
            assistant_search_reads_the_summary_translation_as_summary,
        ),
        (
            "translation_batches_are_kept_per_run_in_save_order",
            translation_batches_per_run,
        ),
        (
            "translation_data_needs_a_recording_and_goes_with_it",
            translation_follows_the_recording,
        ),
        (
            "summary_translation_round_trips",
            summary_translation_round_trips,
        ),
        (
            "a_summary_translation_applies_only_while_its_wish_stands",
            summary_translation_needs_its_wish,
        ),
        (
            "a_new_summary_drops_its_translation",
            a_new_summary_drops_its_translation,
        ),
        (
            "restoring_a_summary_drops_its_translation",
            restoring_a_summary_drops_its_translation,
        ),
        (
            "deleting_a_recording_drops_its_translation",
            deleting_a_recording_drops_its_translation,
        ),
    ];

    /// Runs every case against a fresh store from `make`. Panics with the case name on failure.
    pub fn run_store_contract(make: impl Fn() -> Box<dyn Store>) {
        for (name, case) in CASES {
            let store = make();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(&*store)));
            if let Err(panic) = result {
                eprintln!("store contract case failed: {name}");
                std::panic::resume_unwind(panic);
            }
        }
    }

    fn rid(s: &str) -> RecordingId {
        RecordingId::parse(s).unwrap()
    }

    fn model() -> ModelRef {
        ModelRef::parse("sona/whisper-small").unwrap()
    }

    fn recording(id: &str, created: i64) -> Recording {
        Recording {
            id: rid(id),
            title: None,
            created_at: Timestamp(created),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: None,
        }
    }

    fn create(store: &dyn Store, id: &str, created: i64) {
        store
            .apply(vec![Change::RecordingCreated(recording(id, created))])
            .unwrap();
    }

    fn segment(speaker: u16, range: Option<(u64, u64)>, text: &str) -> Segment {
        Segment {
            range: range.map(|(a, b)| TimeRange::new(Ms(a), Ms(b)).unwrap()),
            speaker: SpeakerId(speaker),
            channel: Channel::System,
            text: text.into(),
            language: None,
        }
    }

    fn transcript(revision: u32, texts: &[&str]) -> Transcript {
        Transcript {
            revision: Revision(revision),
            segments: texts
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    segment(
                        i as u16 % 2,
                        Some((i as u64 * 1000, i as u64 * 1000 + 900)),
                        t,
                    )
                })
                .collect(),
            detected_languages: vec![LangTag::parse("en").unwrap()],
            produced_by: model(),
        }
    }

    fn summary(of: u32, markdown: &str) -> Summary {
        Summary {
            markdown: markdown.into(),
            language: Some(LangTag::parse("en").unwrap()),
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            transcript_revision: Revision(of),
        }
    }

    fn enqueue(store: &dyn Store, id: &str, goal: Goal, priority: Priority, at: i64) {
        store
            .apply(vec![Change::JobEnqueued {
                id: rid(id),
                goal,
                priority,
                at: Timestamp(at),
            }])
            .unwrap();
    }

    fn only_job(store: &dyn Store, id: &str) -> Job {
        let mut jobs = store.jobs_for(&rid(id)).unwrap();
        assert_eq!(jobs.len(), 1, "expected exactly one job for {id}: {jobs:?}");
        jobs.remove(0)
    }

    fn event(store: &dyn Store, job: JobId, event: JobEvent) -> Result<(), StoreError> {
        store.apply(vec![Change::Job { job, event }])
    }

    fn error() -> JobError {
        JobError {
            class: FailureClass::Permanent,
            message: "boom".into(),
            cause: FailureCause::Other,
        }
    }

    fn assert_not_found(result: Result<(), StoreError>) {
        match result {
            Err(StoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    fn fresh_store(store: &dyn Store) {
        assert_eq!(
            store.recordings(&RecordingQuery::default()).unwrap(),
            vec![]
        );
        assert_eq!(store.settings().unwrap(), Settings::default());
        assert_eq!(store.recording(&rid("nope")).unwrap(), None);
        assert_eq!(store.transcript(&rid("nope")).unwrap(), None);
        assert_eq!(store.summary(&rid("nope")).unwrap(), None);
        assert_eq!(
            store.speaker_names(&rid("nope")).unwrap(),
            SpeakerNames::default()
        );
        assert_eq!(store.job(JobId(1)).unwrap(), None);
        assert_eq!(store.jobs_for(&rid("nope")).unwrap(), vec![]);
        assert_eq!(store.next_runnable_job(Timestamp(i64::MAX)).unwrap(), None);
        assert_eq!(store.next_wakeup(Timestamp(0)).unwrap(), None);
    }

    fn recording_round_trip(store: &dyn Store) {
        let full = Recording {
            id: rid("2026-11-15T153012-7f3a"),
            title: Some("Standup \u{1f600} \"quoted\" \u{e9}\u{1ed3}".into()),
            created_at: Timestamp(1_700_000_000_123),
            language: LanguagePref::Pinned(LangTag::parse("vi").unwrap()),
            layout: ChannelLayout::SystemOnly,
            take: Some(TakeInfo {
                container: TakeContainer::Flac,
                rate_hz: 48_000,
                duration: Ms(7_654_321),
            }),
        };
        let bare = Recording {
            id: rid("legacy_stem.v2"),
            title: None,
            created_at: Timestamp(-5),
            language: LanguagePref::Auto,
            layout: ChannelLayout::Mixed,
            take: None,
        };
        store
            .apply(vec![
                Change::RecordingCreated(full.clone()),
                Change::RecordingCreated(bare.clone()),
            ])
            .unwrap();
        assert_eq!(store.recording(&full.id).unwrap(), Some(full));
        assert_eq!(store.recording(&bare.id).unwrap(), Some(bare));
        for container in [
            TakeContainer::Wav,
            TakeContainer::Flac,
            TakeContainer::LegacyWebm,
        ] {
            let id = rid(&format!("c-{container:?}"));
            let rec = Recording {
                take: Some(TakeInfo {
                    container,
                    rate_hz: 16_000,
                    duration: Ms(0),
                }),
                ..recording(id.as_str(), 1)
            };
            store
                .apply(vec![Change::RecordingCreated(rec.clone())])
                .unwrap();
            assert_eq!(store.recording(&id).unwrap(), Some(rec));
        }
        for layout in [
            ChannelLayout::MicSystem,
            ChannelLayout::MicOnly,
            ChannelLayout::SystemOnly,
            ChannelLayout::Mixed,
        ] {
            let id = rid(&format!("l-{layout:?}"));
            let rec = Recording {
                layout,
                ..recording(id.as_str(), 1)
            };
            store
                .apply(vec![Change::RecordingCreated(rec.clone())])
                .unwrap();
            assert_eq!(store.recording(&id).unwrap(), Some(rec));
        }
    }

    fn create_twice(store: &dyn Store) {
        let first = Recording {
            title: Some("first".into()),
            ..recording("r", 10)
        };
        let second = Recording {
            title: Some("second".into()),
            ..recording("r", 20)
        };
        store
            .apply(vec![Change::RecordingCreated(first.clone())])
            .unwrap();
        store.apply(vec![Change::RecordingCreated(second)]).unwrap();
        assert_eq!(store.recording(&rid("r")).unwrap(), Some(first));
        assert_eq!(
            store.recordings(&RecordingQuery::default()).unwrap().len(),
            1
        );
    }

    fn edits(store: &dyn Store) {
        create(store, "r", 10);
        let id = rid("r");
        let take = TakeInfo {
            container: TakeContainer::Wav,
            rate_hz: 48_000,
            duration: Ms(1500),
        };
        let edit = |edit| Change::RecordingEdited {
            id: id.clone(),
            edit,
        };
        store
            .apply(vec![
                edit(RecordingEdit::Title(Some("Roadmap".into()))),
                edit(RecordingEdit::Language(LanguagePref::Pinned(
                    LangTag::parse("fr-CA").unwrap(),
                ))),
                edit(RecordingEdit::Take(take)),
            ])
            .unwrap();
        let got = store.recording(&id).unwrap().unwrap();
        assert_eq!(got.title.as_deref(), Some("Roadmap"));
        assert_eq!(
            got.language,
            LanguagePref::Pinned(LangTag::parse("fr-CA").unwrap())
        );
        assert_eq!(got.take, Some(take));
        assert_eq!(
            (got.created_at, got.layout),
            (Timestamp(10), ChannelLayout::MicSystem)
        );

        store.apply(vec![edit(RecordingEdit::Title(None))]).unwrap();
        assert_eq!(store.recording(&id).unwrap().unwrap().title, None);
        let later = TakeInfo {
            container: TakeContainer::Flac,
            duration: Ms(2000),
            ..take
        };
        store.apply(vec![edit(RecordingEdit::Take(later))]).unwrap();
        assert_eq!(store.recording(&id).unwrap().unwrap().take, Some(later));
    }

    fn edit_missing(store: &dyn Store) {
        assert_not_found(store.apply(vec![Change::RecordingEdited {
            id: rid("ghost"),
            edit: RecordingEdit::Title(Some("x".into())),
        }]));
    }

    fn atomic_apply(store: &dyn Store) {
        create(store, "r", 10);
        enqueue(store, "r", Goal::ALL, Priority::Normal, 10);
        let job = only_job(store, "r");
        let before = store.recordings(&RecordingQuery::default()).unwrap();

        // The first changes are fine, the last is an illegal transition: nothing may land.
        let result = store.apply(vec![
            Change::RecordingCreated(recording("fresh", 99)),
            Change::TranscriptReplaced {
                id: rid("r"),
                transcript: transcript(1, &["hello"]),
            },
            Change::SummaryReplaced {
                id: rid("r"),
                summary: summary(1, "# s"),
            },
            Change::SpeakerNamesReplaced {
                id: rid("r"),
                names: SpeakerNames {
                    for_revision: Revision(1),
                    names: BTreeMap::from([(SpeakerId(1), "Ana".to_owned())]),
                },
            },
            Change::RecordingEdited {
                id: rid("r"),
                edit: RecordingEdit::Title(Some("changed".into())),
            },
            Change::SettingsReplaced(Settings {
                auto_process: false,
                ..Settings::default()
            }),
            Change::JobEnqueued {
                id: rid("r"),
                goal: Goal::TRANSCRIBE,
                priority: Priority::Normal,
                at: Timestamp(11),
            },
            Change::CheckpointSaved {
                key: run("r", "sona/whisper-small", Channel::Mic),
                checkpoint: checkpoint(0, 1_000, &["hi"]),
            },
            Change::Job {
                job: job.id,
                event: JobEvent::StageDone {
                    stage: Stage::Transcribe,
                },
            },
        ]);
        assert!(
            matches!(result, Err(StoreError::IllegalTransition(_))),
            "{result:?}"
        );

        assert_eq!(store.recording(&rid("fresh")).unwrap(), None);
        assert_eq!(store.transcript(&rid("r")).unwrap(), None);
        assert_eq!(store.summary(&rid("r")).unwrap(), None);
        assert_eq!(
            store.speaker_names(&rid("r")).unwrap(),
            SpeakerNames::default()
        );
        assert_eq!(store.settings().unwrap(), Settings::default());
        assert_eq!(store.jobs_for(&rid("r")).unwrap(), vec![job]);
        assert_eq!(
            store.recordings(&RecordingQuery::default()).unwrap(),
            before
        );
        assert_eq!(
            store
                .checkpoints(&run("r", "sona/whisper-small", Channel::Mic))
                .unwrap(),
            vec![]
        );
    }

    fn failed_apply_leaves_no_job(store: &dyn Store) {
        create(store, "r", 10);
        let result = store.apply(vec![
            Change::JobEnqueued {
                id: rid("r"),
                goal: Goal::ALL,
                priority: Priority::Normal,
                at: Timestamp(1),
            },
            Change::TranscriptReplaced {
                id: rid("ghost"),
                transcript: transcript(1, &["x"]),
            },
        ]);
        assert_not_found(result);
        assert_eq!(store.jobs_for(&rid("r")).unwrap(), vec![]);
    }

    fn replace_transcript_and_summary(store: &dyn Store) {
        create(store, "r", 10);
        let id = rid("r");
        let first = Transcript {
            revision: Revision(1),
            segments: vec![
                segment(0, Some((0, 900)), "hello there"),
                segment(1, None, "untimed line"),
                segment(2, Some((2_000, 2_000)), "empty range"),
            ],
            detected_languages: vec![LangTag::parse("en").unwrap(), LangTag::parse("vi").unwrap()],
            produced_by: model(),
        };
        store
            .apply(vec![
                Change::TranscriptReplaced {
                    id: id.clone(),
                    transcript: first.clone(),
                },
                Change::SummaryReplaced {
                    id: id.clone(),
                    summary: summary(1, "# Notes\n\n- line \"one\"\n- d\u{f2}ng hai"),
                },
            ])
            .unwrap();
        assert_eq!(store.transcript(&id).unwrap(), Some(first));
        assert_eq!(
            store.summary(&id).unwrap(),
            Some(summary(1, "# Notes\n\n- line \"one\"\n- d\u{f2}ng hai"))
        );

        let second = transcript(2, &["only this"]);
        store
            .apply(vec![Change::TranscriptReplaced {
                id: id.clone(),
                transcript: second.clone(),
            }])
            .unwrap();
        let got = store.transcript(&id).unwrap().unwrap();
        assert_eq!(got, second, "the old segments are gone, not merged");

        let no_lang = Summary {
            language: None,
            ..summary(2, "second")
        };
        store
            .apply(vec![Change::SummaryReplaced {
                id: id.clone(),
                summary: no_lang.clone(),
            }])
            .unwrap();
        assert_eq!(store.summary(&id).unwrap(), Some(no_lang));
    }

    fn derived_rows_need_a_recording(store: &dyn Store) {
        let ghost = rid("ghost");
        assert_not_found(store.apply(vec![Change::TranscriptReplaced {
            id: ghost.clone(),
            transcript: transcript(1, &["x"]),
        }]));
        assert_not_found(store.apply(vec![Change::SummaryReplaced {
            id: ghost.clone(),
            summary: summary(1, "x"),
        }]));
        assert_not_found(store.apply(vec![Change::SpeakerNamesReplaced {
            id: ghost.clone(),
            names: SpeakerNames::default(),
        }]));
        assert_not_found(store.apply(vec![Change::JobEnqueued {
            id: ghost,
            goal: Goal::ALL,
            priority: Priority::Normal,
            at: Timestamp(0),
        }]));
        assert_not_found(event(store, JobId(42), JobEvent::Cancelled));
    }

    fn speaker_names(store: &dyn Store) {
        create(store, "r", 10);
        let id = rid("r");
        assert_eq!(store.speaker_names(&id).unwrap(), SpeakerNames::default());
        let names = SpeakerNames {
            for_revision: Revision(3),
            names: BTreeMap::from([
                (SpeakerId(0), "Me".to_owned()),
                (SpeakerId(2), "\u{110}\u{1ea1}t".to_owned()),
            ]),
        };
        store
            .apply(vec![Change::SpeakerNamesReplaced {
                id: id.clone(),
                names: names.clone(),
            }])
            .unwrap();
        assert_eq!(store.speaker_names(&id).unwrap(), names);

        let smaller = SpeakerNames {
            for_revision: Revision(4),
            names: BTreeMap::from([(SpeakerId(1), "Bo".to_owned())]),
        };
        store
            .apply(vec![Change::SpeakerNamesReplaced {
                id: id.clone(),
                names: smaller.clone(),
            }])
            .unwrap();
        assert_eq!(
            store.speaker_names(&id).unwrap(),
            smaller,
            "replace, not merge"
        );

        store
            .apply(vec![Change::SpeakerNamesReplaced {
                id: id.clone(),
                names: SpeakerNames::default(),
            }])
            .unwrap();
        assert_eq!(store.speaker_names(&id).unwrap(), SpeakerNames::default());
    }

    fn settings_round_trip(store: &dyn Store) {
        let pinned = Settings {
            stt: Choice::Pinned("sherpa/qwen3-asr-0.6b".parse().unwrap()),
            llm: Choice::Pinned("ollama/hf.co/user/model:q4".parse().unwrap()),
            diar: Choice::Off,
            language: LanguagePref::Pinned(LangTag::parse("zh-Hans-CN").unwrap()),
            meeting_languages: vec![LangTag::parse("tl").unwrap(), LangTag::parse("en").unwrap()],
            output_language: Some(LangTag::parse("en-US").unwrap()),
            auto_process: false,
            processing: Processing::Light,
            switch_recovery: Recovery::Off,
        };
        let off = Settings {
            stt: Choice::Auto,
            llm: Choice::Off,
            diar: Choice::Auto,
            language: LanguagePref::Auto,
            meeting_languages: Vec::new(),
            output_language: None,
            auto_process: true,
            processing: Processing::Fast,
            switch_recovery: Recovery::On,
        };
        for settings in [pinned, off, Settings::default()] {
            store
                .apply(vec![Change::SettingsReplaced(settings.clone())])
                .unwrap();
            assert_eq!(store.settings().unwrap(), settings);
        }
    }

    fn delete_cascades(store: &dyn Store) {
        create(store, "gone", 10);
        create(store, "kept", 20);
        for id in ["gone", "kept"] {
            store
                .apply(vec![
                    Change::TranscriptReplaced {
                        id: rid(id),
                        transcript: transcript(1, &["text"]),
                    },
                    Change::SummaryReplaced {
                        id: rid(id),
                        summary: summary(1, "s"),
                    },
                    Change::SpeakerNamesReplaced {
                        id: rid(id),
                        names: SpeakerNames {
                            for_revision: Revision(1),
                            names: BTreeMap::from([(SpeakerId(1), "Ana".to_owned())]),
                        },
                    },
                ])
                .unwrap();
            enqueue(store, id, Goal::ALL, Priority::Normal, 5);
        }
        let gone_job = only_job(store, "gone");

        store
            .apply(vec![Change::RecordingDeleted(rid("gone"))])
            .unwrap();
        assert_eq!(store.recording(&rid("gone")).unwrap(), None);
        assert_eq!(store.transcript(&rid("gone")).unwrap(), None);
        assert_eq!(store.summary(&rid("gone")).unwrap(), None);
        assert_eq!(
            store.speaker_names(&rid("gone")).unwrap(),
            SpeakerNames::default()
        );
        assert_eq!(store.jobs_for(&rid("gone")).unwrap(), vec![]);
        assert_eq!(store.job(gone_job.id).unwrap(), None);
        assert_eq!(
            store
                .next_runnable_job(Timestamp(100))
                .unwrap()
                .map(|j| j.recording),
            Some(rid("kept"))
        );

        assert!(store.recording(&rid("kept")).unwrap().is_some());
        assert!(store.transcript(&rid("kept")).unwrap().is_some());
        assert!(store.summary(&rid("kept")).unwrap().is_some());
        assert_eq!(store.speaker_names(&rid("kept")).unwrap().names.len(), 1);
        assert_eq!(store.jobs_for(&rid("kept")).unwrap().len(), 1);

        store
            .apply(vec![Change::RecordingDeleted(rid("gone"))])
            .unwrap();

        create(store, "gone", 30);
        assert_eq!(
            store.transcript(&rid("gone")).unwrap(),
            None,
            "no ghost rows from the old life"
        );
        assert_eq!(store.jobs_for(&rid("gone")).unwrap(), vec![]);
    }

    fn list_order_limit_and_filter(store: &dyn Store) {
        store
            .apply(vec![
                Change::RecordingCreated(recording("b", 200)),
                Change::RecordingCreated(recording("a", 300)),
                Change::RecordingCreated(recording("d", 100)),
                Change::RecordingCreated(recording("c", 100)),
            ])
            .unwrap();
        let ids = |q: RecordingQuery| -> Vec<String> {
            store
                .recordings(&q)
                .unwrap()
                .into_iter()
                .map(|r| r.recording.id.to_string())
                .collect()
        };
        assert_eq!(
            ids(RecordingQuery::default()),
            ["a", "b", "c", "d"],
            "newest first, ties by id"
        );
        assert_eq!(ids(RecordingQuery { limit: Some(2) }), ["a", "b"]);
        assert_eq!(ids(RecordingQuery { limit: Some(0) }), Vec::<String>::new());
    }

    /// The ids `text` finds, in the order the store lists them.
    fn found(store: &dyn Store, text: &str) -> Vec<String> {
        let query = Query::parse(text).expect("a query with words");
        store
            .search(&query)
            .unwrap()
            .into_iter()
            .map(|f| f.row.recording.id.to_string())
            .collect()
    }

    fn titled(id: &str, created: i64, title: &str) -> Change {
        Change::RecordingCreated(Recording {
            title: Some(title.into()),
            ..recording(id, created)
        })
    }

    fn said(id: &str, revision: u32, texts: &[&str]) -> Change {
        Change::TranscriptReplaced {
            id: rid(id),
            transcript: transcript(revision, texts),
        }
    }

    fn search_words_any_order_and_prefix(store: &dyn Store) {
        store
            .apply(vec![
                titled("a", 300, "Weekly sync"),
                said("a", 1, &["We did the budget review on Monday"]),
                titled("b", 200, "Planning"),
                said("b", 1, &["The review of the budget", "slipped again"]),
                titled("c", 100, "Kickoff"),
                said("c", 1, &["The budget is fine"]),
                Change::SummaryReplaced {
                    id: rid("c"),
                    summary: summary(1, "## Notes\n- Review the roadmap"),
                },
            ])
            .unwrap();
        assert_eq!(found(store, "budget review"), ["a", "b", "c"], "c: summary");
        assert_eq!(found(store, "review budget"), ["a", "b", "c"], "any order");
        assert_eq!(
            found(store, "bud rev"),
            ["a", "b", "c"],
            "every word is a prefix"
        );
        assert_eq!(
            found(store, "BUDGET slipped"),
            ["b"],
            "across turns, any case"
        );
        assert_eq!(
            found(store, "roadmap budget"),
            ["c"],
            "summary and transcript"
        );
        assert_eq!(
            found(store, "udget"),
            Vec::<String>::new(),
            "word starts only"
        );
        assert_eq!(found(store, "budget zanzibar"), Vec::<String>::new());
        assert_eq!(found(store, "weekly"), ["a"], "titles");
    }

    fn search_folds_accents_both_ways(store: &dyn Store) {
        store
            .apply(vec![
                titled("plain", 300, "Call with Muller"),
                titled("umlaut", 200, "Call with Müller"),
                said("umlaut", 1, &["Die Straße am Ørsted-Park"]),
                titled("other", 100, "Café ﬁnance"),
            ])
            .unwrap();
        for text in ["muller", "Müller", "MULLER", "mül"] {
            assert_eq!(found(store, text), ["plain", "umlaut"], "{text}");
        }
        assert_eq!(found(store, "strasse"), ["umlaut"]);
        assert_eq!(found(store, "STRAẞE orsted"), ["umlaut"]);
        assert_eq!(found(store, "cafe finance"), ["other"]);
        assert_eq!(found(store, "café ﬁ"), ["other"]);
    }

    fn search_reads_final_sigma_and_letters_unicode61_splits(store: &dyn Store) {
        store
            .apply(vec![
                titled("greek", 200, "Ο δρόμος"),
                titled("squared", 100, "🅐 budget"),
            ])
            .unwrap();
        for text in ["ΔΡΟΜΟΣ", "δρομοσ", "δρόμος", "δρομοϲ", "δρομ"] {
            assert_eq!(found(store, text), ["greek"], "{text}");
            let marks = search::highlights(&Query::parse(text).unwrap(), "Ο δρόμος");
            assert_eq!(marks, vec![3..15], "{text} marks the word");
        }
        for text in ["🅐 budget", "budget", "bud"] {
            assert_eq!(found(store, text), ["squared"], "{text}");
            let marks = search::highlights(&Query::parse(text).unwrap(), "🅐 budget");
            assert_eq!(marks.len(), 1, "{text} marks what the index found");
        }
    }

    fn search_prefixes_the_last_part_of_a_typed_split_word(store: &dyn Store) {
        store
            .apply(vec![
                titled("title", 300, "Follow-up notes"),
                titled("talk", 200, "Weekly"),
                said("talk", 1, &["a follow up is due", "write an e-mail"]),
                titled("far", 100, "Planning"),
                said("far", 1, &["follow the plan, up to you"]),
            ])
            .unwrap();
        for text in ["follow-u", "follow-up", "Follow-UP"] {
            assert_eq!(found(store, text), ["title", "talk"], "{text}");
        }
        assert_eq!(found(store, "\"follow u\""), Vec::<String>::new(), "quoted");
        assert_eq!(found(store, "\"follow up\""), ["title", "talk"]);
        assert_eq!(found(store, "follow-down"), Vec::<String>::new());
        assert_eq!(found(store, "e-mai"), ["talk"]);
        assert_eq!(found(store, "e-mai follow-u"), ["talk"]);
    }

    fn search_phrase_needs_order(store: &dyn Store) {
        store
            .apply(vec![
                recording_change("adjacent", 300),
                said("adjacent", 1, &["the budget review is done"]),
                recording_change("apart", 200),
                said("apart", 1, &["the review of the budget"]),
                recording_change("split", 100),
                said("split", 1, &["we talked budget", "review next week"]),
            ])
            .unwrap();
        assert_eq!(found(store, "\"budget review\""), ["adjacent", "split"]);
        assert_eq!(
            found(store, "\"review budget\""),
            Vec::<String>::new(),
            "a phrase keeps its order"
        );
        assert_eq!(
            found(store, "\"budget\""),
            ["adjacent", "apart", "split"],
            "a quoted word is a whole word"
        );
        assert_eq!(found(store, "\"budge\""), Vec::<String>::new());
        assert_eq!(
            found(store, "\"budget review"),
            ["adjacent", "split"],
            "unbalanced"
        );
        assert_eq!(found(store, "budget-review"), ["adjacent", "split"]);
    }

    fn recording_change(id: &str, created: i64) -> Change {
        Change::RecordingCreated(recording(id, created))
    }

    fn search_sets_in_title(store: &dyn Store) {
        store
            .apply(vec![
                titled("title", 300, "Budget review"),
                titled("half", 200, "Budget"),
                said("half", 1, &["review it"]),
                titled("body", 100, "Weekly"),
                said("body", 1, &["budget review"]),
            ])
            .unwrap();
        let hits: Vec<(String, bool)> = store
            .search(&Query::parse("budget rev").unwrap())
            .unwrap()
            .into_iter()
            .map(|f| (f.row.recording.id.to_string(), f.in_title))
            .collect();
        assert_eq!(
            hits,
            [
                ("title".to_owned(), true),
                ("half".to_owned(), false),
                ("body".to_owned(), false)
            ]
        );
        let row = store.search(&Query::parse("weekly").unwrap()).unwrap();
        assert_eq!(
            row[0].row,
            store.recordings(&RecordingQuery::default()).unwrap()[2],
            "a hit carries the same row the list does"
        );
    }

    fn search_follows_rename_retranscribe_names_and_summary(store: &dyn Store) {
        store
            .apply(vec![
                titled("r", 100, "Weekly sync"),
                said("r", 1, &["kilimanjaro trip", "budget"]),
            ])
            .unwrap();
        store
            .apply(vec![Change::RecordingEdited {
                id: rid("r"),
                edit: RecordingEdit::Title(Some("Atlas kickoff".into())),
            }])
            .unwrap();
        assert_eq!(found(store, "atlas"), ["r"], "a new title");
        assert_eq!(
            found(store, "weekly"),
            Vec::<String>::new(),
            "the old title"
        );
        store
            .apply(vec![Change::SpeakerNamesReplaced {
                id: rid("r"),
                names: SpeakerNames {
                    for_revision: Revision(1),
                    names: BTreeMap::from([(SpeakerId(2), "Priyanka".to_owned())]),
                },
            }])
            .unwrap();
        assert_eq!(found(store, "priyanka"), ["r"], "a speaker name");
        store.apply(vec![said("r", 2, &["fresh words"])]).unwrap();
        assert_eq!(
            found(store, "kilimanjaro"),
            Vec::<String>::new(),
            "the old transcript"
        );
        assert_eq!(found(store, "fresh"), ["r"]);
        assert_eq!(
            found(store, "priyanka"),
            Vec::<String>::new(),
            "names given for revision 1 do not name revision 2's speakers"
        );
        store
            .apply(vec![Change::SummaryReplaced {
                id: rid("r"),
                summary: summary(2, "Decision: ship the Beta on Friday"),
            }])
            .unwrap();
        assert_eq!(found(store, "beta friday"), ["r"], "a summary");
        store
            .apply(vec![Change::SummaryReplaced {
                id: rid("r"),
                summary: summary(2, "Nothing decided"),
            }])
            .unwrap();
        assert_eq!(
            found(store, "beta"),
            Vec::<String>::new(),
            "a replaced summary"
        );
        store
            .apply(vec![Change::RecordingEdited {
                id: rid("r"),
                edit: RecordingEdit::Title(None),
            }])
            .unwrap();
        assert_eq!(
            found(store, "atlas"),
            Vec::<String>::new(),
            "a cleared title"
        );
        assert_eq!(found(store, "fresh"), ["r"]);
    }

    fn search_forgets_deleted_and_finds_restored(store: &dyn Store) {
        let restore = || {
            vec![
                titled("gone", 200, "Roadmap"),
                said("gone", 1, &["kilimanjaro"]),
                Change::SummaryReplaced {
                    id: rid("gone"),
                    summary: summary(1, "okavango"),
                },
            ]
        };
        store.apply(restore()).unwrap();
        store
            .apply(vec![titled("kept", 100, "Roadmap two")])
            .unwrap();
        store
            .apply(vec![Change::RecordingDeleted(rid("gone"))])
            .unwrap();
        for text in ["roadmap", "kilimanjaro", "okavango"] {
            let want: &[&str] = if text == "roadmap" { &["kept"] } else { &[] };
            assert_eq!(found(store, text), want, "{text} after delete");
        }
        store.apply(restore()).unwrap();
        assert_eq!(found(store, "roadmap"), ["gone", "kept"]);
        assert_eq!(
            found(store, "kilimanjaro okavango"),
            ["gone"],
            "after restore"
        );
        assert!(
            store
                .apply(vec![titled("x", 1, "x"), said("missing", 1, &["x"])])
                .is_err()
        );
        assert_eq!(
            found(store, "x"),
            Vec::<String>::new(),
            "a failed apply indexes nothing"
        );
    }

    fn search_quotes_fts_keywords(store: &dyn Store) {
        store
            .apply(vec![
                recording_change("k", 200),
                said(
                    "k",
                    1,
                    &["this OR that, NEAR the NOT-quite AND title: * ^ col"],
                ),
                recording_change("other", 100),
                said("other", 1, &["plain words here"]),
            ])
            .unwrap();
        for text in [
            "or",
            "OR",
            "near",
            "not",
            "and",
            "and or",
            "title:",
            "title:col",
            "NEAR(the",
            "\"or that\"",
            "this*",
            "^col",
            "col)",
            "(this",
            "this -that",
            "{title}",
        ] {
            assert_eq!(found(store, text), ["k"], "{text}");
        }
        assert_eq!(found(store, "plain"), ["other"]);
    }

    /// The ids and corrections `text` gives through the same lookup the facade runs.
    fn corrected_search(
        store: &dyn Store,
        text: &str,
    ) -> (Vec<String>, Vec<(String, Vec<String>)>) {
        let (_, hits, corrections) = crate::app::find(
            Query::parse(text).unwrap(),
            |q| store.search(q),
            || store.vocabulary(),
        )
        .unwrap();
        (
            hits.into_iter()
                .map(|f| f.row.recording.id.to_string())
                .collect(),
            corrections.into_iter().map(|c| (c.typed, c.used)).collect(),
        )
    }

    fn corrected(typed: &str, used: &[&str]) -> (String, Vec<String>) {
        (
            typed.to_owned(),
            used.iter().map(|u| u.to_string()).collect(),
        )
    }

    fn search_corrects_only_when_nothing_matches(store: &dyn Store) {
        store
            .apply(vec![
                titled("a", 300, "Planning"),
                said("a", 1, &["the roadmap slips", "budget is fine"]),
                titled("b", 200, "Atlas kickoff"),
                said("b", 1, &["we reviewed roadmaps"]),
                titled("c", 100, "Kilimanjaro"),
                said("c", 1, &["antinational budget"]),
            ])
            .unwrap();
        let none: Vec<String> = Vec::new();
        let no_correction: Vec<(String, Vec<String>)> = Vec::new();

        assert_eq!(
            corrected_search(store, "roadmap"),
            (vec!["a".into(), "b".into()], no_correction.clone()),
            "an exact hit is never corrected, though roadmaps is one edit away"
        );
        assert_eq!(
            corrected_search(store, "roadmpa"),
            (vec!["a".into()], vec![corrected("roadmpa", &["roadmap"])]),
            "a swap"
        );
        assert_eq!(
            corrected_search(store, "kilimanjro").1,
            vec![corrected("kilimanjro", &["kilimanjaro"])],
            "a drop"
        );
        assert_eq!(
            corrected_search(store, "antinatoinal"),
            (
                vec!["c".into()],
                vec![corrected("antinatoinal", &["antinational"])]
            ),
            "two edits at nine letters"
        );
        assert_eq!(
            corrected_search(store, "atlsa"),
            (vec!["b".into()], vec![corrected("atlsa", &["atlas"])]),
            "five letters"
        );
        assert_eq!(
            corrected_search(store, "atls"),
            (none.clone(), no_correction.clone()),
            "four letters"
        );
        assert_eq!(
            corrected_search(store, "\"roadmpa\""),
            (none.clone(), no_correction.clone()),
            "a phrase"
        );
        assert_eq!(
            corrected_search(store, "budgte roadmap"),
            (vec!["a".into()], vec![corrected("budgte", &["budget"])]),
            "only the misspelt word changes"
        );
        assert_eq!(
            corrected_search(store, "roadmpa kilimanjaro budget"),
            (none.clone(), no_correction.clone()),
            "a correction that finds nothing is dropped"
        );
        assert_eq!(
            corrected_search(store, "qqqqqqq"),
            (none, no_correction),
            "no term is near"
        );
    }

    fn terms(store: &dyn Store) -> Vec<String> {
        store.vocabulary().unwrap().terms().to_vec()
    }

    fn vocabulary_follows_writes(store: &dyn Store) {
        assert_eq!(terms(store), Vec::<String>::new(), "an empty library");
        store
            .apply(vec![
                titled("r", 100, "Zanzibar offsite"),
                said("r", 1, &["The Ørsted plan", "ok so"]),
            ])
            .unwrap();
        assert_eq!(
            terms(store),
            ["offsite", "orsted", "plan", "zanzibar"],
            "folded, four letters or more"
        );
        store
            .apply(vec![Change::RecordingEdited {
                id: rid("r"),
                edit: RecordingEdit::Title(Some("Atlas kickoff".into())),
            }])
            .unwrap();
        assert_eq!(
            terms(store),
            ["atlas", "kickoff", "orsted", "plan"],
            "a rename drops the old title's words"
        );
        store
            .apply(vec![
                Change::SpeakerNamesReplaced {
                    id: rid("r"),
                    names: SpeakerNames {
                        for_revision: Revision(1),
                        names: BTreeMap::from([(SpeakerId(2), "Priyanka".to_owned())]),
                    },
                },
                Change::SummaryReplaced {
                    id: rid("r"),
                    summary: summary(1, "Decision: ship Friday"),
                },
            ])
            .unwrap();
        assert_eq!(
            terms(store),
            [
                "atlas", "decision", "friday", "kickoff", "orsted", "plan", "priyanka", "ship"
            ],
            "names and summary"
        );
        store.apply(vec![said("r", 2, &["fresh words"])]).unwrap();
        assert_eq!(
            terms(store),
            [
                "atlas", "decision", "fresh", "friday", "kickoff", "ship", "words"
            ],
            "a new transcript drops the old words and the names of its revision"
        );
        store
            .apply(vec![Change::RecordingDeleted(rid("r"))])
            .unwrap();
        assert_eq!(terms(store), Vec::<String>::new(), "a deleted recording");
    }

    fn list_row_fields(store: &dyn Store) {
        for (id, at) in [("none", 5), ("cur", 4), ("stale", 3), ("orphan", 2)] {
            create(store, id, at);
        }
        store
            .apply(vec![
                Change::TranscriptReplaced {
                    id: rid("cur"),
                    transcript: transcript(2, &["x"]),
                },
                Change::SummaryReplaced {
                    id: rid("cur"),
                    summary: summary(2, "s"),
                },
                Change::TranscriptReplaced {
                    id: rid("stale"),
                    transcript: transcript(3, &["x"]),
                },
                Change::SummaryReplaced {
                    id: rid("stale"),
                    summary: summary(2, "s"),
                },
                Change::SummaryReplaced {
                    id: rid("orphan"),
                    summary: summary(1, "s"),
                },
            ])
            .unwrap();
        enqueue(store, "cur", Goal::ALL, Priority::Normal, 1);
        enqueue(store, "stale", Goal::TRANSCRIBE, Priority::Normal, 1);
        let stale_job = only_job(store, "stale");
        event(store, stale_job.id, JobEvent::Cancelled).unwrap();
        enqueue(store, "stale", Goal::ALL, Priority::Normal, 2);

        let rows: BTreeMap<String, RecordingRow> = store
            .recordings(&RecordingQuery::default())
            .unwrap()
            .into_iter()
            .map(|r| (r.recording.id.to_string(), r))
            .collect();
        let row = |id: &str| &rows[id];
        assert_eq!(
            (
                row("none").has_transcript,
                row("none").summary,
                row("none").job.clone()
            ),
            (false, SummaryFlag::None, None)
        );
        assert_eq!(
            (
                row("cur").has_transcript,
                row("cur").summary,
                row("cur").job.clone()
            ),
            (true, SummaryFlag::Current, Some(JobState::Queued))
        );
        assert_eq!(
            (row("stale").has_transcript, row("stale").summary),
            (true, SummaryFlag::Stale)
        );
        assert_eq!(
            row("stale").job,
            Some(JobState::Queued),
            "the row shows the most recently enqueued job, not the cancelled one"
        );
        assert_eq!(
            (row("orphan").has_transcript, row("orphan").summary),
            (false, SummaryFlag::Stale)
        );
    }

    fn job_transitions(store: &dyn Store) {
        create(store, "r", 10);
        enqueue(store, "r", Goal::ALL, Priority::Normal, 100);
        let fresh = only_job(store, "r");
        assert_eq!(
            fresh,
            Job::new(
                fresh.id,
                rid("r"),
                Goal::ALL,
                Priority::Normal,
                Timestamp(100)
            )
        );
        assert_eq!(store.job(fresh.id).unwrap(), Some(fresh.clone()));

        let t = Stage::Transcribe;
        let s = Stage::Summarize;
        let blocker = Blocker::NoModel { kind: KindTag::Stt };
        let steps = [
            JobEvent::Started { stage: t },
            JobEvent::Blocked {
                stage: t,
                blocker: blocker.clone(),
            },
            JobEvent::Promote,
            JobEvent::Unblock,
            JobEvent::Started { stage: t },
            JobEvent::Retry {
                not_before: Timestamp(500),
            },
            JobEvent::Started { stage: t },
            JobEvent::StageDone { stage: t },
            JobEvent::Started { stage: s },
            JobEvent::Failed {
                stage: s,
                error: error(),
            },
            JobEvent::Retake,
            JobEvent::Started { stage: s },
            JobEvent::StageDone { stage: s },
        ];
        let mut model = fresh;
        for step in steps {
            model = model
                .on(&step)
                .unwrap_or_else(|e| panic!("script is wrong: {e}"));
            event(store, model.id, step.clone()).unwrap();
            assert_eq!(
                store.job(model.id).unwrap(),
                Some(model.clone()),
                "after {step:?} the store must hold what Job::on says"
            );
        }
        assert_eq!(model.state, JobState::Succeeded);
        assert_eq!(model.done, vec![t, s]);
        assert_eq!(store.jobs_for(&rid("r")).unwrap(), vec![model]);
    }

    fn illegal_job_event(store: &dyn Store) {
        create(store, "r", 10);
        enqueue(store, "r", Goal::ALL, Priority::Normal, 1);
        let job = only_job(store, "r");
        let result = event(store, job.id, JobEvent::Retake);
        match result {
            Err(StoreError::IllegalTransition(e)) => {
                assert_eq!(*e.from, JobState::Queued);
                assert_eq!(*e.event, JobEvent::Retake);
            }
            other => panic!("expected IllegalTransition, got {other:?}"),
        }
        assert_eq!(store.job(job.id).unwrap(), Some(job));
    }

    fn job_states_round_trip(store: &dyn Store) {
        use crate::models::{Mismatch, Unavailable, UnavailableReason};
        let t = Stage::Transcribe;
        let blockers = vec![
            Blocker::NeedsProvision {
                model: model(),
                bytes: Some(1_500_000_000),
            },
            Blocker::NeedsProvision {
                model: model(),
                bytes: None,
            },
            Blocker::Unavailable {
                model: model(),
                why: Unavailable {
                    reason: UnavailableReason::UnsupportedPlatform,
                    fix: None,
                },
            },
            Blocker::Unavailable {
                model: model(),
                why: Unavailable {
                    reason: UnavailableReason::OsTooOld {
                        required: "macOS 26".into(),
                    },
                    fix: Some("Update macOS".into()),
                },
            },
            Blocker::Unavailable {
                model: model(),
                why: Unavailable {
                    reason: UnavailableReason::NotEligible {
                        detail: "This Mac can't run Apple Intelligence".into(),
                    },
                    fix: None,
                },
            },
            Blocker::Unavailable {
                model: model(),
                why: Unavailable {
                    reason: UnavailableReason::TurnedOff {
                        detail: "Apple Intelligence is turned off".into(),
                        settings_url: "x-apple.systempreferences:com.apple.Siri-Settings.extension"
                            .into(),
                    },
                    fix: Some("Turn it on in Settings".into()),
                },
            },
            Blocker::Unavailable {
                model: model(),
                why: Unavailable {
                    reason: UnavailableReason::ServerUnreachable {
                        endpoint: "http://localhost:11434".into(),
                    },
                    fix: Some("Start \"ollama\"".into()),
                },
            },
            Blocker::Unavailable {
                model: model(),
                why: Unavailable {
                    reason: UnavailableReason::Preparing {
                        detail: "Apple is getting the model ready".into(),
                    },
                    fix: None,
                },
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::AutoLanguageUnsupported,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::LanguageUnsupported,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::LanguagesUnknown,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::OneLanguagePerMeeting,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::NoDiarization,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::InputTooLong,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::NothingRecognized,
            },
            Blocker::NoModel { kind: KindTag::Stt },
            Blocker::NoModel { kind: KindTag::Llm },
            Blocker::NoModel {
                kind: KindTag::Diar,
            },
        ];
        let mut cases: Vec<Vec<JobEvent>> = blockers
            .into_iter()
            .map(|blocker| vec![JobEvent::Blocked { stage: t, blocker }])
            .collect();
        for (class, cause) in [
            (FailureClass::Cancelled, FailureCause::Other),
            (FailureClass::Transient, FailureCause::Crashed),
            (FailureClass::NeedsUser, FailureCause::DiskFull),
            (FailureClass::Permanent, FailureCause::AudioUnreadable),
        ] {
            cases.push(vec![JobEvent::Failed {
                stage: t,
                error: JobError {
                    class,
                    message: "multi\nline \u{2018}message\u{2019}".into(),
                    cause,
                },
            }]);
        }
        cases.push(vec![JobEvent::Cancelled]);
        cases.push(vec![JobEvent::Promote, JobEvent::Cancelled]);
        cases.push(vec![JobEvent::Interrupted]);
        cases.push(vec![
            JobEvent::Interrupted,
            JobEvent::Started { stage: t },
            JobEvent::Interrupted,
            JobEvent::Started { stage: t },
            JobEvent::Interrupted,
        ]);

        for (n, tail) in cases.into_iter().enumerate() {
            let id = format!("r{n}");
            create(store, &id, 10);
            enqueue(store, &id, Goal::ALL, Priority::Normal, 7);
            let mut model = only_job(store, &id);
            let mut events = vec![JobEvent::Started { stage: t }];
            events.extend(tail);
            for step in events {
                model = model.on(&step).unwrap();
                event(store, model.id, step).unwrap();
            }
            assert_eq!(
                store.job(model.id).unwrap(),
                Some(model.clone()),
                "case {n}: {:?}",
                model.state
            );
        }
    }

    fn runnable_follows_place(store: &dyn Store) {
        for id in ["a", "b", "c"] {
            create(store, id, 1);
        }
        enqueue(store, "a", Goal::ALL, Priority::Normal, 10);
        enqueue(store, "b", Goal::ALL, Priority::Normal, 20);
        enqueue(store, "c", Goal::ALL, Priority::Normal, 30);
        let jid = |id: &str| only_job(store, id).id;
        let next = || {
            store
                .next_runnable_job(Timestamp(1_000))
                .unwrap()
                .map(|j| j.recording.to_string())
        };
        assert_eq!(
            only_job(store, "b").place,
            20,
            "a job starts at its enqueue time"
        );
        assert_eq!(next().as_deref(), Some("a"));

        event(store, jid("c"), JobEvent::Place { place: 5 }).unwrap();
        assert_eq!(only_job(store, "c").place, 5, "the place is stored");
        assert_eq!(next().as_deref(), Some("c"), "a lower place runs sooner");

        event(store, jid("a"), JobEvent::Promote).unwrap();
        assert_eq!(next().as_deref(), Some("a"), "Front still beats any place");

        event(store, jid("a"), JobEvent::Place { place: 100 }).unwrap();
        let a = only_job(store, "a");
        assert_eq!(
            (a.place, a.priority),
            (100, Priority::Normal),
            "placing a job resets its priority"
        );
        assert_eq!(next().as_deref(), Some("c"));
        event(store, jid("c"), JobEvent::Cancelled).unwrap();
        assert_eq!(next().as_deref(), Some("b"));
        event(store, jid("c"), JobEvent::Retake).unwrap();
        assert_eq!(
            next().as_deref(),
            Some("c"),
            "a cancel and retake keep the place"
        );

        let batch: Vec<Change> = [("a", 1), ("b", 2), ("c", 3)]
            .into_iter()
            .map(|(id, place)| Change::Job {
                job: jid(id),
                event: JobEvent::Place { place },
            })
            .collect();
        store.apply(batch).unwrap();
        assert_eq!(
            next().as_deref(),
            Some("a"),
            "a batch of places applies together"
        );
        let failing = vec![
            Change::Job {
                job: jid("b"),
                event: JobEvent::Place { place: 0 },
            },
            Change::Job {
                job: JobId(999),
                event: JobEvent::Place { place: 0 },
            },
        ];
        assert!(store.apply(failing).is_err());
        assert_eq!(
            only_job(store, "b").place,
            2,
            "a failed batch changes nothing"
        );
    }

    fn runnable_ordering(store: &dyn Store) {
        for id in ["x", "y", "z", "w"] {
            create(store, id, 1);
        }
        // Ids ascend in enqueue order while created_at does not, so "oldest" has to mean created_at.
        enqueue(store, "x", Goal::ALL, Priority::Normal, 50);
        enqueue(store, "y", Goal::ALL, Priority::Normal, 40);
        enqueue(store, "z", Goal::ALL, Priority::Normal, 40);
        let at = Timestamp(1_000);
        let next = || {
            store
                .next_runnable_job(at)
                .unwrap()
                .map(|j| j.recording.to_string())
        };
        assert_eq!(
            next().as_deref(),
            Some("y"),
            "oldest created_at, then lowest job id"
        );

        enqueue(store, "w", Goal::ALL, Priority::Front, 99);
        assert_eq!(
            next().as_deref(),
            Some("w"),
            "Front beats older Normal jobs"
        );

        let jid = |id: &str| only_job(store, id).id;
        event(store, jid("x"), JobEvent::Promote).unwrap();
        event(
            store,
            jid("w"),
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        assert_eq!(
            next().as_deref(),
            Some("x"),
            "a running job is not runnable, the promoted one is"
        );

        event(
            store,
            jid("x"),
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        assert_eq!(next().as_deref(), Some("y"));
        event(store, jid("y"), JobEvent::Cancelled).unwrap();
        assert_eq!(next().as_deref(), Some("z"));
        event(
            store,
            jid("z"),
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        event(
            store,
            jid("z"),
            JobEvent::Blocked {
                stage: Stage::Transcribe,
                blocker: Blocker::NoModel { kind: KindTag::Stt },
            },
        )
        .unwrap();
        assert_eq!(
            next(),
            None,
            "running, blocked, and cancelled jobs never come back"
        );
    }

    fn not_before(store: &dyn Store) {
        create(store, "a", 1);
        create(store, "b", 1);
        enqueue(store, "a", Goal::ALL, Priority::Normal, 10);
        enqueue(store, "b", Goal::ALL, Priority::Normal, 20);
        let a = only_job(store, "a").id;
        event(
            store,
            a,
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        event(
            store,
            a,
            JobEvent::Retry {
                not_before: Timestamp(100),
            },
        )
        .unwrap();

        let runnable = |now: i64| {
            store
                .next_runnable_job(Timestamp(now))
                .unwrap()
                .map(|j| j.recording.to_string())
        };
        assert_eq!(runnable(50).as_deref(), Some("b"), "a is backing off");
        assert_eq!(
            store.next_wakeup(Timestamp(50)).unwrap(),
            Some(Timestamp(100))
        );
        assert_eq!(
            store.next_wakeup(Timestamp(99)).unwrap(),
            Some(Timestamp(100))
        );
        assert_eq!(
            runnable(100).as_deref(),
            Some("a"),
            "not_before is inclusive, and a is older"
        );
        assert_eq!(
            store.next_wakeup(Timestamp(100)).unwrap(),
            None,
            "not in the future any more"
        );

        let b = only_job(store, "b").id;
        event(
            store,
            b,
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        event(
            store,
            b,
            JobEvent::Retry {
                not_before: Timestamp(300),
            },
        )
        .unwrap();
        assert_eq!(
            store.next_wakeup(Timestamp(0)).unwrap(),
            Some(Timestamp(100)),
            "the earliest wins"
        );
        event(
            store,
            a,
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        assert_eq!(
            store.next_wakeup(Timestamp(0)).unwrap(),
            Some(Timestamp(300)),
            "a running job has no wakeup"
        );
        assert_eq!(runnable(299), None);
        assert_eq!(runnable(300).as_deref(), Some("b"));
    }

    fn enqueue_idempotent(store: &dyn Store) {
        create(store, "r", 1);
        enqueue(store, "r", Goal::ALL, Priority::Normal, 10);
        let first = only_job(store, "r");
        enqueue(store, "r", Goal::ALL, Priority::Normal, 20);
        assert_eq!(only_job(store, "r"), first, "same goal, nothing new");

        store
            .apply(vec![
                Change::JobEnqueued {
                    id: rid("r"),
                    goal: Goal::ALL,
                    priority: Priority::Normal,
                    at: Timestamp(30),
                },
                Change::JobEnqueued {
                    id: rid("r"),
                    goal: Goal::ALL,
                    priority: Priority::Normal,
                    at: Timestamp(31),
                },
            ])
            .unwrap();
        assert_eq!(
            store.jobs_for(&rid("r")).unwrap().len(),
            1,
            "also within one apply"
        );

        enqueue(store, "r", Goal::ALL, Priority::Front, 40);
        let promoted = only_job(store, "r");
        assert_eq!(
            promoted.priority,
            Priority::Front,
            "a Front request promotes the waiting job"
        );
        assert_eq!(
            (promoted.id, promoted.created_at),
            (first.id, first.created_at)
        );

        enqueue(store, "r", Goal::SUMMARIZE, Priority::Normal, 50);
        let jobs = store.jobs_for(&rid("r")).unwrap();
        assert_eq!(jobs.len(), 2, "a different goal is a different job");
        assert_eq!(jobs[0].id, first.id);
        assert_eq!(jobs[1].goal, Goal::SUMMARIZE);

        enqueue(store, "r", Goal::ALL, Priority::Normal, 60);
        assert_eq!(store.jobs_for(&rid("r")).unwrap().len(), 2);

        event(
            store,
            first.id,
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
        )
        .unwrap();
        enqueue(store, "r", Goal::ALL, Priority::Normal, 70);
        assert_eq!(
            store.jobs_for(&rid("r")).unwrap().len(),
            2,
            "a running job counts as non-terminal"
        );
        event(
            store,
            first.id,
            JobEvent::Blocked {
                stage: Stage::Transcribe,
                blocker: Blocker::NoModel { kind: KindTag::Stt },
            },
        )
        .unwrap();
        enqueue(store, "r", Goal::ALL, Priority::Normal, 80);
        assert_eq!(
            store.jobs_for(&rid("r")).unwrap().len(),
            2,
            "so does a blocked one"
        );

        create(store, "other", 2);
        enqueue(store, "other", Goal::ALL, Priority::Normal, 90);
        assert_eq!(
            store.jobs_for(&rid("other")).unwrap().len(),
            1,
            "other recordings are independent"
        );
    }

    fn enqueue_after_terminal(store: &dyn Store) {
        create(store, "r", 1);
        for (round, terminal) in [JobEvent::Cancelled, JobEvent::Cancelled]
            .into_iter()
            .enumerate()
        {
            enqueue(store, "r", Goal::ALL, Priority::Normal, 10 + round as i64);
            let jobs = store.jobs_for(&rid("r")).unwrap();
            assert_eq!(jobs.len(), round + 1);
            let latest = jobs.last().unwrap();
            assert_eq!(latest.state, JobState::Queued);
            event(store, latest.id, terminal).unwrap();
        }
        enqueue(store, "r", Goal::ALL, Priority::Normal, 99);
        let jobs = store.jobs_for(&rid("r")).unwrap();
        assert_eq!(jobs.len(), 3);
        assert_eq!(
            jobs.iter().filter(|j| !j.is_terminal()).count(),
            1,
            "one live job, the earlier ones stay as history"
        );
    }

    fn job_ids_unique(store: &dyn Store) {
        create(store, "a", 1);
        create(store, "b", 1);
        enqueue(store, "a", Goal::ALL, Priority::Normal, 1);
        enqueue(store, "b", Goal::ALL, Priority::Normal, 1);
        let (a, b) = (only_job(store, "a").id, only_job(store, "b").id);
        assert!(b > a);
        store
            .apply(vec![Change::RecordingDeleted(rid("b"))])
            .unwrap();
        create(store, "c", 1);
        enqueue(store, "c", Goal::ALL, Priority::Normal, 1);
        let c = only_job(store, "c").id;
        assert!(
            c > b,
            "a deleted job's id is not handed out again: {c:?} vs {b:?}"
        );
    }

    fn concurrent_applies(store: &dyn Store) {
        const THREADS: usize = 6;
        const EACH: usize = 20;
        std::thread::scope(|scope| {
            for t in 0..THREADS {
                scope.spawn(move || {
                    for i in 0..EACH {
                        let id = format!("t{t}-{i}");
                        store
                            .apply(vec![
                                Change::RecordingCreated(recording(&id, (t * EACH + i) as i64)),
                                Change::JobEnqueued {
                                    id: rid(&id),
                                    goal: Goal::ALL,
                                    priority: Priority::Normal,
                                    at: Timestamp(i as i64),
                                },
                            ])
                            .unwrap();
                        let _ = store.recordings(&RecordingQuery::default()).unwrap();
                    }
                });
            }
        });
        let rows = store.recordings(&RecordingQuery::default()).unwrap();
        assert_eq!(rows.len(), THREADS * EACH);
        assert!(rows.iter().all(|r| r.job == Some(JobState::Queued)));
        let mut ids = Vec::new();
        for row in rows {
            ids.push(only_job(store, row.recording.id.as_str()).id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), THREADS * EACH, "every job got its own id");
    }

    fn run(id: &str, model: &str, channel: Channel) -> CheckpointKey {
        CheckpointKey {
            recording: rid(id),
            model: ModelRef::parse(model).unwrap(),
            channel,
            language: LanguagePref::Auto,
        }
    }

    fn range(a: u64, b: u64) -> TimeRange {
        TimeRange::new(Ms(a), Ms(b)).unwrap()
    }

    /// A window with one segment per text, spread evenly over it.
    fn checkpoint(start: u64, end: u64, texts: &[&str]) -> Checkpoint {
        let step = (end - start) / texts.len().max(1) as u64;
        Checkpoint {
            span: range(start, end),
            segments: texts
                .iter()
                .enumerate()
                .map(|(i, text)| RawSegment {
                    range: range(start + i as u64 * step, start + (i as u64 + 1) * step),
                    text: (*text).to_owned(),
                    speaker: None,
                    language: None,
                })
                .collect(),
            detected_language: None,
        }
    }

    fn save(store: &dyn Store, key: &CheckpointKey, checkpoint: Checkpoint) {
        store
            .apply(vec![Change::CheckpointSaved {
                key: key.clone(),
                checkpoint,
            }])
            .unwrap();
    }

    fn checkpoints_per_run(store: &dyn Store) {
        create(store, "r", 10);
        create(store, "other", 20);
        let key = run("r", "sona/whisper-small", Channel::System);
        let full = Checkpoint {
            span: range(30_000, 60_000),
            segments: vec![
                RawSegment {
                    range: range(30_500, 31_000),
                    text: "Xin ch\u{e0}o \"b\u{1ea1}n\"".into(),
                    speaker: Some(3),
                    language: Some(LangTag::parse("vi").unwrap()),
                },
                RawSegment {
                    range: range(31_000, 31_000),
                    text: String::new(),
                    speaker: Some(0),
                    language: None,
                },
            ],
            detected_language: Some(LangTag::parse("vi").unwrap()),
        };
        let empty = Checkpoint {
            span: range(90_000, 95_000),
            segments: vec![],
            detected_language: None,
        };
        save(store, &key, empty.clone());
        save(store, &key, checkpoint(0, 30_000, &["one", "two"]));
        save(store, &key, full.clone());
        assert_eq!(
            store.checkpoints(&key).unwrap(),
            vec![checkpoint(0, 30_000, &["one", "two"]), full, empty],
            "earliest window first, every field kept"
        );

        let others = [
            run("r", "sona/whisper-large", Channel::System),
            run("r", "sona/whisper-small", Channel::Mic),
            CheckpointKey {
                language: LanguagePref::Pinned(LangTag::parse("en").unwrap()),
                ..key.clone()
            },
            run("other", "sona/whisper-small", Channel::System),
        ];
        for other in &others {
            assert_eq!(store.checkpoints(other).unwrap(), vec![], "{other:?}");
        }
        save(store, &others[0], checkpoint(0, 30_000, &["large"]));
        assert_eq!(
            store.checkpoints(&others[0]).unwrap(),
            vec![checkpoint(0, 30_000, &["large"])]
        );
        assert_eq!(store.checkpoints(&key).unwrap().len(), 3);
    }

    fn checkpoint_replace(store: &dyn Store) {
        create(store, "r", 10);
        let key = run("r", "sona/whisper-small", Channel::Mic);
        save(store, &key, checkpoint(0, 30_000, &["first try"]));
        save(store, &key, checkpoint(0, 30_000, &["second", "try"]));
        save(store, &key, checkpoint(0, 29_000, &["a different span"]));
        assert_eq!(
            store.checkpoints(&key).unwrap(),
            vec![
                checkpoint(0, 29_000, &["a different span"]),
                checkpoint(0, 30_000, &["second", "try"]),
            ]
        );
    }

    fn checkpoints_cleared(store: &dyn Store) {
        create(store, "r", 10);
        create(store, "kept", 20);
        let small = run("r", "sona/whisper-small", Channel::Mic);
        let large = run("r", "sona/whisper-large", Channel::System);
        let kept = run("kept", "sona/whisper-small", Channel::Mic);
        for key in [&small, &large, &kept] {
            save(store, key, checkpoint(0, 1_000, &["x"]));
        }
        store
            .apply(vec![
                Change::CheckpointsCleared(rid("r")),
                Change::TranscriptReplaced {
                    id: rid("r"),
                    transcript: transcript(1, &["x"]),
                },
            ])
            .unwrap();
        assert_eq!(store.checkpoints(&small).unwrap(), vec![]);
        assert_eq!(store.checkpoints(&large).unwrap(), vec![]);
        assert_eq!(store.checkpoints(&kept).unwrap().len(), 1);
        store
            .apply(vec![Change::CheckpointsCleared(rid("r"))])
            .unwrap();
        store
            .apply(vec![Change::CheckpointsCleared(rid("ghost"))])
            .unwrap();
    }

    fn checkpoints_follow_the_recording(store: &dyn Store) {
        let ghost = run("ghost", "sona/whisper-small", Channel::Mic);
        assert_not_found(store.apply(vec![Change::CheckpointSaved {
            key: ghost.clone(),
            checkpoint: checkpoint(0, 1_000, &["x"]),
        }]));
        assert_eq!(store.checkpoints(&ghost).unwrap(), vec![]);

        create(store, "gone", 10);
        let key = run("gone", "sona/whisper-small", Channel::Mic);
        save(store, &key, checkpoint(0, 1_000, &["x"]));
        store
            .apply(vec![Change::RecordingDeleted(rid("gone"))])
            .unwrap();
        assert_eq!(store.checkpoints(&key).unwrap(), vec![]);
        create(store, "gone", 20);
        assert_eq!(
            store.checkpoints(&key).unwrap(),
            vec![],
            "no ghost checkpoints from the old life"
        );
    }

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn translation(revision: u32, target: &str, texts: &[Option<&str>]) -> Translation {
        Translation {
            target: tag(target),
            transcript_revision: Revision(revision),
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            texts: texts.iter().map(|t| t.map(str::to_owned)).collect(),
        }
    }

    fn translation_run(id: &str, revision: u32, target: &str, model: &str) -> TranslationKey {
        TranslationKey {
            recording: rid(id),
            revision: Revision(revision),
            target: tag(target),
            model: ModelRef::parse(model).unwrap(),
        }
    }

    fn batch(texts: &[(usize, &str)]) -> TranslationBatch {
        TranslationBatch {
            texts: texts.iter().map(|(i, t)| (*i, (*t).to_owned())).collect(),
        }
    }

    fn segment_language_round_trips(store: &dyn Store) {
        create(store, "r", 10);
        let mut t = transcript(1, &["hello", "kumusta", "unknown"]);
        t.segments[0].language = Some(tag("en"));
        t.segments[1].language = Some(tag("fil"));
        store
            .apply(vec![Change::TranscriptReplaced {
                id: rid("r"),
                transcript: t.clone(),
            }])
            .unwrap();
        let back = store.transcript(&rid("r")).unwrap().unwrap();
        let languages: Vec<Option<&str>> = back
            .segments
            .iter()
            .map(|s| s.language.as_ref().map(LangTag::as_str))
            .collect();
        assert_eq!(languages, [Some("en"), Some("fil"), None]);
        assert_eq!(back, t);
    }

    fn translation_round_trips(store: &dyn Store) {
        create(store, "r", 10);
        assert_eq!(store.translation(&rid("r")).unwrap(), None);
        let t = transcript(1, &["bonjour", "ok"]);
        let first = translation(1, "en", &[Some("hello \"there\"\nfriend"), None]);
        store
            .apply(vec![
                Change::TranscriptReplaced {
                    id: rid("r"),
                    transcript: t.clone(),
                },
                Change::TranslationReplaced {
                    id: rid("r"),
                    translation: first.clone(),
                },
            ])
            .unwrap();
        assert_eq!(store.translation(&rid("r")).unwrap(), Some(first));
        assert_eq!(
            store.transcript(&rid("r")).unwrap().unwrap(),
            t,
            "the spoken transcript is untouched"
        );

        let second = translation(1, "de", &[Some("hallo"), Some("gut")]);
        store
            .apply(vec![Change::TranslationReplaced {
                id: rid("r"),
                translation: second.clone(),
            }])
            .unwrap();
        assert_eq!(store.translation(&rid("r")).unwrap(), Some(second));
        assert_eq!(store.translation(&rid("other")).unwrap(), None);
    }

    fn translation_goes_stale_with_the_revision(store: &dyn Store) {
        create(store, "r", 10);
        store
            .apply(vec![
                Change::TranscriptReplaced {
                    id: rid("r"),
                    transcript: transcript(1, &["a", "b"]),
                },
                Change::TranslationReplaced {
                    id: rid("r"),
                    translation: translation(1, "en", &[Some("A"), Some("B")]),
                },
            ])
            .unwrap();
        store
            .apply(vec![Change::TranscriptReplaced {
                id: rid("r"),
                transcript: transcript(2, &["a", "b", "c"]),
            }])
            .unwrap();
        let kept = store.translation(&rid("r")).unwrap().unwrap();
        let current = store.transcript(&rid("r")).unwrap().unwrap();
        assert_eq!(kept.transcript_revision, Revision(1));
        assert!(!kept.applies_to(&current));
        assert_eq!(
            crate::domain::TranslationFlag::derive(
                Some((kept.transcript_revision, &kept.target)),
                Some(current.revision),
                Some(&tag("en")),
            ),
            crate::domain::TranslationFlag::Stale
        );
    }

    fn search_finds_the_translation_while_it_applies(store: &dyn Store) {
        store
            .apply(vec![
                titled("r", 10, "Weekly sync"),
                said("r", 1, &["Aprubado na ang badyet.", "Okay."]),
                Change::TranslationReplaced {
                    id: rid("r"),
                    translation: translation(1, "en", &[Some("The budget is approved."), None]),
                },
            ])
            .unwrap();
        assert_eq!(found(store, "approved"), ["r"], "a translated word");
        assert_eq!(
            found(store, "\"budget is approved\""),
            ["r"],
            "a translated phrase"
        );
        assert_eq!(found(store, "aprubado"), ["r"], "the spoken words stay");
        assert_eq!(
            found(store, "okay approved"),
            ["r"],
            "spoken and translated together"
        );
        assert!(
            store
                .vocabulary()
                .unwrap()
                .terms()
                .iter()
                .any(|t| t == "approved"),
            "a correction can reach a translated word"
        );

        store
            .apply(vec![Change::TranslationReplaced {
                id: rid("r"),
                translation: translation(1, "fr", &[Some("Le budget est validé."), None]),
            }])
            .unwrap();
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "a replaced translation"
        );
        assert_eq!(found(store, "valide"), ["r"], "the new translation");

        store
            .apply(vec![said("r", 2, &["Aprubado na ang badyet.", "Okay."])])
            .unwrap();
        assert_eq!(
            found(store, "valide"),
            Vec::<String>::new(),
            "a translation of another revision no longer applies"
        );
        assert_eq!(found(store, "aprubado"), ["r"]);

        store
            .apply(vec![Change::TranslationReplaced {
                id: rid("r"),
                translation: translation(1, "en", &[Some("The budget is approved."), None]),
            }])
            .unwrap();
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "a translation stored for an older revision is not indexed"
        );
        store
            .apply(vec![Change::TranslationReplaced {
                id: rid("r"),
                translation: translation(2, "en", &[Some("The budget is approved.")]),
            }])
            .unwrap();
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "a translation with the wrong number of segments is not indexed"
        );
        store
            .apply(vec![Change::TranslationReplaced {
                id: rid("r"),
                translation: translation(2, "en", &[Some("The budget is approved."), None]),
            }])
            .unwrap();
        assert_eq!(
            found(store, "approved"),
            ["r"],
            "the current revision's translation"
        );

        store
            .apply(vec![Change::RecordingDeleted(rid("r"))])
            .unwrap();
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "after delete"
        );
    }

    fn assistant_search_reads_the_translation_as_transcript(store: &dyn Store) {
        use crate::domain::{AssistantQuery, Fields};
        store
            .apply(vec![
                titled("r", 10, "Weekly sync"),
                said("r", 1, &["Aprubado na ang badyet."]),
                Change::TranslationReplaced {
                    id: rid("r"),
                    translation: translation(1, "en", &[Some("The budget is approved.")]),
                },
            ])
            .unwrap();
        let found_with = |fields: Fields| -> Vec<String> {
            store
                .assistant_recordings(&AssistantQuery {
                    query: Some(Query::parse("approved").unwrap()),
                    fields,
                    since: None,
                    until: None,
                    after: None,
                    limit: 10,
                })
                .unwrap()
                .into_iter()
                .map(|f| f.row.recording.id.to_string())
                .collect()
        };
        let transcripts = Fields {
            title: true,
            speakers: true,
            summary: true,
            transcript: true,
        };
        let summaries = Fields {
            transcript: false,
            ..transcripts
        };
        assert_eq!(found_with(transcripts), ["r"]);
        assert_eq!(found_with(summaries), Vec::<String>::new());
        let has = |fields: Fields| {
            store
                .assistant_vocabulary(fields)
                .unwrap()
                .terms()
                .iter()
                .any(|t| t == "approved")
        };
        assert!(
            has(transcripts),
            "a transcript grant may be corrected toward it"
        );
        assert!(!has(summaries), "a summaries grant never sees it");
    }

    fn translation_batches_per_run(store: &dyn Store) {
        create(store, "r", 10);
        create(store, "other", 20);
        let key = translation_run("r", 1, "en", "ollama/gemma4:e4b");
        let save = |key: &TranslationKey, batch: TranslationBatch| {
            store
                .apply(vec![Change::TranslationBatchSaved {
                    key: key.clone(),
                    batch,
                }])
                .unwrap();
        };
        save(&key, batch(&[(4, "four"), (5, "five \"q\"")]));
        save(&key, batch(&[(0, "zero")]));
        assert_eq!(
            store.translation_batches(&key).unwrap(),
            vec![
                batch(&[(4, "four"), (5, "five \"q\"")]),
                batch(&[(0, "zero")])
            ],
            "save order, not index order"
        );
        let others = [
            translation_run("r", 2, "en", "ollama/gemma4:e4b"),
            translation_run("r", 1, "fr", "ollama/gemma4:e4b"),
            translation_run("r", 1, "en", "ollama/other"),
            translation_run("other", 1, "en", "ollama/gemma4:e4b"),
        ];
        for other in &others {
            assert_eq!(
                store.translation_batches(other).unwrap(),
                vec![],
                "{other:?}"
            );
        }
        save(&others[3], batch(&[(1, "x")]));
        store
            .apply(vec![Change::TranslationBatchesCleared(rid("r"))])
            .unwrap();
        assert_eq!(store.translation_batches(&key).unwrap(), vec![]);
        assert_eq!(store.translation_batches(&others[3]).unwrap().len(), 1);
        store
            .apply(vec![Change::TranslationBatchesCleared(rid("ghost"))])
            .unwrap();
    }

    fn translation_follows_the_recording(store: &dyn Store) {
        let key = translation_run("ghost", 1, "en", "ollama/gemma4:e4b");
        assert_not_found(store.apply(vec![Change::TranslationBatchSaved {
            key: key.clone(),
            batch: batch(&[(0, "x")]),
        }]));
        assert_not_found(store.apply(vec![Change::TranslationReplaced {
            id: rid("ghost"),
            translation: translation(1, "en", &[Some("x")]),
        }]));

        create(store, "gone", 10);
        let key = translation_run("gone", 1, "en", "ollama/gemma4:e4b");
        store
            .apply(vec![
                Change::TranslationBatchSaved {
                    key: key.clone(),
                    batch: batch(&[(0, "x")]),
                },
                Change::TranslationReplaced {
                    id: rid("gone"),
                    translation: translation(1, "en", &[Some("x")]),
                },
            ])
            .unwrap();
        store
            .apply(vec![Change::RecordingDeleted(rid("gone"))])
            .unwrap();
        create(store, "gone", 20);
        assert_eq!(store.translation(&rid("gone")).unwrap(), None);
        assert_eq!(store.translation_batches(&key).unwrap(), vec![]);
    }

    fn summary_translation_of(target: &str, markdown: &str) -> SummaryTranslation {
        SummaryTranslation {
            target: tag(target),
            produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
            markdown: markdown.into(),
        }
    }

    fn want(store: &dyn Store, id: &str, target: &str) {
        store
            .apply(vec![Change::SummaryTranslationWanted {
                id: rid(id),
                target: tag(target),
            }])
            .unwrap();
    }

    fn store_translation(store: &dyn Store, id: &str, translation: &SummaryTranslation) {
        store
            .apply(vec![Change::SummaryTranslated {
                id: rid(id),
                translation: translation.clone(),
            }])
            .unwrap();
    }

    /// A recording with a summary and a stored translation of it into `en`.
    fn translated_summary(store: &dyn Store, id: &str) -> SummaryTranslation {
        create(store, id, 10);
        store
            .apply(vec![Change::SummaryReplaced {
                id: rid(id),
                summary: summary(1, "## Mga punto\n- Sige"),
            }])
            .unwrap();
        want(store, id, "en");
        let done = summary_translation_of("en", "## Points\n- \"Okay\"\n\n- Fine");
        store_translation(store, id, &done);
        done
    }

    fn search_finds_the_summary_translation(store: &dyn Store) {
        store
            .apply(vec![
                titled("r", 10, "Weekly sync"),
                Change::SummaryReplaced {
                    id: rid("r"),
                    summary: summary(1, "## Mga punto\n- Aprubado na ang badyet"),
                },
            ])
            .unwrap();
        want(store, "r", "en");
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "a wish alone indexes nothing"
        );
        store_translation(
            store,
            "r",
            &summary_translation_of("en", "## Points\n- The budget is **approved**"),
        );
        assert_eq!(found(store, "approved"), ["r"], "a translated word");
        assert_eq!(
            found(store, "\"budget is approved\""),
            ["r"],
            "a translated phrase"
        );
        assert_eq!(found(store, "aprubado"), ["r"], "the written words stay");
        assert_eq!(
            found(store, "badyet approved"),
            ["r"],
            "written and translated together"
        );
        assert!(
            store
                .vocabulary()
                .unwrap()
                .terms()
                .iter()
                .any(|t| t == "approved"),
            "a correction can reach a translated word"
        );

        want(store, "r", "de");
        store_translation(
            store,
            "r",
            &summary_translation_of("de", "## Punkte\n- Das Budget ist genehmigt"),
        );
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "a replaced translation"
        );
        assert_eq!(found(store, "genehmigt"), ["r"], "the new translation");

        store
            .apply(vec![Change::SummaryReplaced {
                id: rid("r"),
                summary: summary(1, "## Mga punto\n- Tapos na"),
            }])
            .unwrap();
        assert_eq!(
            found(store, "genehmigt"),
            Vec::<String>::new(),
            "a summary written again drops its translation"
        );
        assert_eq!(found(store, "tapos"), ["r"]);

        want(store, "r", "en");
        store_translation(
            store,
            "r",
            &summary_translation_of("en", "## Points\n- It is finished"),
        );
        assert_eq!(found(store, "finished"), ["r"]);
        store
            .apply(vec![Change::SummaryRestored(rid("r"))])
            .unwrap();
        assert_eq!(
            found(store, "finished"),
            Vec::<String>::new(),
            "a restored summary drops its translation"
        );
        assert_eq!(found(store, "aprubado"), ["r"], "the restored summary");

        want(store, "r", "en");
        store_translation(
            store,
            "r",
            &summary_translation_of("en", "## Points\n- The budget is approved"),
        );
        store
            .apply(vec![Change::RecordingDeleted(rid("r"))])
            .unwrap();
        assert_eq!(
            found(store, "approved"),
            Vec::<String>::new(),
            "after delete"
        );
    }

    fn assistant_search_reads_the_summary_translation_as_summary(store: &dyn Store) {
        use crate::domain::{AssistantQuery, Fields};
        store
            .apply(vec![
                titled("r", 10, "Weekly sync"),
                Change::SummaryReplaced {
                    id: rid("r"),
                    summary: summary(1, "## Mga punto\n- Aprubado na ang badyet"),
                },
            ])
            .unwrap();
        want(store, "r", "en");
        store_translation(
            store,
            "r",
            &summary_translation_of("en", "## Points\n- The budget is approved"),
        );
        let found_with = |fields: Fields| -> Vec<String> {
            store
                .assistant_recordings(&AssistantQuery {
                    query: Some(Query::parse("approved").unwrap()),
                    fields,
                    since: None,
                    until: None,
                    after: None,
                    limit: 10,
                })
                .unwrap()
                .into_iter()
                .map(|f| f.row.recording.id.to_string())
                .collect()
        };
        let summaries = Fields {
            title: true,
            speakers: true,
            summary: true,
            transcript: false,
        };
        let titles = Fields {
            speakers: false,
            summary: false,
            ..summaries
        };
        assert_eq!(found_with(summaries), ["r"]);
        assert_eq!(found_with(titles), Vec::<String>::new());
        let has = |fields: Fields| {
            store
                .assistant_vocabulary(fields)
                .unwrap()
                .terms()
                .iter()
                .any(|t| t == "approved")
        };
        assert!(
            has(summaries),
            "a summaries grant may be corrected toward it"
        );
        assert!(!has(titles), "a titles grant never sees it");
    }

    fn summary_translation_round_trips(store: &dyn Store) {
        create(store, "r", 10);
        let id = rid("r");
        assert_eq!(store.summary_translation(&id).unwrap(), None);
        assert_eq!(store.summary_translation_wanted(&id).unwrap(), None);
        store
            .apply(vec![Change::SummaryReplaced {
                id: id.clone(),
                summary: summary(1, "## Mga punto\n- Sige"),
            }])
            .unwrap();

        want(store, "r", "en");
        assert_eq!(
            store.summary_translation_wanted(&id).unwrap(),
            Some(tag("en"))
        );
        assert_eq!(store.summary_translation(&id).unwrap(), None);
        let english = summary_translation_of("en", "## Points\n- \"Okay\"\n\n- Fine");
        store_translation(store, "r", &english);
        assert_eq!(
            store.summary_translation(&id).unwrap(),
            Some(english.clone())
        );
        assert_eq!(
            store.summary_translation_wanted(&id).unwrap(),
            None,
            "storing it ends the wish"
        );
        assert_eq!(
            store.summary(&id).unwrap().unwrap().markdown,
            "## Mga punto\n- Sige",
            "the summary is untouched"
        );

        want(store, "r", "de");
        assert_eq!(
            store.summary_translation(&id).unwrap(),
            Some(english),
            "a new wish leaves the stored translation in place"
        );
        let german = summary_translation_of("de", "## Punkte\n- Gut");
        store_translation(store, "r", &german);
        assert_eq!(
            store.summary_translation(&id).unwrap(),
            Some(german),
            "the new translation replaces the old"
        );
        assert_eq!(store.summary_translation(&rid("other")).unwrap(), None);
        assert_not_found(store.apply(vec![Change::SummaryTranslationWanted {
            id: rid("ghost"),
            target: tag("en"),
        }]));
        assert_not_found(store.apply(vec![Change::SummaryTranslated {
            id: rid("ghost"),
            translation: summary_translation_of("en", "x"),
        }]));
    }

    fn summary_translation_needs_its_wish(store: &dyn Store) {
        create(store, "r", 10);
        let id = rid("r");
        store
            .apply(vec![Change::SummaryReplaced {
                id: id.clone(),
                summary: summary(1, "## Mga punto"),
            }])
            .unwrap();
        store_translation(store, "r", &summary_translation_of("en", "no wish"));
        assert_eq!(store.summary_translation(&id).unwrap(), None);
        want(store, "r", "de");
        store_translation(
            store,
            "r",
            &summary_translation_of("en", "another language"),
        );
        assert_eq!(store.summary_translation(&id).unwrap(), None);
        assert_eq!(
            store.summary_translation_wanted(&id).unwrap(),
            Some(tag("de"))
        );

        want(store, "r", "en");
        store
            .apply(vec![Change::SummaryReplaced {
                id: id.clone(),
                summary: summary(1, "## Written while translating"),
            }])
            .unwrap();
        store_translation(
            store,
            "r",
            &summary_translation_of("en", "of the old summary"),
        );
        assert_eq!(
            store.summary_translation(&id).unwrap(),
            None,
            "a translation of a replaced summary is never stored"
        );
        assert_eq!(store.summary_translation_wanted(&id).unwrap(), None);
    }

    fn a_new_summary_drops_its_translation(store: &dyn Store) {
        translated_summary(store, "r");
        want(store, "r", "fr");
        let id = rid("r");
        store
            .apply(vec![Change::SummaryReplaced {
                id: id.clone(),
                summary: summary(1, "## Nouveau"),
            }])
            .unwrap();
        assert_eq!(store.summary_translation(&id).unwrap(), None);
        assert_eq!(store.summary_translation_wanted(&id).unwrap(), None);
        assert_eq!(store.summary(&id).unwrap().unwrap().markdown, "## Nouveau");
    }

    fn restoring_a_summary_drops_its_translation(store: &dyn Store) {
        translated_summary(store, "r");
        let id = rid("r");
        store
            .apply(vec![Change::SummaryReplaced {
                id: id.clone(),
                summary: summary(1, "## Second"),
            }])
            .unwrap();
        let again = summary_translation_of("en", "## Second, in English");
        want(store, "r", "en");
        store_translation(store, "r", &again);
        assert_eq!(store.summary_translation(&id).unwrap(), Some(again));
        want(store, "r", "de");
        store
            .apply(vec![Change::SummaryRestored(id.clone())])
            .unwrap();
        assert_eq!(
            store.summary(&id).unwrap().unwrap().markdown,
            "## Mga punto\n- Sige"
        );
        assert_eq!(store.summary_translation(&id).unwrap(), None);
        assert_eq!(store.summary_translation_wanted(&id).unwrap(), None);

        let none_to_restore = rid("n");
        create(store, "n", 20);
        store
            .apply(vec![Change::SummaryReplaced {
                id: none_to_restore.clone(),
                summary: summary(1, "## Only"),
            }])
            .unwrap();
        want(store, "n", "en");
        store_translation(store, "n", &summary_translation_of("en", "## Only"));
        assert!(
            store
                .apply(vec![Change::SummaryRestored(none_to_restore.clone())])
                .is_err()
        );
        assert!(
            store
                .summary_translation(&none_to_restore)
                .unwrap()
                .is_some(),
            "a restore that finds nothing to restore changes nothing"
        );
    }

    fn deleting_a_recording_drops_its_translation(store: &dyn Store) {
        translated_summary(store, "gone");
        want(store, "gone", "fr");
        store
            .apply(vec![Change::RecordingDeleted(rid("gone"))])
            .unwrap();
        create(store, "gone", 30);
        assert_eq!(store.summary_translation(&rid("gone")).unwrap(), None);
        assert_eq!(
            store.summary_translation_wanted(&rid("gone")).unwrap(),
            None
        );
        assert_eq!(store.summary_translation(&rid("gone")).unwrap(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_store_passes_the_store_contract() {
        run_store_contract(|| Box::new(MemStore::new()));
    }

    #[test]
    fn every_case_has_a_unique_name() {
        let mut names: Vec<_> = CASES.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
    }
}
