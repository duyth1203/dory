//! Drives `Dory` through its public facade with a fake for every port.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use dory_core::app::{Dory, Event, OpenError, Options, Ports, RecorderStatus, StartOptions};
use dory_core::domain::{
    Blocker, Channel, ChannelLayout, Goal, Job, JobEvent, JobState, LangTag, Ms, Priority,
    Recording, RecordingId, RecordingQuery, Revision, Segment, Settings, SpeakerId, Stage,
    TakeContainer, TakeInfo, TimeRange, Timestamp,
};
use dory_core::models::{
    Availability, CatalogError, Choice, DiarizationSupport, KindTag, ModelId, ModelRef, Stt,
};
use dory_core::ports::capture::{CaptureError, Source};
use dory_core::ports::{Change, Store, StoreRead, Takes};
use dory_core::recorder::{FinalizeReason, Limits};
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeSttProvider, FakeTranscriber, ManualClock,
    MemTakes, Step, block_on, eventually, llm_entry, stt_entry,
};

const RATE: u32 = 8_000;
const STT: &str = "fake/stt";
const LLM: &str = "fake/llm";
const SUMMARY: &str = "## Key points\n- Ship it Friday.";

struct Rig {
    store: Arc<MemStore>,
    takes: MemTakes,
    capture: Arc<FakeCapture>,
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
        Rig {
            store: Arc::new(MemStore::new()),
            takes: MemTakes::new(),
            capture: Arc::new(FakeCapture::new()),
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
            store: self.store.clone(),
            takes: Arc::new(self.takes.clone()),
            capture: self.capture.clone(),
            clock: self.clock.clone(),
            stt: vec![self.stt.clone()],
            llm: vec![self.llm_provider.clone()],
            diar: vec![],
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
        writer.write(&vec![0.0; RATE as usize * 2]).unwrap();
        writer.finish().unwrap();
        let take = self.takes.seal(&id).unwrap();
        self.store
            .apply(vec![Change::RecordingCreated(recording(&id, Some(take)))])
            .unwrap();
        id
    }
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
    assert!(prompts[0].0.contains("tag vi"), "{}", prompts[0].0);
    assert!(
        prompts[0].1.contains("Me: Ship it Friday."),
        "{}",
        prompts[0].1
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
    assert_eq!(recovery.deleted_orphans, vec![stray.clone()]);
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
    std::thread::sleep(Duration::from_millis(30));
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
    assert_eq!(dory.recorder().status(), RecorderStatus::Idle);
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
