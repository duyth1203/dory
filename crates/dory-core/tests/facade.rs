//! Drives `Dory` through its public facade with a fake for every port.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use dory_core::app::{
    Dory, DoryError, Event, OpenError, Options, Ports, RecorderStatus, StartOptions,
};
use dory_core::capture::FrameSink;
use dory_core::domain::{
    AssistantAccess, AssistantClient, AssistantQuery, Blocker, Channel, ChannelLayout, ClientKey,
    Fields, Goal, Job, JobEvent, JobId, JobState, LangTag, Levels, Ms, Priority, Recording,
    RecordingEdit, RecordingId, RecordingQuery, RecordingRow, Revision, Segment, Settings,
    SpeakerId, SpeakerNames, Stage, Summary, SummaryTranslation, TakeContainer, TakeInfo,
    TimeRange, Timestamp, Transcript, Translation, Waveform,
};
use dory_core::models::{
    Availability, CatalogError, Choice, DiarizationSupport, KindTag, Llm, ModelEntry, ModelId,
    ModelRef, ProviderId, Stt,
};
use dory_core::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, CaptureHandle, CaptureSpec, InputDevice,
    Source,
};
use dory_core::ports::{
    CancelToken, Change, Checkpoint, CheckpointKey, Clip, EngineError, LegacyFile, ModelProvider,
    PlayState, ProvisionError, ProvisionSink, Store, StoreError, StoreRead, TakeError, TakeWriter,
    Takes, Transcriber, TranslationBatch, TranslationKey,
};
use dory_core::recorder::{FinalizeReason, Limits};
use dory_core::search::{Found, Query, Source as Where, Vocabulary};
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakePlayback, FakeSttProvider,
    FakeTranscriber, FakeTrash, ManualClock, MemTakes, Step, block_on, eventually, llm_entry,
    stt_entry,
};

const RATE: u32 = 8_000;
const STT: &str = "fake/stt";
const LLM: &str = "fake/llm";
const SUMMARY: &str = "## Key points\n- Ship it Friday.";

struct Rig {
    store: Arc<MemStore>,
    takes: MemTakes,
    /// What `Dory` sees: `store` and `takes` behind hooks a test can install.
    hooked_store: Arc<HookedStore>,
    hooked_takes: Arc<HookedTakes>,
    trash: Arc<FakeTrash>,
    /// Holds the take markers and the fake trash.
    _scratch: tempfile::TempDir,
    capture: Arc<FakeCapture>,
    playback: Arc<FakePlayback>,
    clock: Arc<ManualClock>,
    transcriber: Arc<FakeTranscriber>,
    llm: Arc<FakeLanguageModel>,
    stt: Arc<FakeSttProvider>,
    llm_provider: Arc<FakeLlmProvider>,
}

impl Rig {
    fn new() -> Rig {
        let transcriber = Arc::new(
            FakeTranscriber::new()
                .with_segments(
                    Channel::Mic,
                    &[
                        (0, 2_000, 0, "Let's start."),
                        (12_000, 14_000, 0, "Ship it Friday."),
                    ],
                )
                .with_segments(
                    Channel::System,
                    &[
                        (3_000, 5_000, 4, "Agreed."),
                        (6_000, 8_000, 9, "I'll take notes."),
                        (15_000, 16_000, 4, "Bye."),
                    ],
                )
                .detecting("vi"),
        );
        let llm = Arc::new(FakeLanguageModel::new(SUMMARY));
        let store = Arc::new(MemStore::new());
        let scratch = tempfile::tempdir().unwrap();
        let (takes_dir, trash_dir) = (scratch.path().join("takes"), scratch.path().join("trash"));
        std::fs::create_dir_all(&takes_dir).unwrap();
        std::fs::create_dir_all(&trash_dir).unwrap();
        let takes = MemTakes::in_dir(takes_dir);
        Rig {
            trash: Arc::new(FakeTrash::new(trash_dir)),
            _scratch: scratch,
            hooked_store: Arc::new(HookedStore {
                inner: store.clone(),
                before: Mutex::new(None),
                after: Mutex::new(None),
            }),
            hooked_takes: Arc::new(HookedTakes {
                inner: takes.clone(),
                seal: Mutex::new(None),
                adopt: Mutex::new(None),
            }),
            store,
            takes,
            capture: Arc::new(FakeCapture::new()),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(1_763_300_000_000))),
            stt: Arc::new(FakeSttProvider::new(
                "fake",
                vec![stt_entry(STT, DiarizationSupport::Native)],
                transcriber.clone(),
            )),
            llm_provider: Arc::new(FakeLlmProvider::new(
                "fake",
                vec![llm_entry(LLM)],
                llm.clone(),
            )),
            transcriber,
            llm,
        }
    }

    fn ports(&self) -> Ports {
        Ports {
            store: self.hooked_store.clone(),
            takes: self.hooked_takes.clone(),
            trash: self.trash.clone(),
            capture: self.capture.clone(),
            playback: self.playback.clone(),
            clock: self.clock.clone(),
            stt: vec![self.stt.clone()],
            llm: vec![self.llm_provider.clone()],
            diar: vec![],
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        }
    }

    fn open(&self) -> Dory {
        self.open_with(options())
    }

    fn open_with(&self, options: Options) -> Dory {
        Dory::open(self.ports(), options).expect("opens")
    }

    /// A finished, sealed recording, as if recorded earlier.
    fn seed(&self, name: &str) -> RecordingId {
        let id = RecordingId::parse(name).unwrap();
        let mut writer = self
            .takes
            .begin(&id, ChannelLayout::MicSystem, RATE)
            .unwrap();
        writer.write(&vec![0.0; RATE as usize * 2 * 20]).unwrap();
        writer.finish().unwrap();
        let take = self.takes.seal(&id).unwrap();
        self.store
            .apply(vec![Change::RecordingCreated(recording(&id, Some(take)))])
            .unwrap();
        id
    }
}

type ApplyHook = Arc<dyn Fn(&[Change]) -> Result<(), StoreError> + Send + Sync>;
type SealHook = Arc<dyn Fn(&RecordingId) -> Result<(), TakeError> + Send + Sync>;
type AdoptHook = SealHook;

/// A `MemStore` whose applies a test can watch, hold, or fail. `before` runs ahead of each apply
/// and an error from it fails the apply. `after` runs once an apply committed.
struct HookedStore {
    inner: Arc<MemStore>,
    before: Mutex<Option<ApplyHook>>,
    after: Mutex<Option<ApplyHook>>,
}

impl HookedStore {
    fn before(&self, hook: impl Fn(&[Change]) -> Result<(), StoreError> + Send + Sync + 'static) {
        *self.before.lock().unwrap() = Some(Arc::new(hook));
    }

    fn after(&self, hook: impl Fn(&[Change]) + Send + Sync + 'static) {
        *self.after.lock().unwrap() = Some(Arc::new(move |changes: &[Change]| {
            hook(changes);
            Ok(())
        }));
    }
}

impl StoreRead for HookedStore {
    fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        self.inner.recording(id)
    }
    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError> {
        self.inner.recordings(query)
    }
    fn search(&self, query: &Query) -> Result<Vec<Found>, StoreError> {
        self.inner.search(query)
    }
    fn vocabulary(&self) -> Result<Arc<Vocabulary>, StoreError> {
        self.inner.vocabulary()
    }
    fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError> {
        self.inner.transcript(id)
    }
    fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.inner.summary(id)
    }
    fn translation(&self, id: &RecordingId) -> Result<Option<Translation>, StoreError> {
        self.inner.translation(id)
    }
    fn summary_translation(
        &self,
        id: &RecordingId,
    ) -> Result<Option<SummaryTranslation>, StoreError> {
        self.inner.summary_translation(id)
    }
    fn summary_translation_wanted(&self, id: &RecordingId) -> Result<Option<LangTag>, StoreError> {
        self.inner.summary_translation_wanted(id)
    }
    fn translation_batches(
        &self,
        key: &TranslationKey,
    ) -> Result<Vec<TranslationBatch>, StoreError> {
        self.inner.translation_batches(key)
    }
    fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError> {
        self.inner.speaker_names(id)
    }
    fn job(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.inner.job(id)
    }
    fn jobs_for(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError> {
        self.inner.jobs_for(id)
    }
    fn next_runnable_job(&self, now: Timestamp) -> Result<Option<Job>, StoreError> {
        self.inner.next_runnable_job(now)
    }
    fn next_wakeup(&self, now: Timestamp) -> Result<Option<Timestamp>, StoreError> {
        self.inner.next_wakeup(now)
    }
    fn settings(&self) -> Result<Settings, StoreError> {
        self.inner.settings()
    }
    fn checkpoints(&self, key: &CheckpointKey) -> Result<Vec<Checkpoint>, StoreError> {
        self.inner.checkpoints(key)
    }
    fn assistant_access(&self) -> Result<AssistantAccess, StoreError> {
        self.inner.assistant_access()
    }
    fn assistant_client(&self, key: &ClientKey) -> Result<Option<AssistantClient>, StoreError> {
        self.inner.assistant_client(key)
    }
    fn hidden_from_assistants(&self, id: &RecordingId) -> Result<bool, StoreError> {
        self.inner.hidden_from_assistants(id)
    }
    fn assistant_recordings(&self, query: &AssistantQuery) -> Result<Vec<Found>, StoreError> {
        self.inner.assistant_recordings(query)
    }
    fn assistant_vocabulary(&self, fields: Fields) -> Result<Vocabulary, StoreError> {
        self.inner.assistant_vocabulary(fields)
    }
    fn previous_summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.inner.previous_summary(id)
    }
    fn summary_instruction(&self, id: &RecordingId) -> Result<Option<String>, StoreError> {
        self.inner.summary_instruction(id)
    }
}

