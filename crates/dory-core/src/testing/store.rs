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
//! - `recordings` lists newest first (ties broken by id). A text query matches, case-insensitively,
//!   the title, the transcript text, or the summary. The row's job is the most recently enqueued
//!   one.
//! - `next_runnable_job` is Front first, then lowest `place`, then lowest job id. A job's `place`
//!   starts as its enqueue time and changes only through `JobEvent::Place`.
//! - Checkpoints are read per run (recording, model, channel, language), earliest window first. A
//!   second save of the same run and span replaces the first. Clearing drops every run of one
//!   recording, and so does deleting the recording. Saving for a missing recording is `NotFound`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, PoisonError};

use crate::domain::{
    Job, JobEvent, JobId, JobState, Ms, Priority, Recording, RecordingEdit, RecordingId,
    RecordingQuery, RecordingRow, Settings, SpeakerNames, Summary, SummaryFlag, Timestamp,
    Transcript,
};
use crate::ports::{Change, Checkpoint, CheckpointKey, Store, StoreError, StoreRead};

#[derive(Clone, Debug)]
struct Entry {
    recording: Recording,
    transcript: Option<Transcript>,
    summary: Option<Summary>,
    names: SpeakerNames,
}

#[derive(Clone, Debug)]
struct State {
    recordings: BTreeMap<RecordingId, Entry>,
    jobs: BTreeMap<JobId, Job>,
    last_job: u64,
    settings: Settings,
    checkpoints: HashMap<CheckpointKey, BTreeMap<(Ms, Ms), Checkpoint>>,
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
            }
            Change::TranscriptReplaced { id, transcript } => {
                self.entry_mut(&id)?.transcript = Some(transcript);
            }
            Change::SpeakerNamesReplaced { id, names } => self.entry_mut(&id)?.names = names,
            Change::SummaryReplaced { id, summary } => {
                self.entry_mut(&id)?.summary = Some(summary);
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

fn matches_text(entry: &Entry, needle: &str) -> bool {
    let title = entry.recording.title.as_deref().unwrap_or_default();
    let transcript = entry
        .transcript
        .iter()
        .flat_map(|t| t.segments.iter())
        .any(|s| s.text.to_lowercase().contains(needle));
    let summary = entry
        .summary
        .as_ref()
        .is_some_and(|s| s.markdown.to_lowercase().contains(needle));
    title.to_lowercase().contains(needle) || transcript || summary
}

impl StoreRead for MemStore {
    fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        Ok(self.state().recordings.get(id).map(|e| e.recording.clone()))
    }

    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError> {
        let state = self.state();
        let needle = query
            .text
            .as_deref()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty());
        let mut entries: Vec<&Entry> = state
            .recordings
            .values()
            .filter(|e| needle.as_deref().is_none_or(|n| matches_text(e, n)))
            .collect();
        entries.sort_by(|a, b| {
            b.recording
                .created_at
                .cmp(&a.recording.created_at)
                .then_with(|| a.recording.id.cmp(&b.recording.id))
        });
        if let Some(limit) = query.limit {
            entries.truncate(limit as usize);
        }
        Ok(entries
            .into_iter()
            .map(|e| {
                let job = state
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
            })
            .collect())
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
        LanguagePref, Ms, Processing, Revision, Segment, SpeakerId, Stage, TakeContainer, TakeInfo,
        TimeRange,
    };
    use crate::models::{Choice, KindTag, ModelRef};
    use crate::ports::{Checkpoint, CheckpointKey, RawSegment};

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
            "list_is_newest_first_with_limit_and_text_filter",
            list_order_limit_and_filter,
        ),
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
            auto_process: false,
            processing: Processing::Light,
        };
        let off = Settings {
            stt: Choice::Auto,
            llm: Choice::Off,
            diar: Choice::Auto,
            language: LanguagePref::Auto,
            meeting_languages: Vec::new(),
            auto_process: true,
            processing: Processing::Fast,
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
                Change::RecordingCreated(Recording {
                    title: Some("Budget review".into()),
                    ..recording("b", 200)
                }),
                Change::RecordingCreated(recording("a", 300)),
                Change::RecordingCreated(recording("d", 100)),
                Change::RecordingCreated(recording("c", 100)),
                Change::TranscriptReplaced {
                    id: rid("c"),
                    transcript: transcript(1, &["We discussed the ROADMAP for Q3"]),
                },
                Change::SummaryReplaced {
                    id: rid("d"),
                    summary: summary(0, "Decision: ship the Beta on Friday"),
                },
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
        assert_eq!(ids(RecordingQuery::default()), ["a", "b", "c", "d"]);
        assert_eq!(
            ids(RecordingQuery {
                limit: Some(2),
                ..RecordingQuery::default()
            }),
            ["a", "b"]
        );
        let text = |t: &str| RecordingQuery {
            text: Some(t.into()),
            limit: None,
        };
        assert_eq!(ids(text("budget")), ["b"], "title, any case");
        assert_eq!(ids(text("  Roadmap  ")), ["c"], "transcript, trimmed");
        assert_eq!(ids(text("BETA")), ["d"], "summary");
        assert_eq!(ids(text("nothing like this")), Vec::<String>::new());
        assert_eq!(
            ids(text("")),
            ["a", "b", "c", "d"],
            "blank text filters nothing"
        );
        assert_eq!(
            ids(RecordingQuery {
                text: Some("e".into()),
                limit: Some(1),
            }),
            ["b"],
            "limit applies after the filter and the order"
        );

        store
            .apply(vec![Change::RecordingEdited {
                id: rid("a"),
                edit: RecordingEdit::Title(Some("Sprint planning".into())),
            }])
            .unwrap();
        assert_eq!(ids(text("sprint")), ["a"], "a renamed title is searchable");
        store
            .apply(vec![Change::TranscriptReplaced {
                id: rid("c"),
                transcript: transcript(2, &["fresh words"]),
            }])
            .unwrap();
        assert_eq!(
            ids(text("roadmap")),
            Vec::<String>::new(),
            "a replaced transcript is not searchable"
        );
        assert_eq!(ids(text("fresh")), ["c"]);
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
                        detail: "Apple Intelligence is off".into(),
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
                why: Mismatch::NoDiarization,
            },
            Blocker::Incompatible {
                model: model(),
                why: Mismatch::InputTooLong,
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