impl Store for HookedStore {
    fn apply(&self, changes: Vec<Change>) -> Result<(), StoreError> {
        // Cloned out so a hook that blocks never holds the lock other appliers need.
        let before = self.before.lock().unwrap().clone();
        let after = self.after.lock().unwrap().clone();
        if let Some(hook) = before {
            hook(&changes)?;
        }
        self.inner.apply(changes.clone())?;
        if let Some(hook) = after {
            hook(&changes)?;
        }
        Ok(())
    }
}

/// `MemTakes` whose `seal` and `adopt` a test can hold or fail.
struct HookedTakes {
    inner: MemTakes,
    seal: Mutex<Option<SealHook>>,
    adopt: Mutex<Option<AdoptHook>>,
}

impl HookedTakes {
    fn on_seal(
        &self,
        hook: impl Fn(&RecordingId) -> Result<(), TakeError> + Send + Sync + 'static,
    ) {
        *self.seal.lock().unwrap() = Some(Arc::new(hook));
    }

    fn on_adopt(
        &self,
        hook: impl Fn(&RecordingId) -> Result<(), TakeError> + Send + Sync + 'static,
    ) {
        *self.adopt.lock().unwrap() = Some(Arc::new(hook));
    }
}

impl Takes for HookedTakes {
    fn begin(
        &self,
        id: &RecordingId,
        layout: ChannelLayout,
        rate_hz: u32,
    ) -> Result<Box<dyn TakeWriter>, TakeError> {
        self.inner.begin(id, layout, rate_hz)
    }
    fn seal(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        let hook = self.seal.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(id)?;
        }
        self.inner.seal(id)
    }
    fn clip(
        &self,
        id: &RecordingId,
        channel: Channel,
        span: Option<TimeRange>,
    ) -> Result<Clip, TakeError> {
        self.inner.clip(id, channel, span)
    }
    fn slice(&self, whole: &Clip, span: TimeRange) -> Result<Clip, TakeError> {
        self.inner.slice(whole, span)
    }
    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError> {
        self.inner.orphans()
    }
    fn stored(&self) -> Result<Vec<RecordingId>, TakeError> {
        self.inner.stored()
    }
    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError> {
        self.inner.inspect(id)
    }
    fn import(&self, id: &RecordingId, source: &Path) -> Result<TakeInfo, TakeError> {
        self.inner.import(id, source)
    }
    fn legacy_original(&self, id: &RecordingId) -> Result<Option<LegacyFile>, TakeError> {
        self.inner.legacy_original(id)
    }
    fn adopt(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        let hook = self.adopt.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(id)?;
        }
        self.inner.adopt(id)
    }
    fn levels(&self, id: &RecordingId) -> Result<Levels, TakeError> {
        self.inner.levels(id)
    }
    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        self.inner.playback_path(id)
    }
    fn delete(&self, id: &RecordingId) -> Result<(), TakeError> {
        self.inner.delete(id)
    }
    fn files(&self, id: &RecordingId) -> Result<Vec<PathBuf>, TakeError> {
        self.inner.files(id)
    }
}

/// Holds whichever thread reaches it until the test opens it, and tells the test when one has.
#[derive(Clone)]
struct Gate {
    reached: (Sender<()>, Receiver<()>),
    open: (Sender<()>, Receiver<()>),
}

impl Gate {
    fn new() -> Gate {
        Gate {
            reached: crossbeam_channel::unbounded(),
            open: crossbeam_channel::unbounded(),
        }
    }

    fn pass(&self) {
        let _ = self.reached.0.send(());
        let _ = self.open.1.recv_timeout(Duration::from_secs(10));
    }

    fn wait_reached(&self, what: &str) {
        self.reached
            .1
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    fn open(&self) {
        let _ = self.open.0.send(());
    }
}

fn starts(changes: &[Change]) -> bool {
    changes.iter().any(|c| {
        matches!(
            c,
            Change::Job {
                event: JobEvent::Started { .. },
                ..
            }
        )
    })
}

fn seals(changes: &[Change]) -> bool {
    changes.iter().any(|c| {
        matches!(
            c,
            Change::RecordingEdited {
                edit: RecordingEdit::Take(TakeInfo {
                    container: TakeContainer::Flac,
                    ..
                }),
                ..
            }
        )
    })
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("the disk is busy".into())
}

/// Starts a recording of `seconds` of silence and waits until all of it is written.
fn record(dory: &Dory, rig: &Rig, seconds: usize) -> RecordingId {
    rig.capture.push_script(self::seconds(seconds));
    dory.recorder().start(StartOptions::default()).unwrap();
    let want = ms(seconds as u64 * 1000);
    eventually(
        "the script to be written",
        || matches!(dory.recorder().status(), RecorderStatus::Recording { recorded, .. } if recorded == want),
    );
    dory.recorder().status().id().cloned().unwrap()
}

/// Stops, lets the grace period run out, and waits for the recorder to be idle.
fn stop_for_good(dory: &Dory, rig: &Rig) {
    dory.recorder().stop().unwrap();
    eventually("grace", || {
        matches!(dory.recorder().status(), RecorderStatus::Grace { .. })
    });
    rig.clock.advance_ms(5_000);
    eventually("idle", || dory.recorder().status() == RecorderStatus::Idle);
}

fn take_of(dory: &Dory, id: &RecordingId) -> Option<TakeInfo> {
    dory.library().recording(id).unwrap().and_then(|r| r.take)
}

fn recording(id: &RecordingId, take: Option<TakeInfo>) -> Recording {
    Recording {
        id: id.clone(),
        title: None,
        created_at: Timestamp(1_000),
        language: Default::default(),
        layout: ChannelLayout::MicSystem,
        take,
    }
}

fn options() -> Options {
    Options {
        limits: Limits {
            rate_hz: NonZeroU32::new(RATE).unwrap(),
            ..Limits::default()
        },
        clock_poll: Duration::from_millis(2),
        ..Options::default()
    }
}

fn seconds(n: usize) -> Vec<Step> {
    (0..n * 10)
        .map(|_| Step::Silence {
            frames: RATE as usize / 10,
        })
        .collect()
}

fn ms(n: u64) -> Ms {
    Ms(n)
}

fn job_of(dory: &Dory, id: &RecordingId) -> Option<Job> {
    dory.library().jobs(id).unwrap().pop()
}

fn wait_for_job(dory: &Dory, id: &RecordingId, state: JobState) {
    eventually(&format!("job of {id} to be {state:?}"), || {
        job_of(dory, id).is_some_and(|j| j.state == state)
    });
}

fn drain(events: &dory_core::app::Subscription) -> Vec<Event> {
    std::iter::from_fn(|| events.try_recv()).collect()
}

fn segment(start: u64, end: u64, speaker: u16, channel: Channel, text: &str) -> Segment {
    Segment {
        range: TimeRange::new(Ms(start), Ms(end)),
        speaker: SpeakerId(speaker),
        channel,
        text: text.into(),
        // The transcriber detects Vietnamese for every window of this test.
        language: LangTag::parse("vi").ok(),
    }
}

#[test]
fn records_with_a_pause_and_an_undo_then_processes_to_a_summary() {
    let rig = Rig::new();
    for _ in 0..3 {
        rig.capture.push_script(seconds(10));
    }
    let dory = rig.open();
    let events = dory.events();
    let recorder = dory.recorder();

    recorder
        .start(StartOptions {
            title: Some("Weekly sync".into()),
            ..StartOptions::default()
        })
        .unwrap();
    eventually(
        "10 s recorded",
        || matches!(recorder.status(), RecorderStatus::Recording { recorded, .. } if recorded == ms(10_000)),
    );
    let id = recorder.status().id().cloned().unwrap();

    recorder.pause().unwrap();
    eventually("paused", || {
        matches!(recorder.status(), RecorderStatus::Paused { .. })
    });
    assert_eq!(rig.capture.running(), 0, "pause releases the device");
    recorder.resume().unwrap();
    eventually(
        "20 s recorded",
        || matches!(recorder.status(), RecorderStatus::Recording { recorded, .. } if recorded == ms(20_000)),
    );

    recorder.stop().unwrap();
    eventually("grace", || {
        matches!(recorder.status(), RecorderStatus::Grace { .. })
    });
    assert_eq!(
        rig.capture.running(),
        0,
        "the grace period releases the device"
    );
    recorder.undo_stop().unwrap();
    eventually("30 s recorded after the undo", || {
        recorder.status()
            == RecorderStatus::Recording {
                id: id.clone(),
                recorded: ms(30_000),
            }
    });

    recorder.stop().unwrap();
    eventually("grace", || {
        matches!(recorder.status(), RecorderStatus::Grace { .. })
    });
    rig.clock.advance_ms(5_000);
    eventually("idle", || recorder.status() == RecorderStatus::Idle);
    wait_for_job(&dory, &id, JobState::Succeeded);

    let library = dory.library();
    let rows = library.list(&RecordingQuery::default()).unwrap();
    assert_eq!(rows.len(), 1, "the undo continued the same recording");
    let saved = library.recording(&id).unwrap().unwrap();
    assert_eq!(saved.title.as_deref(), Some("Weekly sync"));
    assert_eq!(
        saved.take,
        Some(TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: RATE,
            duration: ms(30_000),
        })
    );
    assert_eq!(rig.takes.frames(&id), Some(30 * RATE as u64));
    assert_eq!(rig.capture.start_count(), 3);
    assert_eq!(rig.capture.running(), 0);

    let transcript = library.transcript(&id).unwrap().unwrap();
    assert_eq!(transcript.revision, Revision(1));
    assert_eq!(
        transcript.segments,
        vec![
            segment(0, 2_000, 0, Channel::Mic, "Let's start."),
            segment(3_000, 5_000, 1, Channel::System, "Agreed."),
            segment(6_000, 8_000, 2, Channel::System, "I'll take notes."),
            segment(12_000, 14_000, 0, Channel::Mic, "Ship it Friday."),
            segment(15_000, 16_000, 1, Channel::System, "Bye."),
        ]
    );
    let vi = LangTag::parse("vi").unwrap();
    assert_eq!(transcript.detected_languages, vec![vi.clone()]);
    assert_eq!(transcript.produced_by, ModelRef::parse(STT).unwrap());
    let calls = rig.transcriber.calls();
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.channel, c.diarize))
            .collect::<Vec<_>>(),
        vec![(Channel::Mic, false), (Channel::System, true)],
        "the mic clip is the local user, only the system clip is diarized"
    );

    let summary = library.summary(&id).unwrap().unwrap();
    assert_eq!(summary.markdown, SUMMARY);
    assert_eq!(summary.transcript_revision, transcript.revision);
    assert_eq!(summary.language, Some(vi));
    let prompts = rig.llm.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].system.contains("tag vi"),
        "{}",
        prompts[0].system
    );
    assert!(
        prompts[0].user.contains("Me: Ship it Friday."),
        "{}",
        prompts[0].user
    );

    let seen = drain(&events);
    for want in [
        Event::RecordingAdded(id.clone()),
        Event::Finished {
            id: id.clone(),
            reason: FinalizeReason::Stopped,
        },
        Event::TranscriptChanged(id.clone()),
        Event::SummaryChanged(id.clone()),
    ] {
        assert!(seen.contains(&want), "missing {want:?}");
    }
    assert!(!seen.contains(&Event::Resync));
}

#[test]
fn startup_seals_crashed_takes_drops_stray_audio_and_requeues_running_jobs() {
    let rig = Rig::new();
    let crashed = RecordingId::parse("crashed-mid-recording").unwrap();
    rig.store
        .apply(vec![Change::RecordingCreated(recording(&crashed, None))])
        .unwrap();
    let mut writer = rig
        .takes
        .begin(&crashed, ChannelLayout::MicSystem, RATE)
        .unwrap();
    writer.write(&vec![0.0; RATE as usize * 2 * 3]).unwrap();
    drop(writer);

    let stray = RecordingId::parse("deleted-before-crash").unwrap();
    drop(
        rig.takes
            .begin(&stray, ChannelLayout::MicSystem, RATE)
            .unwrap(),
    );
    // Deleted while its seal ran, then the seal finished: sealed audio with no row.
    let sealed_stray = RecordingId::parse("sealed-after-delete").unwrap();
    rig.takes
        .begin(&sealed_stray, ChannelLayout::MicSystem, RATE)
        .unwrap()
        .finish()
        .unwrap();
    rig.takes.seal(&sealed_stray).unwrap();

    let interrupted = rig.seed("crashed-mid-stage");
    rig.store
        .apply(vec![Change::JobEnqueued {
            id: interrupted.clone(),
            goal: Goal::ALL,
            priority: Priority::Normal,
            at: Timestamp(1),
        }])
        .unwrap();
    let job = rig.store.jobs_for(&interrupted).unwrap()[0].id;
    rig.store
        .apply(vec![Change::Job {
            job,
            event: JobEvent::Started {
                stage: Stage::Transcribe,
            },
        }])
        .unwrap();

    let dory = rig.open();
    let recovery = dory.recovery().clone();
    assert_eq!(recovery.sealed, vec![crashed.clone()]);
    assert_eq!(
        recovery.deleted_orphans,
        vec![stray.clone(), sealed_stray.clone()]
    );
    assert_eq!(recovery.interrupted, vec![job]);
    assert!(recovery.problems.is_empty(), "{:?}", recovery.problems);

    wait_for_job(&dory, &crashed, JobState::Succeeded);
    wait_for_job(&dory, &interrupted, JobState::Succeeded);
    let take = dory.library().recording(&crashed).unwrap().unwrap().take;
    assert_eq!(
        take,
        Some(TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: RATE,
            duration: ms(3_000),
        })
    );
    assert_eq!(rig.takes.frames(&stray), None);
    assert_eq!(rig.takes.frames(&sealed_stray), None);
    assert!(rig.takes.orphans().unwrap().is_empty());
    dory.shutdown();
    drop(dory);

    let again = rig.open();
    assert_eq!(
        again.recovery(),
        &Default::default(),
        "a second start finds nothing to repair"
    );
}

#[test]
fn cancelling_a_running_stage_stops_the_engine_and_keeps_no_output() {
    let rig = Rig::new();
    let id = rig.seed("to-cancel");
    rig.transcriber.hold(true);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the engine to run", || rig.transcriber.is_running());
    let job = job_of(&dory, &id).unwrap();
    assert_eq!(
        job.state,
        JobState::Running {
            stage: Stage::Transcribe
        }
    );

    dory.pipeline().cancel(job.id).unwrap();
    wait_for_job(&dory, &id, JobState::Cancelled);
    eventually("the engine to stop", || !rig.transcriber.is_running());
    assert_eq!(dory.library().transcript(&id).unwrap(), None);
    assert!(
        dory.pipeline().cancel(job.id).is_err(),
        "a cancelled job cannot be cancelled again"
    );

    rig.transcriber.hold(false);
    dory.pipeline().retry(job.id).unwrap();
    wait_for_job(&dory, &id, JobState::Succeeded);
    assert!(dory.library().transcript(&id).unwrap().is_some());
}

#[test]
fn a_job_blocked_on_a_missing_model_runs_once_the_model_is_provisioned() {
    let rig = Rig::new();
    let stt: ModelId<Stt> = STT.parse().unwrap();
    rig.stt
        .set_availability(&stt, Availability::NeedsProvision { bytes: Some(1_000) });
    let id = rig.seed("needs-a-model");
    let dory = rig.open();
    let events = dory.events();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for_job(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NeedsProvision {
                model: stt.erase(),
                bytes: Some(1_000),
            },
        },
    );
    assert_eq!(rig.stt.opens(), 0, "auto never downloads");

    block_on(dory.models().provision(stt.clone())).unwrap();
    wait_for_job(&dory, &id, JobState::Succeeded);
    assert_eq!(rig.stt.provisions(), 1);
    let seen = drain(&events);
    assert!(seen.contains(&Event::ProvisionFinished {
        model: stt.erase(),
        result: Ok(()),
    }));
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::ProvisionProgress { done: 100, .. }))
    );
}

#[test]
fn a_subscriber_that_never_reads_cannot_stall_recording() {
    let rig = Rig::new();
    rig.capture.push_script(seconds(10));
    let dory = rig.open_with(Options {
        event_capacity: 8,
        level_every: Duration::from_millis(1),
        ..options()
    });
    let stalled = dory.events();
    let recorder = dory.recorder();
    recorder.start(StartOptions::default()).unwrap();
    eventually(
        "10 s recorded",
        || matches!(recorder.status(), RecorderStatus::Recording { recorded, .. } if recorded == ms(10_000)),
    );
    let id = recorder.status().id().cloned().unwrap();
    recorder.stop().unwrap();
    eventually("grace", || {
        matches!(recorder.status(), RecorderStatus::Grace { .. })
    });
    rig.clock.advance_ms(5_000);
    wait_for_job(&dory, &id, JobState::Succeeded);

    let got = drain(&stalled);
    assert_eq!(got.first(), Some(&Event::Resync));
    assert!(got.len() <= 9, "bounded queue plus one resync, got {got:?}");
    assert_eq!(drain(&stalled), vec![]);
}

#[test]
fn reaching_the_cap_warns_then_finalizes_without_a_grace_period() {
    let rig = Rig::new();
    let paced = (0..50)
        .flat_map(|_| {
            [
                Step::Silence {
                    frames: RATE as usize / 10,
                },
                Step::Wait(Duration::from_millis(1)),
            ]
        })
        .collect();
    rig.capture.push_script(paced);
    let dory = rig.open_with(Options {
        limits: Limits {
            cap: ms(3_000),
            warn_before: ms(1_000),
            ..options().limits
        },
        ..options()
    });
    let events = dory.events();
    dory.recorder().start(StartOptions::default()).unwrap();
    eventually("a recording", || dory.recorder().status().id().is_some());
    let id = dory.recorder().status().id().cloned().unwrap();
    wait_for_job(&dory, &id, JobState::Succeeded);
    assert_eq!(dory.recorder().status(), RecorderStatus::Idle);
    assert_eq!(rig.capture.running(), 0);
    let seen = drain(&events);
    assert!(
        seen.iter().any(|e| matches!(e, Event::TimeWarning { .. })),
        "{seen:?}"
    );
    assert!(seen.contains(&Event::Finished {
        id,
        reason: FinalizeReason::CapReached
    }));
}

#[test]
fn a_device_that_will_not_start_leaves_no_recording_behind() {
    let rig = Rig::new();
    rig.capture
        .fail_next_start(CaptureError::AccessDenied(Source::Microphone));
    let dory = rig.open();
    let events = dory.events();
    dory.recorder().start(StartOptions::default()).unwrap();
    eventually("the discard to commit", || {
        drain(&events)
            .iter()
            .any(|e| matches!(e, Event::RecordingDeleted(_)))
    });
    // The discard commits before the recorder's own status settles, so wait for both.
    eventually("the recorder to be idle", || {
        dory.recorder().status() == RecorderStatus::Idle
    });
    assert!(
        dory.library()
            .list(&RecordingQuery::default())
            .unwrap()
            .is_empty()
    );
    assert!(rig.takes.orphans().unwrap().is_empty());
}

#[test]
fn engines_unload_after_sitting_idle_and_load_again_when_needed() {
    let rig = Rig::new();
    let first = rig.seed("first");
    let dory = rig.open_with(Options {
        engine_idle_ms: 60_000,
        ..options()
    });
    dory.pipeline().enqueue(&first, Goal::TRANSCRIBE).unwrap();
    wait_for_job(&dory, &first, JobState::Succeeded);
    assert_eq!(rig.stt.live_engines(), 1, "kept loaded for the next job");

    rig.clock.advance_ms(60_000);
    eventually("the idle engine to unload", || rig.stt.live_engines() == 0);

    let second = rig.seed("second");
    dory.pipeline().enqueue(&second, Goal::TRANSCRIBE).unwrap();
    wait_for_job(&dory, &second, JobState::Succeeded);
    assert_eq!(rig.stt.opens(), 2);
}

#[test]
fn open_rejects_a_catalog_with_duplicate_ids() {
    let rig = Rig::new();
    let mut ports = rig.ports();
    ports.stt.push(rig.stt.clone());
    match Dory::open(ports, options()) {
        Err(OpenError::Catalog(CatalogError::Duplicate(id))) => {
            assert_eq!(id, ModelRef::parse(STT).unwrap());
        }
        other => panic!("expected a duplicate id, got {:?}", other.err()),
    }
}

#[test]
fn a_pinned_model_that_is_gone_falls_back_to_auto_and_unknown_pins_are_refused() {
    let rig = Rig::new();
    let gone: ModelId<Stt> = "fake/gone".parse().unwrap();
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            stt: Choice::Pinned(gone.clone()),
            ..Settings::default()
        })])
        .unwrap();
    let dory = rig.open();
    assert_eq!(
        dory.recovery().choices_reset,
        vec![(KindTag::Stt, gone.erase())]
    );
    assert_eq!(dory.settings().unwrap().stt, Choice::Auto);

    assert!(dory.models().choose(Choice::Pinned(gone)).is_err());
    let known: ModelId<Stt> = STT.parse().unwrap();
    dory.models().choose(Choice::Pinned(known.clone())).unwrap();
    let rows = block_on(dory.models().list::<Stt>()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].chosen);
    assert_eq!(rows[0].status.availability, Availability::Ready);
}

#[test]
fn a_cancel_that_commits_while_started_is_committing_still_stops_the_stage() {
    let rig = Rig::new();
    let id = rig.seed("cancel-in-the-window");
    rig.transcriber.hold(true);
    // Hold the worker right after `Started` commits: the moment a cancel used to find no token.
    let gate = Gate::new();
    let at_start = gate.clone();
    rig.hooked_store.after(move |changes| {
        if starts(changes) {
            at_start.pass();
        }
    });
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    gate.wait_reached("Started to commit");
    let job = job_of(&dory, &id).unwrap();
    dory.pipeline().cancel(job.id).unwrap();
    gate.open();

    wait_for_job(&dory, &id, JobState::Cancelled);
    // The held engine returns only when its token is cancelled.
    eventually("the engine to see the cancel and return", || {
        rig.transcriber.calls().len() == 1 && !rig.transcriber.is_running()
    });
    assert_eq!(dory.library().transcript(&id).unwrap(), None);
}

#[test]
fn deleting_a_recording_while_its_take_seals_is_refused_until_the_seal_lands() {
    let rig = Rig::new();
    let gate = Gate::new();
    let at_seal = gate.clone();
    rig.hooked_takes.on_seal(move |_| {
        at_seal.pass();
        Ok(())
    });
    let dory = rig.open();
    let id = record(&dory, &rig, 2);
    stop_for_good(&dory, &rig);
    gate.wait_reached("the seal to start");

    assert!(matches!(
        dory.library().delete(&id),
        Err(DoryError::Saving(busy)) if busy == id
    ));
    gate.open();
    eventually("the sealed take to commit", || {
        take_of(&dory, &id).is_some_and(|t| t.container == TakeContainer::Flac)
    });

    dory.library().delete(&id).unwrap();
    assert!(
        rig.takes.stored().unwrap().is_empty(),
        "no audio left behind"
    );
    assert!(
        dory.library()
            .list(&RecordingQuery::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_seal_that_fails_lists_the_wav_and_the_next_start_seals_it() {
    let rig = Rig::new();
    rig.hooked_takes
        .on_seal(|_| Err(TakeError::Decode("the encoder broke".into())));
    let dory = rig.open();
    let events = dory.events();
    let id = record(&dory, &rig, 2);
    stop_for_good(&dory, &rig);
    eventually("the WAV to be listed", || {
        take_of(&dory, &id).is_some_and(|t| t.container == TakeContainer::Wav)
    });
    assert!(drain(&events).iter().any(|e| matches!(
        e,
        Event::Error {
            context: "sealing the take",
            ..
        }
    )));
    assert!(
        job_of(&dory, &id).is_none(),
        "nothing processes an unsealed take"
    );
    drop(dory);

    *rig.hooked_takes.seal.lock().unwrap() = None;
    let again = rig.open();
    assert_eq!(again.recovery().sealed, vec![id.clone()]);
    assert_eq!(
        take_of(&again, &id),
        Some(TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: RATE,
            duration: ms(2_000),
        })
    );
    wait_for_job(&again, &id, JobState::Succeeded);
}

#[test]
fn a_sealed_take_whose_commit_fails_for_a_moment_is_committed_without_a_restart() {
    let rig = Rig::new();
    let failures = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&failures);
    rig.hooked_store.before(move |changes| {
        if seals(changes) && counted.fetch_add(1, Ordering::SeqCst) < 2 {
            return Err(unavailable());
        }
        Ok(())
    });
    let dory = rig.open();
    let events = dory.events();
    let id = record(&dory, &rig, 2);
    stop_for_good(&dory, &rig);

    wait_for_job(&dory, &id, JobState::Succeeded);
    assert_eq!(
        take_of(&dory, &id).map(|t| t.container),
        Some(TakeContainer::Flac)
    );
    assert_eq!(
        failures.load(Ordering::SeqCst),
        3,
        "two failures, then it landed"
    );
    assert!(
        !drain(&events)
            .iter()
            .any(|e| matches!(e, Event::Error { .. })),
        "a commit that landed on a retry is not an error"
    );
}

#[test]
fn a_commit_that_keeps_failing_is_reported_and_the_next_start_commits_it() {
    let rig = Rig::new();
    rig.hooked_store.before(|changes| match seals(changes) {
        true => Err(unavailable()),
        false => Ok(()),
    });
    let dory = rig.open();
    let events = dory.events();
    let id = record(&dory, &rig, 1);
    stop_for_good(&dory, &rig);
    dory.shutdown();
    assert!(drain(&events).iter().any(|e| matches!(
        e,
        Event::Error {
            context: "saving the recording",
            ..
        }
    )));
    assert!(rig.takes.is_sealed(&id));
    assert_eq!(take_of(&dory, &id), None);
    drop(dory);

    *rig.hooked_store.before.lock().unwrap() = None;
    let again = rig.open();
    assert_eq!(again.recovery().sealed, vec![id.clone()]);
    wait_for_job(&again, &id, JobState::Succeeded);
}

#[test]
fn dropping_dory_while_recording_seals_the_take_before_it_returns() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = record(&dory, &rig, 3);
    drop(dory);

    assert!(rig.takes.is_sealed(&id));
    assert_eq!(
        rig.store.recording(&id).unwrap().unwrap().take,
        Some(TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: RATE,
            duration: ms(3_000),
        })
    );
    let again = rig.open();
    assert_eq!(
        again.recovery(),
        &Default::default(),
        "nothing left to repair"
    );
}

#[test]
fn two_threads_shutting_down_at_once_both_wait_for_the_seal() {
    let rig = Rig::new();
    let gate = Gate::new();
    let at_seal = gate.clone();
    rig.hooked_takes.on_seal(move |_| {
        at_seal.pass();
        Ok(())
    });
    let dory = rig.open();
    let id = record(&dory, &rig, 1);
    std::thread::scope(|scope| {
        let callers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    dory.shutdown();
                    take_of(&dory, &id)
                })
            })
            .collect();
        gate.wait_reached("the seal to start");
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            callers.iter().all(|c| !c.is_finished()),
            "neither caller returns while the seal is held"
        );
        gate.open();
        for caller in callers {
            assert_eq!(
                caller.join().unwrap().map(|t| t.container),
                Some(TakeContainer::Flac)
            );
        }
    });
}

#[test]
fn dropping_dory_mid_stage_cancels_the_stage_and_requeues_the_job() {
    let rig = Rig::new();
    let id = rig.seed("mid-stage");
    rig.transcriber.hold(true);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the engine to run", || rig.transcriber.is_running());

    let (dropped, done) = crossbeam_channel::bounded(1);
    let started = Instant::now();
    std::thread::spawn(move || {
        drop(dory);
        let _ = dropped.send(());
    });
    done.recv_timeout(Duration::from_secs(5))
        .expect("drop to return without waiting for the held stage");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!rig.transcriber.is_running());
    let job = rig.store.jobs_for(&id).unwrap().pop().unwrap();
    assert_eq!(job.state, JobState::Queued, "a shutdown is not a cancel");

    rig.transcriber.hold(false);
    let again = rig.open();
    wait_for_job(&again, &id, JobState::Succeeded);
}

/// Capture that writes one second of silence and then keeps the sink after `stop`, as a careless
/// adapter might. Its probes panic.
#[derive(Default)]
struct LeakyCapture {
    kept: Mutex<Vec<FrameSink>>,
}

struct NoopHandle;

impl CaptureHandle for NoopHandle {
    fn stop(self: Box<Self>) {}
}

impl CaptureBackend for LeakyCapture {
    fn probe(&self) -> CaptureCaps {
        panic!("the probe crashed")
    }

    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        panic!("the device list crashed")
    }

    fn request_access(&self, _: Source) -> Result<Access, CaptureError> {
        Ok(Access::Granted)
    }

    fn start(
        &self,
        _: &CaptureSpec,
        mut sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        sink.push(&vec![0.0; RATE as usize * sink.channels()]);
        self.kept.lock().unwrap().push(sink);
        Ok(Box::new(NoopHandle))
    }
}

#[test]
fn an_adapter_that_keeps_its_sink_after_stop_cannot_hang_the_recorder() {
    // On its own thread, so a recorder that hangs fails this test instead of hanging the run.
    let (done, finished) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let rig = Rig::new();
        let leaky = Arc::new(LeakyCapture::default());
        let dory = Dory::open(
            Ports {
                capture: leaky.clone(),
                ..rig.ports()
            },
            options(),
        )
        .unwrap();
        dory.recorder().start(StartOptions::default()).unwrap();
        eventually(
            "a second recorded",
            || matches!(dory.recorder().status(), RecorderStatus::Recording { recorded, .. } if recorded == ms(1_000)),
        );
        let id = dory.recorder().status().id().cloned().unwrap();
        stop_for_good(&dory, &rig);
        wait_for_job(&dory, &id, JobState::Succeeded);
        let _ = done.send((
            take_of(&dory, &id).map(|t| t.duration),
            leaky.kept.lock().unwrap().len(),
        ));
    });
    let (duration, kept) = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the recorder to stop, seal, and process the take");
    assert_eq!(duration, Some(ms(1_000)));
    assert_eq!(kept, 1, "the sink is still alive");
}

#[test]
fn a_probe_that_panics_resolves_to_an_error_instead_of_panicking_the_caller() {
    let rig = Rig::new();
    let dory = Dory::open(
        Ports {
            capture: Arc::new(LeakyCapture::default()),
            ..rig.ports()
        },
        options(),
    )
    .unwrap();
    assert!(block_on(dory.recorder().probe()).is_err());
    assert!(block_on(dory.recorder().input_devices()).is_err());
}

/// A speech model whose download panics.
struct PanickyProvider;

impl ModelProvider<Stt> for PanickyProvider {
    fn id(&self) -> ProviderId {
        ProviderId::parse("boom").unwrap()
    }

    fn catalog(&self) -> Vec<ModelEntry<Stt>> {
        vec![stt_entry("boom/stt", DiarizationSupport::None)]
    }

    fn availability(&self, _: &ModelId<Stt>) -> Availability {
        Availability::NeedsProvision { bytes: None }
    }

    fn provision(
        &self,
        _: &ModelId<Stt>,
        _: &mut dyn ProvisionSink,
        _: &CancelToken,
    ) -> Result<(), ProvisionError> {
        panic!("the download crashed")
    }

    fn open(
        &self,
        model: &ModelId<Stt>,
        _: &CancelToken,
    ) -> Result<Arc<dyn Transcriber>, EngineError> {
        Err(EngineError::ModelMissing(model.to_string()))
    }
}

#[test]
fn a_provider_that_panics_reports_a_failure_and_can_be_tried_again() {
    let rig = Rig::new();
    let mut ports = rig.ports();
    ports.stt.push(Arc::new(PanickyProvider));
    let dory = Dory::open(ports, options()).unwrap();
    let events = dory.events();
    let boom: ModelId<Stt> = "boom/stt".parse().unwrap();
    for attempt in 0..2 {
        match block_on(dory.models().provision(boom.clone())) {
            Err(ProvisionError::Other(why)) => {
                assert!(why.contains("panicked"), "attempt {attempt}: {why}");
            }
            other => panic!("attempt {attempt}: {other:?}"),
        }
    }
    let finished = drain(&events)
        .into_iter()
        .filter(|e| matches!(e, Event::ProvisionFinished { result: Err(_), .. }))
        .count();
    assert_eq!(finished, 2);
}

#[test]
fn choosing_two_kinds_at_once_keeps_both_choices() {
    let rig = Rig::new();
    let dory = rig.open();
    let stt: ModelId<Stt> = STT.parse().unwrap();
    let llm: ModelId<Llm> = LLM.parse().unwrap();
    for _ in 0..50 {
        dory.models().choose::<Stt>(Choice::Auto).unwrap();
        dory.models().choose::<Llm>(Choice::Auto).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| dory.models().choose(Choice::Pinned(stt.clone())).unwrap());
            scope.spawn(|| dory.models().choose(Choice::Pinned(llm.clone())).unwrap());
        });
        let settings = dory.settings().unwrap();
        assert_eq!(settings.stt, Choice::Pinned(stt.clone()));
        assert_eq!(settings.llm, Choice::Pinned(llm.clone()));
    }
}

#[test]
fn an_empty_speech_catalog_opens_and_blocks_jobs_on_the_missing_model() {
    let rig = Rig::new();
    let id = rig.seed("no-engine");
    let dory = Dory::open(
        Ports {
            stt: vec![],
            ..rig.ports()
        },
        options(),
    )
    .unwrap();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for_job(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NoModel { kind: KindTag::Stt },
        },
    );
}

#[test]
fn an_imported_audio_file_becomes_a_mixed_recording_with_its_take() {
    let rig = Rig::new();
    let dory = rig.open();
    let first = dory
        .library()
        .import_audio(Path::new("/tmp/meeting.wav"), Some("Planning".into()))
        .unwrap();
    let second = dory
        .library()
        .import_audio(Path::new("/tmp/meeting.wav"), None)
        .unwrap();
    assert_ne!(
        first, second,
        "two imports in the same second get their own ids"
    );
    let recording = dory.library().recording(&first).unwrap().unwrap();
    assert_eq!(recording.layout, ChannelLayout::Mixed);
    assert_eq!(recording.title.as_deref(), Some("Planning"));
    assert!(recording.take.is_some());
    assert!(
        dory.library().jobs(&first).unwrap().is_empty(),
        "an import waits for an explicit process"
    );
}

#[test]
fn a_player_opens_on_the_sealed_take_with_its_layout_and_refuses_a_take_being_saved() {
    let rig = Rig::new();
    let id = rig.seed("played");
    let dory = rig.open();

    let player = dory.library().player(&id).unwrap();
    let opened = rig.playback.opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].path, PathBuf::from("mem/played"));
    assert_eq!(opened[0].layout, ChannelLayout::MicSystem);
    assert_eq!(opened[0].duration, ms(20_000));

    player.seek(ms(500)).unwrap();
    player.play().unwrap();
    rig.playback.advance(ms(250));
    let status = player.status();
    assert_eq!(
        (status.state, status.position),
        (PlayState::Playing, ms(750))
    );

    let missing = RecordingId::parse("nope").unwrap();
    assert!(matches!(
        dory.library().player(&missing),
        Err(DoryError::Store(StoreError::NotFound(_)))
    ));

    rig.capture.push_script(seconds(1));
    dory.recorder().start(StartOptions::default()).unwrap();
    eventually("recording", || dory.recorder().status().id().is_some());
    let recording = dory.recorder().status().id().cloned().unwrap();
    assert!(matches!(
        dory.library().player(&recording),
        Err(DoryError::Recording(_))
    ));
    assert_eq!(rig.playback.opened().len(), 1);
}

#[test]
fn a_waveform_rises_where_the_take_is_loud_and_is_refused_while_recording() {
    let rig = Rig::new();
    let id = RecordingId::parse("waved").unwrap();
    let mut writer = rig
        .takes
        .begin(&id, ChannelLayout::MicSystem, RATE)
        .unwrap();
    let second = RATE as usize * 2;
    writer.write(&vec![0.0; second * 5]).unwrap();
    let speech: Vec<f32> = (0..second * 5)
        .map(|i| 0.3 * (i as f32 * 0.07).sin() * (i % 2) as f32)
        .collect();
    writer.write(&speech).unwrap();
    writer.finish().unwrap();
    let take = rig.takes.seal(&id).unwrap();
    rig.store
        .apply(vec![Change::RecordingCreated(recording(&id, Some(take)))])
        .unwrap();
    let dory = rig.open();

    let wave = dory.library().waveform(&id).unwrap();
    let heights = wave.heights();
    assert_eq!(heights.len(), dory_core::domain::BUCKETS);
    let (silent, loud) = heights.split_at(heights.len() / 2);
    assert!(silent.iter().all(|h| *h == Waveform::FLOOR), "{silent:?}");
    assert!(loud.iter().all(|h| *h > 0.9), "{loud:?}");

    rig.capture.push_script(seconds(1));
    dory.recorder().start(StartOptions::default()).unwrap();
    eventually("recording", || dory.recorder().status().id().is_some());
    let recording = dory.recorder().status().id().cloned().unwrap();
    assert!(matches!(
        dory.library().waveform(&recording),
        Err(DoryError::Recording(_))
    ));
}

/// A recording with a transcript, a summary, and a named speaker, as a finished job leaves it.
fn processed(dory: &Dory, rig: &Rig, name: &str) -> RecordingId {
    let id = rig.seed(name);
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for_job(dory, &id, JobState::Succeeded);
    let speaker = dory.library().transcript(&id).unwrap().unwrap().segments[0].speaker;
    dory.library()
        .rename_speaker(&id, speaker, Some("Ana".into()))
        .unwrap();
    dory.library()
        .set_title(&id, Some("Weekly sync".into()))
        .unwrap();
    id
}

fn library_is_empty(dory: &Dory) -> bool {
    dory.library()
        .list(&RecordingQuery::default())
        .unwrap()
        .is_empty()
}

#[test]
fn trash_then_restore_brings_back_the_row_transcript_summary_names_and_audio() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "weekly");
    let library = dory.library();
    let before = (
        library.recording(&id).unwrap().unwrap(),
        library.transcript(&id).unwrap().unwrap(),
        library.summary(&id).unwrap().unwrap(),
        library.speaker_names(&id).unwrap(),
    );
    assert!(!before.3.names.is_empty());
    let frames = rig.takes.frames(&id);
    assert!(frames.is_some());

    let trashed = library.trash(&id).unwrap();
    assert_eq!((trashed.id(), trashed.title()), (&id, Some("Weekly sync")));
    assert!(library_is_empty(&dory));
    assert_eq!(library.transcript(&id).unwrap(), None);
    assert!(library.jobs(&id).unwrap().is_empty());
    assert!(rig.takes.stored().unwrap().is_empty(), "the audio left");
    assert_eq!(rig.trash.trashed_files().len(), 1, "and went to the trash");

    assert_eq!(library.restore(trashed).unwrap(), id);
    let after = (
        library.recording(&id).unwrap().unwrap(),
        library.transcript(&id).unwrap().unwrap(),
        library.summary(&id).unwrap().unwrap(),
        library.speaker_names(&id).unwrap(),
    );
    assert_eq!(after, before);
    assert_eq!(rig.takes.frames(&id), frames);
    assert!(rig.trash.trashed_files().is_empty());
    assert!(library.player(&id).is_ok(), "the audio plays again");
    assert!(
        library.jobs(&id).unwrap().is_empty(),
        "jobs are not restored"
    );
}

#[test]
fn trash_refuses_a_take_being_recorded_or_sealed() {
    let rig = Rig::new();
    let gate = Gate::new();
    let at_seal = gate.clone();
    rig.hooked_takes.on_seal(move |_| {
        at_seal.pass();
        Ok(())
    });
    let dory = rig.open();
    let id = record(&dory, &rig, 2);
    assert!(matches!(
        dory.library().trash(&id),
        Err(DoryError::Recording(busy)) if busy == id
    ));

    stop_for_good(&dory, &rig);
    gate.wait_reached("the seal to start");
    assert!(matches!(
        dory.library().trash(&id),
        Err(DoryError::Saving(busy)) if busy == id
    ));
    gate.open();
    eventually("the sealed take to commit", || {
        take_of(&dory, &id).is_some_and(|t| t.container == TakeContainer::Flac)
    });
    assert!(rig.trash.trashed_files().is_empty(), "nothing moved");
    assert!(dory.library().trash(&id).is_ok());
}

#[test]
fn trashing_an_unknown_recording_moves_nothing() {
    let rig = Rig::new();
    let dory = rig.open();
    let missing = RecordingId::parse("never-existed").unwrap();
    assert!(matches!(
        dory.library().trash(&missing),
        Err(DoryError::Store(StoreError::NotFound(_)))
    ));
}

#[test]
fn trash_cancels_a_running_job_and_the_restored_recording_can_be_processed_again() {
    let rig = Rig::new();
    let id = rig.seed("mid-stage");
    rig.transcriber.hold(true);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the engine to run", || rig.transcriber.is_running());

    let trashed = dory.library().trash(&id).unwrap();
    eventually("the engine to stop", || !rig.transcriber.is_running());
    rig.transcriber.hold(false);
    assert!(library_is_empty(&dory));
    assert!(dory.library().jobs(&id).unwrap().is_empty());

    dory.library().restore(trashed).unwrap();
    assert_eq!(dory.library().transcript(&id).unwrap(), None);
    assert!(
        dory.library().jobs(&id).unwrap().is_empty(),
        "the cancelled job is not restored"
    );
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for_job(&dory, &id, JobState::Succeeded);
    assert!(dory.library().transcript(&id).unwrap().is_some());
}

#[test]
fn a_trash_that_refuses_leaves_the_recording_whole() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "refused");
    rig.trash.fail_moves(true);

    assert!(matches!(
        dory.library().trash(&id),
        Err(DoryError::Trash(_))
    ));
    assert!(dory.library().recording(&id).unwrap().is_some());
    assert!(dory.library().transcript(&id).unwrap().is_some());
    assert!(rig.takes.frames(&id).is_some());
}

#[test]
fn a_failed_delete_commit_puts_the_files_back() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "commit-fails");
    rig.hooked_store.before(|changes| {
        match changes
            .iter()
            .any(|c| matches!(c, Change::RecordingDeleted(_)))
        {
            true => Err(unavailable()),
            false => Ok(()),
        }
    });

    assert!(dory.library().trash(&id).is_err());
    assert!(dory.library().recording(&id).unwrap().is_some());
    assert!(rig.takes.frames(&id).is_some(), "the audio is back");
    assert!(rig.trash.trashed_files().is_empty());
}

#[test]
fn restoring_after_the_trash_was_emptied_fails_and_creates_no_row() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "emptied");
    let trashed = dory.library().trash(&id).unwrap();
    for file in rig.trash.trashed_files() {
        std::fs::remove_file(file).unwrap();
    }

    assert!(matches!(
        dory.library().restore(trashed),
        Err(DoryError::Trash(_))
    ));
    assert!(library_is_empty(&dory));
}

#[test]
fn restoring_over_a_recording_with_the_same_id_is_refused() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "twice");
    let trashed = dory.library().trash(&id).unwrap();
    rig.store
        .apply(vec![Change::RecordingCreated(recording(&id, None))])
        .unwrap();

    assert!(matches!(
        dory.library().restore(trashed),
        Err(DoryError::Exists(same)) if same == id
    ));
    assert_eq!(rig.trash.trashed_files().len(), 1, "the files stay trashed");
}

#[test]
fn a_restore_whose_commit_fails_puts_the_files_back_in_the_trash() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "restore-fails");
    let trashed = dory.library().trash(&id).unwrap();
    rig.hooked_store.before(|changes| {
        match changes
            .iter()
            .any(|c| matches!(c, Change::RecordingCreated(_)))
        {
            true => Err(unavailable()),
            false => Ok(()),
        }
    });

    assert!(dory.library().restore(trashed).is_err());
    assert!(library_is_empty(&dory));
    assert!(rig.takes.stored().unwrap().is_empty());
    assert_eq!(rig.trash.trashed_files().len(), 1);
}

#[test]
fn a_crash_between_putting_the_files_back_and_the_commit_loses_the_recording_at_next_start() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = processed(&dory, &rig, "crash-window");
    let trashed = dory.library().trash(&id).unwrap();
    rig.hooked_store.before(|changes| {
        if changes
            .iter()
            .any(|c| matches!(c, Change::RecordingCreated(_)))
        {
            panic!("the process dies before the restore commits");
        }
        Ok(())
    });

    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dory.library().restore(trashed)
    }));
    assert!(died.is_err());
    drop(dory);
    rig.hooked_store.before(|_| Ok(()));
    assert_eq!(
        rig.takes.stored().unwrap(),
        vec![id.clone()],
        "the audio is back in place"
    );
    assert!(rig.trash.trashed_files().is_empty());
    assert_eq!(rig.store.recording(&id).unwrap(), None, "with no row");

    let dory = rig.open();
    assert_eq!(dory.recovery().deleted_orphans, vec![id.clone()]);
    assert!(rig.takes.stored().unwrap().is_empty());
    assert!(library_is_empty(&dory));
}

const ORIGINAL_BYTES: &[u8] = b"pretend this is the old app's webm";

impl Rig {
    /// A recording the old app made: a real file in its folder, and a pointer to it.
    fn seed_legacy(&self, name: &str) -> (RecordingId, PathBuf) {
        let id = RecordingId::parse(name).unwrap();
        let old_app = self.old_app();
        std::fs::create_dir_all(&old_app).unwrap();
        let original = old_app.join(format!("{name}.webm"));
        std::fs::write(&original, ORIGINAL_BYTES).unwrap();
        let take = self.takes.link_legacy(&id, &original, Ms(20_000)).unwrap();
        assert_eq!(take.container, TakeContainer::LegacyWebm);
        let mut row = recording(&id, Some(take));
        row.layout = ChannelLayout::Mixed;
        self.store
            .apply(vec![Change::RecordingCreated(row)])
            .unwrap();
        (id, original)
    }

    fn old_app(&self) -> PathBuf {
        self._scratch.path().join("old-app")
    }
}

fn container_of(dory: &Dory, id: &RecordingId) -> TakeContainer {
    take_of(dory, id).unwrap().container
}

#[test]
fn originals_lists_the_files_recordings_still_read_in_place() {
    let rig = Rig::new();
    let (kept, kept_path) = rig.seed_legacy("kept");
    let (gone, gone_path) = rig.seed_legacy("gone");
    rig.seed("own");
    std::fs::remove_file(gone_path).unwrap();
    let dory = rig.open();

    let originals = dory.library().originals().unwrap();
    assert_eq!(
        originals,
        vec![dory_core::app::Original {
            id: kept,
            path: kept_path,
            bytes: ORIGINAL_BYTES.len() as u64,
        }]
    );
    assert!(!originals.iter().any(|o| o.id == gone));
}

#[test]
fn trashing_originals_keeps_every_recording_playable_and_undo_puts_the_files_back() {
    let rig = Rig::new();
    let (first, first_path) = rig.seed_legacy("first");
    let (second, second_path) = rig.seed_legacy("second");
    let own = rig.seed("own");
    let dory = rig.open();
    let library = dory.library();

    let trashed = library.trash_originals().unwrap();
    assert_eq!(trashed.count(), 2);
    assert_eq!(trashed.bytes(), 2 * ORIGINAL_BYTES.len() as u64);
    assert_eq!(trashed.skipped(), 0);
    assert!(!first_path.exists() && !second_path.exists());
    assert_eq!(rig.trash.trashed_files().len(), 2);

    for id in [&first, &second] {
        assert_eq!(container_of(&dory, id), TakeContainer::Flac);
        assert_eq!(rig.takes.legacy_original(id).unwrap().map(|f| f.path), None);
        let clip = rig.takes.clip(id, Channel::Mixed, None).unwrap();
        assert_eq!(clip.duration, Ms(20_000));
        assert!(library.player(id).is_ok());
        assert!(
            !rig.takes
                .playback_path(id)
                .unwrap()
                .starts_with(rig.old_app())
        );
        assert!(
            !rig.takes.files(id).unwrap().contains(&first_path),
            "the original was never one of Dory's files"
        );
    }
    assert_eq!(container_of(&dory, &own), TakeContainer::Flac);
    assert_eq!(library.originals().unwrap(), vec![]);
    assert_eq!(
        library.trash_originals().unwrap().count(),
        0,
        "a second run finds nothing"
    );

    library.restore_originals(trashed).unwrap();
    assert_eq!(std::fs::read(&first_path).unwrap(), ORIGINAL_BYTES);
    assert_eq!(std::fs::read(&second_path).unwrap(), ORIGINAL_BYTES);
    assert!(rig.trash.trashed_files().is_empty());
    for id in [&first, &second] {
        assert_eq!(container_of(&dory, id), TakeContainer::Flac);
        assert!(library.player(id).is_ok());
    }
}

#[test]
fn a_recording_with_a_running_job_keeps_its_original() {
    let rig = Rig::new();
    let (busy, busy_path) = rig.seed_legacy("busy");
    let (idle, idle_path) = rig.seed_legacy("idle");
    rig.transcriber.hold(true);
    let dory = rig.open();
    dory.pipeline().enqueue(&busy, Goal::ALL).unwrap();
    eventually("the engine to run", || rig.transcriber.is_running());

    let trashed = dory.library().trash_originals().unwrap();
    assert_eq!((trashed.count(), trashed.skipped()), (1, 1));
    assert!(busy_path.is_file(), "the job is still reading it");
    assert!(!idle_path.exists());
    assert_eq!(container_of(&dory, &busy), TakeContainer::LegacyWebm);
    assert_eq!(container_of(&dory, &idle), TakeContainer::Flac);
    assert_eq!(
        dory.library().originals().unwrap().len(),
        1,
        "it can be trashed once the job is done"
    );
    rig.transcriber.hold(false);
}

#[test]
fn a_failing_copy_moves_nothing_to_the_trash() {
    let rig = Rig::new();
    let (first, first_path) = rig.seed_legacy("first");
    let (second, second_path) = rig.seed_legacy("second");
    let failing = second.clone();
    rig.hooked_takes.on_adopt(move |id| {
        if *id == failing {
            Err(TakeError::ConverterMissing("ffmpeg".into()))
        } else {
            Ok(())
        }
    });
    let dory = rig.open();

    assert!(matches!(
        dory.library().trash_originals(),
        Err(DoryError::Take(TakeError::ConverterMissing(_)))
    ));
    assert!(first_path.is_file() && second_path.is_file());
    assert!(rig.trash.trashed_files().is_empty());
    assert_eq!(container_of(&dory, &second), TakeContainer::LegacyWebm);
    assert!(
        dory.library().player(&second).is_ok(),
        "it still plays from the original"
    );
    assert_eq!(
        container_of(&dory, &first),
        TakeContainer::Flac,
        "a finished copy stays"
    );
}

#[test]
fn a_trash_that_refuses_leaves_the_originals_in_place() {
    let rig = Rig::new();
    let (id, path) = rig.seed_legacy("refused");
    rig.trash.fail_moves(true);
    let dory = rig.open();

    assert!(matches!(
        dory.library().trash_originals(),
        Err(DoryError::Trash(_))
    ));
    assert!(path.is_file());
    assert!(dory.library().player(&id).is_ok());
}

#[test]
fn a_row_left_saying_webm_over_a_copied_take_is_corrected_on_the_next_run() {
    let rig = Rig::new();
    let (id, path) = rig.seed_legacy("half");
    let dory = rig.open();
    rig.hooked_store.before(|changes| {
        if changes
            .iter()
            .any(|c| matches!(c, Change::RecordingEdited { .. }))
        {
            Err(StoreError::Unavailable("the disk is busy".into()))
        } else {
            Ok(())
        }
    });
    assert!(matches!(
        dory.library().trash_originals(),
        Err(DoryError::Store(_))
    ));
    assert_eq!(container_of(&dory, &id), TakeContainer::LegacyWebm);
    assert!(path.is_file() && rig.trash.trashed_files().is_empty());

    rig.hooked_store.before(|_| Ok(()));
    assert_eq!(dory.library().trash_originals().unwrap().count(), 0);
    assert_eq!(container_of(&dory, &id), TakeContainer::Flac);
}

const FAR_FUTURE: Timestamp = Timestamp(i64::MAX / 2);

/// Three queued jobs, enqueued in the order `a`, `b`, `c`, that the worker leaves alone: each
/// has started once and been put back with a retry far in the future.
fn held_queue(rig: &Rig) -> (Dory, [JobId; 3]) {
    let mut changes = Vec::new();
    let ids = ["a", "b", "c"].map(|name| rig.seed(name));
    for (n, id) in ids.iter().enumerate() {
        changes.push(Change::JobEnqueued {
            id: id.clone(),
            goal: Goal::ALL,
            priority: Priority::Normal,
            at: Timestamp(10 + n as i64),
        });
    }
    rig.store.apply(changes).unwrap();
    let jobs = ids.map(|id| rig.store.jobs_for(&id).unwrap()[0].id);
    let hold = jobs.iter().flat_map(|&job| {
        [
            Change::Job {
                job,
                event: JobEvent::Started {
                    stage: Stage::Transcribe,
                },
            },
            Change::Job {
                job,
                event: JobEvent::Retry {
                    not_before: FAR_FUTURE,
                },
            },
        ]
    });
    rig.store.apply(hold.collect()).unwrap();
    (rig.open(), jobs)
}

fn queued_ids(dory: &Dory) -> Vec<u64> {
    dory.pipeline()
        .queue()
        .unwrap()
        .iter()
        .map(|j| j.id.0)
        .collect()
}

fn ids(order: &[JobId]) -> Vec<u64> {
    order.iter().map(|j| j.0).collect()
}

#[test]
fn a_queued_job_can_be_moved_to_any_place_and_to_the_top_twice_in_a_row() {
    let rig = Rig::new();
    let (dory, [a, b, c]) = held_queue(&rig);
    assert_eq!(queued_ids(&dory), ids(&[a, b, c]));

    dory.pipeline().move_job(c, 0).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[c, a, b]), "to the top");

    dory.pipeline().move_job(a, 0).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[a, c, b]), "to the top again");

    dory.pipeline().move_job(c, 2).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[a, b, c]), "down");

    dory.pipeline().move_job(a, 1).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[b, a, c]), "to the middle");

    dory.pipeline().move_job(b, 99).unwrap();
    assert_eq!(
        queued_ids(&dory),
        ids(&[a, c, b]),
        "past the end clamps to the last place"
    );
}

#[test]
fn the_worker_takes_the_jobs_in_the_order_the_user_set() {
    let rig = Rig::new();
    let (dory, [a, _b, c]) = held_queue(&rig);
    dory.pipeline().move_job(c, 0).unwrap();
    dory.pipeline().move_job(a, 1).unwrap();
    let next = rig.store.next_runnable_job(FAR_FUTURE).unwrap().unwrap();
    assert_eq!(next.id, c);
    dory.pipeline().cancel(c).unwrap();
    let next = rig.store.next_runnable_job(FAR_FUTURE).unwrap().unwrap();
    assert_eq!(next.id, a);
}

#[test]
fn a_move_is_one_commit_so_a_reader_never_sees_half_a_reorder() {
    let rig = Rig::new();
    let (dory, [_a, _b, c]) = held_queue(&rig);
    let applies = Arc::new(Mutex::new(Vec::new()));
    let seen = applies.clone();
    rig.hooked_store
        .after(move |changes| seen.lock().unwrap().push(changes.to_vec()));
    dory.pipeline().move_job(c, 0).unwrap();
    let applies = applies.lock().unwrap();
    let places: Vec<_> = applies
        .iter()
        .filter(|changes| {
            changes.iter().any(|c| {
                matches!(
                    c,
                    Change::Job {
                        event: JobEvent::Place { .. },
                        ..
                    }
                )
            })
        })
        .collect();
    assert_eq!(places.len(), 1, "one apply carries every Place");
    assert_eq!(places[0].len(), 3, "all three jobs changed place");
}

#[test]
fn a_move_over_a_front_job_takes_its_place_and_the_front_job_follows() {
    let rig = Rig::new();
    let (dory, [a, b, c]) = held_queue(&rig);
    dory.pipeline().promote(b).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[b, a, c]));
    dory.pipeline().move_job(c, 0).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[c, b, a]));
    assert_eq!(
        dory.pipeline().job(b).unwrap().unwrap().priority,
        Priority::Normal
    );
}

#[test]
fn cancelling_then_retrying_a_job_puts_it_back_where_it_was() {
    let rig = Rig::new();
    let (dory, [a, b, c]) = held_queue(&rig);
    // A retried job is runnable at once. The worker stops before it commits any start, so the
    // jobs stay queued while the test reads the order.
    let gate = Gate::new();
    let at_start = gate.clone();
    rig.hooked_store.before(move |changes| {
        if starts(changes) {
            at_start.pass();
        }
        Ok(())
    });

    dory.pipeline().cancel(b).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[a, c]));
    dory.pipeline().retry(b).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[a, b, c]), "undo of a cancel");

    dory.pipeline().move_job(c, 0).unwrap();
    dory.pipeline().cancel(c).unwrap();
    dory.pipeline().retry(c).unwrap();
    assert_eq!(
        queued_ids(&dory),
        ids(&[c, a, b]),
        "an undo keeps a place the user chose"
    );

    rig.hooked_store.before(|_| Ok(()));
    gate.open();
}

#[test]
fn moving_a_job_that_is_not_queued_does_nothing() {
    let rig = Rig::new();
    let (dory, [a, b, c]) = held_queue(&rig);
    dory.pipeline().cancel(a).unwrap();
    dory.pipeline().move_job(a, 0).unwrap();
    dory.pipeline().move_job(JobId(999), 0).unwrap();
    assert_eq!(queued_ids(&dory), ids(&[b, c]));
    assert_eq!(
        dory.pipeline().job(a).unwrap().unwrap().state,
        JobState::Cancelled
    );
}

#[test]
fn a_snippet_quotes_the_translation_when_the_match_is_there() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = RecordingId::parse("talk").unwrap();
    let transcript = Transcript {
        revision: Revision(1),
        segments: vec![
            segment(0, 1_000, 1, Channel::System, "Aprubado na ang badyet."),
            segment(1_000, 2_000, 2, Channel::System, "Okay."),
        ],
        detected_languages: Vec::new(),
        produced_by: ModelRef::parse(STT).unwrap(),
    };
    let translation = Translation {
        target: LangTag::parse("en").unwrap(),
        transcript_revision: Revision(1),
        produced_by: ModelRef::parse(LLM).unwrap(),
        texts: vec![Some("The budget is approved.".into()), None],
    };
    rig.store
        .apply(vec![
            Change::RecordingCreated(recording(&id, None)),
            Change::TranscriptReplaced {
                id: id.clone(),
                transcript,
            },
            Change::TranslationReplaced {
                id: id.clone(),
                translation,
            },
        ])
        .unwrap();
    let library = dory.library();
    let quoted = |text: &str| {
        let query = Query::parse(text).unwrap();
        let found = library.snippets(&query, std::slice::from_ref(&id)).unwrap();
        let e = found[0].clone().expect("a snippet");
        let marked: Vec<String> = e
            .snippet
            .marks
            .iter()
            .map(|m| e.snippet.text[m.clone()].to_owned())
            .collect();
        (e.source, e.item, e.snippet.text, marked)
    };
    assert!(
        library
            .search("approved")
            .unwrap()
            .is_some_and(|r| r.hits.iter().any(|h| h.row.recording.id == id)),
        "the translation is searched"
    );
    assert_eq!(
        quoted("approved"),
        (
            Where::Transcript,
            0,
            "The budget is approved.".to_owned(),
            vec!["approved".to_owned()]
        ),
        "a translated word is quoted from the translation"
    );
    assert_eq!(
        quoted("aprubado"),
        (
            Where::Transcript,
            0,
            "Aprubado na ang badyet.".to_owned(),
            vec!["Aprubado".to_owned()]
        ),
        "a spoken word is quoted as spoken"
    );
}

#[test]
fn snippets_come_from_turns_then_summary() {
    let rig = Rig::new();
    let dory = rig.open();
    let id = |name: &str| RecordingId::parse(name).unwrap();
    let made = |name: &str, revision: u32, lines: &[(u16, &str)], markdown: Option<&str>| {
        let mut changes = vec![Change::RecordingCreated(recording(&id(name), None))];
        if !lines.is_empty() {
            changes.push(Change::TranscriptReplaced {
                id: id(name),
                transcript: Transcript {
                    revision: Revision(revision),
                    segments: lines
                        .iter()
                        .map(|(speaker, text)| segment(0, 1_000, *speaker, Channel::System, text))
                        .collect(),
                    detected_languages: Vec::new(),
                    produced_by: ModelRef::parse(STT).unwrap(),
                },
            });
        }
        if let Some(markdown) = markdown {
            changes.push(Change::SummaryReplaced {
                id: id(name),
                summary: Summary {
                    markdown: markdown.into(),
                    language: None,
                    produced_by: ModelRef::parse(LLM).unwrap(),
                    transcript_revision: Revision(revision),
                },
            });
        }
        rig.store.apply(changes).unwrap();
    };
    made(
        "talk",
        1,
        &[
            (1, "Welcome everyone."),
            (2, "We agreed the roadmap slips"),
            (2, "to the third quarter."),
        ],
        Some("## Notes\n- The roadmap is in the summary too"),
    );
    made(
        "notes",
        1,
        &[(1, "Nothing about it here.")],
        Some("## Decisions\n- **Budget** approved"),
    );
    made("named", 1, &[(7, "Hello."), (8, "Hi.")], None);
    made("empty", 1, &[], None);
    rig.store
        .apply(vec![Change::SpeakerNamesReplaced {
            id: id("named"),
            names: SpeakerNames {
                for_revision: Revision(1),
                names: [(SpeakerId(7), "Rita Roadmap".to_owned())].into(),
            },
        }])
        .unwrap();

    let library = dory.library();
    let ids = [
        id("talk"),
        id("notes"),
        id("named"),
        id("empty"),
        id("gone"),
    ];
    let shown = |text: &str| {
        let query = Query::parse(text).unwrap();
        library
            .snippets(&query, &ids)
            .unwrap()
            .into_iter()
            .map(|found| {
                found.map(|e| {
                    let marked: Vec<_> = e
                        .snippet
                        .marks
                        .iter()
                        .map(|m| e.snippet.text[m.clone()].to_owned())
                        .collect();
                    (e.source, e.snippet.text, marked)
                })
            })
            .collect::<Vec<_>>()
    };
    let road = |s: &str| Some((Where::Transcript, s.to_owned(), vec!["roadmap".to_owned()]));
    assert_eq!(
        shown("roadmap"),
        [
            road("We agreed the roadmap slips to the third quarter."),
            None,
            Some((
                Where::Speakers,
                "Rita Roadmap".to_owned(),
                vec!["Roadmap".to_owned()]
            )),
            None,
            None,
        ],
        "a turn joins its segments, and a name is the last resort"
    );
    assert_eq!(
        shown("budget")[1],
        Some((
            Where::Summary,
            "Budget approved".to_owned(),
            vec!["Budget".to_owned()]
        )),
        "the summary, without its Markdown"
    );
    assert_eq!(
        shown("roadmap summary")[0].as_ref().map(|e| e.0),
        Some(Where::Transcript),
        "the transcript wins when both hold a word"
    );
}
