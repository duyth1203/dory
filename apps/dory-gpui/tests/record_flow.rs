//! P5's record flow through the window: a real `Dory` over the sample store, the core's scripted
//! `FakeCapture`, and a `ManualClock` that alone ends the undo grace. The recorder and pipeline
//! run on their own threads, so each wait polls the window until the state arrives.
//!
//! The processing card's states (`docs/design/processing-view.md` section 4) are driven here too:
//! gates hold the seal, the speech model's load, the engine, and the summary, so each state stays
//! on screen until the test has read it.

mod common;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Options, Ports, RecorderStatus};
use dory_core::domain::{
    Blocker, Channel, ChannelLayout, FailureClass, Goal, Job, JobError, JobEvent, JobState, Levels,
    Ms, Priority, RecordingId, Stage, TakeInfo, TimeRange, Timestamp,
};
use dory_core::models::{
    Availability, DiarizationSupport, KindTag, Llm, ModelEntry, ModelId, ProviderId, Stt,
};
use dory_core::ports::{
    CancelToken, Change, Clip, CompletionRequest, Conditions, EngineError, LanguageModel,
    LegacyFile, ModelProvider, Power, ProvisionError, ProvisionSink, SegmentSink, Store as _,
    StoreRead as _, TakeError, TakeWriter, Takes, TextSink, Thermal, TranscribeOutcome,
    TranscribeRequest, Transcriber,
};
use dory_core::recorder::Limits;
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeMachine, FakeSttProvider, FakeTranscriber,
    FakeTrash, ManualClock, MemTakes, Step, llm_entry, stt_entry,
};
use dory_gpui::app::AppState;
use dory_gpui::fixtures;
use dory_gpui::library::Selection;
use dory_gpui::menus::{self, Recorder};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, TestAppContext, test::TestWindowExt as _,
};

use crate::common::{id_of, open_with, sample_store};

const RATE: u32 = 8_000;
const STT: &str = "fake/stt";
const LLM: &str = "fake/llm";

struct Rig {
    store: Arc<MemStore>,
    capture: Arc<FakeCapture>,
    clock: Arc<ManualClock>,
    stt: Vec<Arc<dyn ModelProvider<Stt>>>,
    llm: Vec<Arc<dyn ModelProvider<Llm>>>,
    machine: Arc<FakeMachine>,
    /// Holds sealing and fails clips, when a test asks.
    takes: Arc<GatedTakes>,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            store: sample_store(),
            capture: Arc::new(FakeCapture::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: Vec::new(),
            machine: Arc::new(FakeMachine::default()),
            takes: Arc::new(GatedTakes::default()),
        }
    }

    fn auto_process(self, on: bool) -> Rig {
        let mut settings = self.store.settings().unwrap();
        settings.auto_process = on;
        self.store
            .apply(vec![Change::SettingsReplaced(settings)])
            .unwrap();
        self
    }

    fn open(&self, cx: &mut TestAppContext, limits: Limits) -> (AnyWindowHandle, Entity<AppState>) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dory-record-flow-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        *self.takes.inner.lock().unwrap() = Some(MemTakes::in_dir(&dir));
        let dory = Dory::open(
            Ports {
                store: self.store.clone(),
                takes: self.takes.clone(),
                trash: Arc::new(FakeTrash::new(&dir)),
                capture: self.capture.clone(),
                playback: Arc::new(FakePlayback::new()),
                clock: self.clock.clone(),
                stt: self.stt.clone(),
                llm: self.llm.clone(),
                diar: Vec::new(),
                machine: self.machine.clone(),
                guard: None,
            },
            Options {
                limits,
                clock_poll: Duration::from_millis(2),
                level_every: Duration::from_millis(10),
                retry_base_ms: 1,
                ..Options::default()
            },
        )
        .unwrap();
        cx.executor().allow_parking();
        let opened = open_with(cx, dory);
        cx.update(menus::install);
        opened
    }

    fn jobs(&self, id: &RecordingId) -> Vec<Job> {
        self.store.jobs_for(id).unwrap()
    }
}

/// A failed assertion must not leave a take sealing forever, or the window never closes.
impl Drop for Rig {
    fn drop(&mut self) {
        self.takes.seal.release();
    }
}

fn limits() -> Limits {
    Limits {
        rate_hz: NonZeroU32::new(RATE).unwrap(),
        ..Limits::default()
    }
}

fn seconds(tenths: usize) -> Vec<Step> {
    (0..tenths)
        .map(|_| Step::Silence {
            frames: RATE as usize / 10,
        })
        .collect()
}

/// Polls until `ok`, letting the recorder and pipeline threads run between frames.
fn until(cx: &mut TestAppContext, what: &str, mut ok: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Lets the threads run a while, for asserting that something does not happen.
fn settle(cx: &mut TestAppContext) {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(5));
        cx.run_until_parked();
    }
}

fn status(app: &Entity<AppState>, cx: &mut TestAppContext) -> RecorderStatus {
    app.read_with(cx, |app, _| app.recorder().clone())
}

fn recorded(app: &Entity<AppState>, cx: &mut TestAppContext) -> Ms {
    app.read_with(cx, |app, cx| app.live().read(cx).recorded)
}

fn phase(app: &Entity<AppState>, cx: &mut TestAppContext) -> Recorder {
    app.read_with(cx, |app, cx| menus::menu_state(app, cx).recorder)
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    })
    .unwrap();
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
}

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|e| e.label().map(str::to_owned))
    })
    .unwrap()
}

fn focused(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).and_then(|e| e.focused()) == Some(true)
    })
    .unwrap()
}

/// Starts from the shortcut and waits until `tenths` of a second are recorded.
fn record(
    window: AnyWindowHandle,
    app: &Entity<AppState>,
    tenths: u64,
    cx: &mut TestAppContext,
) -> RecordingId {
    press(window, "cmd-shift-r", cx);
    until(cx, "the take to be written", |cx| {
        recorded(app, cx) == Ms(tenths * 100)
    });
    status(app, cx).id().cloned().unwrap()
}

fn stop_into_grace(window: AnyWindowHandle, app: &Entity<AppState>, cx: &mut TestAppContext) {
    press(window, "cmd-.", cx);
    until(cx, "grace", |cx| {
        matches!(status(app, cx), RecorderStatus::Grace { .. })
    });
}

fn end_grace(rig: &Rig, app: &Entity<AppState>, cx: &mut TestAppContext) {
    rig.clock.advance_ms(5_000);
    until(cx, "idle", |cx| status(app, cx) == RecorderStatus::Idle);
}

#[gpui_kit::test]
fn start_pause_resume_and_stop_follow_the_recorder(cx: &mut TestAppContext) {
    let rig = Rig::new();
    rig.capture.push_script(seconds(20));
    rig.capture.push_script(seconds(20));
    let (window, app) = rig.open(cx, limits());
    assert_eq!(
        label(window, "record", cx).as_deref(),
        Some("Start recording")
    );

    let id = record(window, &app, 20, cx);
    assert_eq!(phase(&app, cx), Recorder::Recording);
    assert_eq!(
        app.read_with(cx, |app, _| app.selection.clone()),
        Selection::One(id.clone()),
        "the new recording is selected"
    );
    assert_eq!(
        app.read_with(cx, |app, _| app.visible_ids()[0].clone()),
        id,
        "its row sits at the top"
    );
    let row = label(window, &format!("row-{id}"), cx).unwrap();
    assert!(row.ends_with(", Recording"), "{row}");
    assert_eq!(
        label(window, "record-pill", cx).as_deref(),
        Some("Recording, 2 seconds")
    );
    assert_eq!(
        label(window, "record", cx),
        None,
        "the pill replaces Record"
    );

    press(window, "cmd-shift-p", cx);
    until(cx, "paused", |cx| {
        matches!(status(&app, cx), RecorderStatus::Paused { .. })
    });
    assert_eq!(rig.capture.running(), 0, "pause releases the device");
    assert_eq!(
        label(window, "record-pill", cx).as_deref(),
        Some("Paused, 2 seconds")
    );
    assert_eq!(
        label(window, "record-pause", cx).as_deref(),
        Some("Resume recording")
    );
    assert_eq!(phase(&app, cx), Recorder::Paused);

    click(window, "record-pause", cx);
    until(cx, "4 s recorded after the resume", |cx| {
        recorded(&app, cx) == Ms(4_000)
    });

    click(window, "record-stop", cx);
    until(cx, "grace", |cx| {
        matches!(status(&app, cx), RecorderStatus::Grace { .. })
    });
    assert_eq!(phase(&app, cx), Recorder::Grace);
    assert_eq!(
        label(window, "undo-stop-toast", cx).as_deref(),
        Some("Recording saved. Undo, 5 seconds left")
    );
    assert_eq!(label(window, "record-pill", cx), None);
    assert_eq!(
        label(window, "record", cx).as_deref(),
        Some("Start recording")
    );
}

#[gpui_kit::test]
fn undo_inside_the_grace_window_resumes_into_the_same_take(cx: &mut TestAppContext) {
    let rig = Rig::new();
    rig.capture.push_script(seconds(10));
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    let before = app.read_with(cx, |app, _| app.recordings.len());

    let id = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    click(window, "undo-stop", cx);
    until(cx, "2 s in the same take after the undo", |cx| {
        status(&app, cx).id() == Some(&id) && recorded(&app, cx) == Ms(2_000)
    });
    assert!(matches!(status(&app, cx), RecorderStatus::Recording { .. }));
    assert_eq!(label(window, "undo-stop-toast", cx), None);
    assert!(rig.jobs(&id).is_empty(), "the undo queued nothing");

    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "the take to be listed with its audio", |cx| {
        app.read_with(cx, |app, _| {
            app.recordings
                .iter()
                .any(|r| r.recording.id == id && r.recording.take.is_some())
        })
    });
    let take = rig.store.recording(&id).unwrap().unwrap().take.unwrap();
    assert_eq!(take.duration, Ms(2_000), "one file holds both halves");
    assert_eq!(app.read_with(cx, |app, _| app.recordings.len()), before + 1);
}

#[gpui_kit::test]
fn there_is_no_undo_after_the_grace_window(cx: &mut TestAppContext) {
    let rig = Rig::new();
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    assert_eq!(label(window, "undo-stop-toast", cx), None);
    assert_eq!(phase(&app, cx), Recorder::Idle);

    app.update(cx, |app, cx| app.undo_stop(cx));
    settle(cx);
    assert_eq!(status(&app, cx), RecorderStatus::Idle);
    assert_eq!(rig.capture.start_count(), 1, "nothing started again");
}

#[gpui_kit::test]
fn nothing_is_queued_until_the_grace_ends_and_then_once(cx: &mut TestAppContext) {
    let rig = Rig::new();
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    let id = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    settle(cx);
    assert!(rig.jobs(&id).is_empty(), "queued inside the grace window");
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some("Recording stopped. Transcription starts in 5 seconds."),
        "the card shows from the moment Stop is pressed"
    );
    assert_eq!(
        label(window, "card-step-0", cx).as_deref(),
        Some("Recording, Stopped")
    );
    assert_eq!(
        label(window, "card-step-1", cx).as_deref(),
        Some("Transcript, Starts in 5 s")
    );

    end_grace(&rig, &app, cx);
    until(cx, "a job", |_| !rig.jobs(&id).is_empty());
    settle(cx);
    let jobs = rig.jobs(&id);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].goal, Goal::ALL);
    assert_eq!(
        label(window, "finish-card", cx),
        None,
        "automatic processing shows no finish card"
    );
}

#[gpui_kit::test]
fn the_cap_warns_turns_the_pill_amber_then_saves_without_a_grace(cx: &mut TestAppContext) {
    let rig = Rig::new();
    rig.capture.push_script(seconds(25));
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(
        cx,
        Limits {
            cap: Ms(3_000),
            warn_before: Ms(1_000),
            ..limits()
        },
    );
    let id = record(window, &app, 25, cx);
    assert!(
        app.read_with(cx, |app, cx| app.live().read(cx).cap_at.is_some()),
        "the warning arrived"
    );
    let pill = label(window, "record-pill", cx).unwrap();
    assert!(pill.ends_with("left"), "{pill}");

    press(window, "cmd-shift-p", cx);
    until(cx, "paused", |cx| {
        matches!(status(&app, cx), RecorderStatus::Paused { .. })
    });
    press(window, "cmd-shift-p", cx);
    until(cx, "the cap to end the take", |cx| {
        status(&app, cx) == RecorderStatus::Idle
    });
    until(cx, "a job, with no grace to wait out", |_| {
        !rig.jobs(&id).is_empty()
    });
    assert_eq!(label(window, "undo-stop-toast", cx), None);
}

#[gpui_kit::test]
fn with_automatic_processing_off_the_finish_card_queues_what_was_picked(cx: &mut TestAppContext) {
    let rig = Rig::new().auto_process(false);
    rig.capture.push_script(seconds(10));
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());

    let first = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "the finish card", |cx| {
        label(window, "finish-card", cx).is_some()
    });
    assert!(rig.jobs(&first).is_empty(), "nothing queued by itself");
    assert!(
        focused(window, "finish-all", cx),
        "the primary button has focus"
    );
    assert_eq!(
        label(window, "finish-estimate", cx).as_deref(),
        Some("Transcribing takes about 1 minute on this Mac.")
    );
    let row = label(window, &format!("row-{first}"), cx).unwrap();
    assert!(row.ends_with(", Ready to transcribe"), "{row}");

    click(window, "finish-transcript", cx);
    until(cx, "a transcript-only job", |_| {
        !rig.jobs(&first).is_empty()
    });
    assert_eq!(rig.jobs(&first)[0].goal, Goal::TRANSCRIBE);
    assert!(!rig.store.settings().unwrap().auto_process);

    let second = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "the second finish card", |cx| {
        label(window, "finish-card", cx).is_some()
    });
    click(window, "finish-always", cx);
    click(window, "finish-all", cx);
    until(cx, "a full job", |_| !rig.jobs(&second).is_empty());
    assert_eq!(rig.jobs(&second)[0].goal, Goal::ALL);
    until(cx, "automatic processing back on", |cx| {
        app.read_with(cx, |app, _| app.auto_process())
    });
    assert!(rig.store.settings().unwrap().auto_process);
}

fn seed(rig: &Rig, id: &RecordingId, events: Vec<JobEvent>) {
    rig.store
        .apply(vec![Change::JobEnqueued {
            id: id.clone(),
            goal: Goal::ALL,
            priority: Priority::Normal,
            at: Timestamp(fixtures::now().timestamp_millis()),
        }])
        .unwrap();
    let job = rig.jobs(id)[0].id;
    let events = events
        .into_iter()
        .map(|event| Change::Job { job, event })
        .collect();
    rig.store.apply(events).unwrap();
}

fn select(app: &Entity<AppState>, id: &RecordingId, cx: &mut TestAppContext) {
    let id = id.clone();
    app.update(cx, |app, cx| app.select(Selection::One(id), cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn try_again_retries_a_failed_job(cx: &mut TestAppContext) {
    let rig = Rig::new();
    let id = id_of("Weekly sync");
    seed(
        &rig,
        &id,
        vec![
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
            JobEvent::Failed {
                stage: Stage::Transcribe,
                error: JobError {
                    class: FailureClass::Permanent,
                    message: "The upload broke.".into(),
                    cause: dory_core::domain::FailureCause::Other,
                },
            },
        ],
    );
    let (window, app) = rig.open(cx, limits());
    select(&app, &id, cx);
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some(
            "Transcription didn't finish. Your recording is saved, but transcription failed: The upload broke. Try Again."
        )
    );
    assert_eq!(label(window, "card-fix", cx).as_deref(), Some("Try Again"));
    assert!(
        !focused(window, "card-fix", cx),
        "selecting a failed recording leaves focus where it was"
    );

    click(window, "card-fix", cx);
    until(
        cx,
        "the retry to run and block on the missing speech model",
        |_| {
            rig.jobs(&id)[0].state
                == JobState::Blocked {
                    stage: Stage::Transcribe,
                    blocker: Blocker::NoModel { kind: KindTag::Stt },
                }
        },
    );
    until(cx, "the new problem", |cx| {
        label(window, "card-status", cx).as_deref()
            == Some(
                "Transcription didn't start. Your recording is saved, but no speech model is set up. Choose a Speech Model.",
            )
    });
    assert_eq!(
        label(window, "card-fix", cx).as_deref(),
        Some("Choose a Speech Model…")
    );
    assert!(
        focused(window, "card-fix", cx),
        "the fix takes focus when the new failure appears"
    );
}

#[gpui_kit::test]
fn a_missing_summary_model_offers_set_up_summaries(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    let model: ModelId<Llm> = LLM.parse().unwrap();
    let llm = Arc::new(FakeLlmProvider::new(
        "fake",
        vec![llm_entry(LLM)],
        Arc::new(FakeLanguageModel::new("## Key points")),
    ));
    llm.set_availability(
        &model,
        Availability::NeedsProvision {
            bytes: Some(4_000_000_000),
        },
    );
    rig.llm = vec![llm.clone()];
    let id = id_of("Hiring debrief");
    seed(
        &rig,
        &id,
        vec![
            JobEvent::Started {
                stage: Stage::Transcribe,
            },
            JobEvent::StageDone {
                stage: Stage::Transcribe,
            },
            JobEvent::Started {
                stage: Stage::Summarize,
            },
            JobEvent::Blocked {
                stage: Stage::Summarize,
                blocker: Blocker::NeedsProvision {
                    model: model.erase(),
                    bytes: Some(4_000_000_000),
                },
            },
        ],
    );
    let (window, app) = rig.open(cx, limits());
    select(&app, &id, cx);
    until(cx, "the summary's problem", |cx| {
        label(window, "summary-missing", cx).as_deref()
            == Some("The transcript is ready, but summaries aren't set up.")
    });
    assert_eq!(
        label(window, "summary-fix", cx).as_deref(),
        Some("Set Up Summaries")
    );
    assert!(
        label(window, "player-root", cx).is_some()
            && label(window, "processing-card", cx).is_none(),
        "the transcript is ready, so the card is the player and only the Summary tab shows it"
    );
    let row = label(window, &format!("row-{id}"), cx).unwrap();
    assert!(row.ends_with(", Needs attention"), "{row}");
    click(window, "summary-fix", cx);
    settle(cx);
    assert!(matches!(rig.jobs(&id)[0].state, JobState::Blocked { .. }));
    assert_eq!(llm.provisions(), 0, "the sheet explains, it does not pull");
}

#[gpui_kit::test]
fn a_missing_speech_model_downloads_from_the_row_then_runs(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    let model: ModelId<Stt> = STT.parse().unwrap();
    let stt = Arc::new(FakeSttProvider::new(
        "fake",
        vec![stt_entry(STT, DiarizationSupport::Native)],
        Arc::new(FakeTranscriber::new().with_segments(Channel::Mic, &[(0, 900, 0, "Hello.")])),
    ));
    stt.set_availability(
        &model,
        Availability::NeedsProvision {
            bytes: Some(487_211_443),
        },
    );
    rig.stt = vec![stt.clone()];
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());

    let id = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "the job to block on the download", |_| {
        rig.jobs(&id).first().is_some_and(|job| {
            matches!(
                &job.state,
                JobState::Blocked {
                    blocker: Blocker::NeedsProvision { .. },
                    ..
                }
            )
        })
    });
    until(cx, "the Download button", |cx| {
        label(window, "card-fix", cx).as_deref() == Some("Download fake/stt, 487 megabytes")
    });
    assert_eq!(
        label(window, "card-headline", cx).as_deref(),
        Some("Transcription didn't start")
    );
    assert!(
        focused(window, "card-fix", cx),
        "the fix takes focus when the failure appears"
    );
    assert_eq!(stt.provisions(), 0, "nothing downloads by itself");

    click(window, "card-fix", cx);
    until(cx, "the download", |_| stt.provisions() == 1);
    until(cx, "the job to run once the model is here", |_| {
        rig.jobs(&id)[0].state == JobState::Succeeded
    });
    until(cx, "the player in place of the card", |cx| {
        label(window, "card-fix", cx).is_none() && label(window, "player-root", cx).is_some()
    });
}

/// A recording is on screen before its take is saved, so the player first knows no length.
/// Once the take lands, the player shows its length, and the skip buttons can move.
#[gpui_kit::test]
fn the_player_learns_the_length_once_the_take_is_saved(cx: &mut TestAppContext) {
    let rig = Rig::new().auto_process(false);
    rig.capture.push_script(seconds(30));
    let (window, app) = rig.open(cx, limits());
    record(window, &app, 30, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "the player to show the take's length", |cx| {
        label(window, "player-time", cx).is_some_and(|t| t.ends_with("/ 0:03"))
    });
}

/// Holds whatever waits on it until the test releases it, or the work is cancelled.
#[derive(Default)]
struct Gate(AtomicBool);

impl Gate {
    fn held() -> Gate {
        Gate(AtomicBool::new(true))
    }

    fn hold(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn release(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    fn wait(&self, cancel: Option<&CancelToken>) {
        while self.0.load(Ordering::SeqCst) && !cancel.is_some_and(CancelToken::is_cancelled) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// `MemTakes` whose sealing a test can hold and whose clips a test can fail.
#[derive(Default)]
struct GatedTakes {
    inner: Mutex<Option<MemTakes>>,
    seal: Gate,
    clip_error: Mutex<Option<fn() -> TakeError>>,
}

impl GatedTakes {
    fn inner(&self) -> MemTakes {
        self.inner.lock().unwrap().clone().expect("the rig opened")
    }
}

impl Takes for GatedTakes {
    fn begin(
        &self,
        id: &RecordingId,
        layout: ChannelLayout,
        rate_hz: u32,
    ) -> Result<Box<dyn TakeWriter>, TakeError> {
        self.inner().begin(id, layout, rate_hz)
    }
    fn seal(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        self.seal.wait(None);
        self.inner().seal(id)
    }
    fn clip(
        &self,
        id: &RecordingId,
        channel: Channel,
        span: Option<TimeRange>,
    ) -> Result<Clip, TakeError> {
        if let Some(error) = *self.clip_error.lock().unwrap() {
            return Err(error());
        }
        self.inner().clip(id, channel, span)
    }
    fn slice(&self, whole: &Clip, span: TimeRange) -> Result<Clip, TakeError> {
        self.inner().slice(whole, span)
    }
    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError> {
        self.inner().orphans()
    }
    fn stored(&self) -> Result<Vec<RecordingId>, TakeError> {
        self.inner().stored()
    }
    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError> {
        self.inner().inspect(id)
    }
    fn import(&self, id: &RecordingId, source: &Path) -> Result<TakeInfo, TakeError> {
        self.inner().import(id, source)
    }
    fn legacy_original(&self, id: &RecordingId) -> Result<Option<LegacyFile>, TakeError> {
        self.inner().legacy_original(id)
    }
    fn adopt(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        self.inner().adopt(id)
    }
    fn levels(&self, id: &RecordingId) -> Result<Levels, TakeError> {
        self.inner().levels(id)
    }
    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        self.inner().playback_path(id)
    }
    fn delete(&self, id: &RecordingId) -> Result<(), TakeError> {
        self.inner().delete(id)
    }
    fn files(&self, id: &RecordingId) -> Result<Vec<PathBuf>, TakeError> {
        self.inner().files(id)
    }
}

/// A speech model whose load waits at a gate, so "Getting ready to transcribe" stays up.
struct SlowToLoad {
    inner: FakeSttProvider,
    load: Gate,
}

impl SlowToLoad {
    fn new(engine: Arc<dyn Transcriber>) -> Arc<SlowToLoad> {
        Arc::new(SlowToLoad {
            inner: FakeSttProvider::new(
                "fake",
                vec![stt_entry(STT, DiarizationSupport::Native)],
                engine,
            ),
            load: Gate::held(),
        })
    }
}

impl ModelProvider<Stt> for SlowToLoad {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }
    fn catalog(&self) -> Vec<ModelEntry<Stt>> {
        self.inner.catalog()
    }
    fn availability(&self, model: &ModelId<Stt>) -> Availability {
        self.inner.availability(model)
    }
    fn provision(
        &self,
        model: &ModelId<Stt>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError> {
        self.inner.provision(model, progress, cancel)
    }
    fn open(
        &self,
        model: &ModelId<Stt>,
        cancel: &CancelToken,
    ) -> Result<Arc<dyn Transcriber>, EngineError> {
        self.load.wait(Some(cancel));
        self.inner.open(model, cancel)
    }
}

/// A summary model that writes only once the test lets it.
struct SlowSummary {
    inner: FakeLanguageModel,
    write: Gate,
}

impl LanguageModel for SlowSummary {
    fn count_tokens(&self, text: &str) -> u32 {
        self.inner.count_tokens(text)
    }

    fn complete(
        &self,
        req: &CompletionRequest<'_>,
        sink: &mut dyn TextSink,
        cancel: &CancelToken,
    ) -> Result<String, EngineError> {
        self.write.wait(Some(cancel));
        self.inner.complete(req, sink, cancel)
    }
}

/// A speech engine whose runtime dies on every call.
struct Crashing;

impl Transcriber for Crashing {
    fn transcribe(
        &self,
        _: &TranscribeRequest<'_>,
        _: &mut dyn SegmentSink,
        _: &CancelToken,
    ) -> Result<TranscribeOutcome, EngineError> {
        Err(EngineError::Crashed("the engine stopped".into()))
    }
}

fn row_word(window: AnyWindowHandle, id: &RecordingId, cx: &mut TestAppContext) -> String {
    label(window, &format!("row-{id}"), cx).unwrap_or_default()
}

/// A take of two seconds on both channels: the mic says one line, and the system channel is
/// the engine's second call, which the test can hold at 50%.
fn speaking_engine() -> Arc<FakeTranscriber> {
    Arc::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(0, 900, 0, "We ship on Friday.")]),
    )
}

#[gpui_kit::test]
fn the_card_walks_from_stop_to_the_player(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    let engine = speaking_engine();
    engine.hold_from(1);
    let stt = SlowToLoad::new(engine.clone());
    let summary = Arc::new(SlowSummary {
        inner: FakeLanguageModel::new("## Key points\n- We ship on Friday."),
        write: Gate::held(),
    });
    rig.stt = vec![stt.clone()];
    rig.llm = vec![Arc::new(FakeLlmProvider::new(
        "fake",
        vec![llm_entry(LLM)],
        summary.clone(),
    ))];
    rig.capture.push_script(seconds(20));
    rig.takes.seal.hold();
    let (window, app) = rig.open(cx, limits());
    let id = record(window, &app, 20, cx);

    stop_into_grace(window, &app, cx);
    assert_eq!(
        label(window, "card-headline", cx).as_deref(),
        Some("Recording stopped"),
        "P1"
    );
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some("Recording stopped. Transcription starts in 5 seconds.")
    );
    assert!(row_word(window, &id, cx).ends_with(", Stopped"));
    assert_eq!(
        label(window, "transcript-placeholder", cx).as_deref(),
        Some("The transcript appears here, line by line, as it is written."),
        "the Transcript tab waits for the lines"
    );

    end_grace(&rig, &app, cx);
    until(cx, "P2", |cx| {
        label(window, "card-headline", cx).as_deref() == Some("Saving the recording")
    });
    assert_eq!(
        label(window, "card-detail", cx).as_deref(),
        Some("2 seconds of audio. This takes a moment.")
    );
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some("Saving the recording.")
    );
    assert!(row_word(window, &id, cx).ends_with(", Saving"));

    rig.takes.seal.release();
    until(cx, "P3b", |cx| {
        label(window, "card-headline", cx).as_deref() == Some("Getting ready to transcribe")
    });
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some("Getting ready to transcribe. Loading the speech model.")
    );
    assert!(row_word(window, &id, cx).ends_with(", Preparing"));

    stt.load.release();
    until(cx, "P4 with the first line", |cx| {
        label(window, "draft-0", cx).is_some()
            && label(window, "card-status", cx).as_deref()
                == Some("Transcribing, 50%. Estimating time left.")
    });
    assert_eq!(
        label(window, "card-headline", cx).as_deref(),
        Some("Transcribing")
    );
    assert_eq!(
        label(window, "card-detail", cx).as_deref(),
        Some("50% · estimating time left")
    );
    assert!(row_word(window, &id, cx).ends_with(", Transcribing 50%"));
    assert_eq!(
        label(window, "draft-0", cx).as_deref(),
        Some("0 seconds, We ship on Friday."),
        "the line streams into the Transcript tab"
    );
    assert_eq!(
        label(window, "transcript-footnote", cx).as_deref(),
        Some("Speaker names are added when transcription finishes.")
    );
    assert_eq!(
        label(window, "card-step-1", cx).as_deref(),
        Some("Transcript, 50%")
    );

    engine.hold(false);
    until(cx, "P6", |cx| {
        label(window, "card-headline", cx).as_deref() == Some("Writing the summary")
    });
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some("Transcript ready. Writing the summary.")
    );
    assert!(row_word(window, &id, cx).ends_with(", Summarizing"));
    assert_eq!(
        app.read_with(cx, |app, _| app.tab),
        dory_gpui::app::DetailTab::Transcript,
        "the finished transcript is the tab to read while the summary is written"
    );

    summary.write.release();
    until(cx, "P7, the player in the same place", |cx| {
        label(window, "processing-card", cx).is_none() && label(window, "player-root", cx).is_some()
    });
    assert!(row_word(window, &id, cx).ends_with(", Done"));
    assert_eq!(
        app.read_with(cx, |app, _| app.tab),
        dory_gpui::app::DetailTab::Summary,
        "the summary shows once it is ready"
    );
    let sentence = app.read_with(cx, |app, _| {
        let row = app.row(&id).unwrap();
        app.phase(row)
            .sentence(&dory_gpui::library::title(row, &chrono::Local))
    });
    assert!(
        sentence.starts_with("Summary ready for "),
        "what VoiceOver says when it is done: {sentence}"
    );
}

#[gpui_kit::test]
fn the_summary_tab_waits_when_the_user_picked_the_transcript(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    let summary = Arc::new(SlowSummary {
        inner: FakeLanguageModel::new("## Key points\n- We ship on Friday."),
        write: Gate::held(),
    });
    with_engine(&mut rig, speaking_engine());
    rig.llm = vec![Arc::new(FakeLlmProvider::new(
        "fake",
        vec![llm_entry(LLM)],
        summary.clone(),
    ))];
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    let id = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "P6", |cx| {
        label(window, "card-headline", cx).as_deref() == Some("Writing the summary")
    });
    click(window, "transcript-tab", cx);
    summary.write.release();
    until(cx, "P7", |cx| label(window, "player-root", cx).is_some());
    assert!(row_word(window, &id, cx).ends_with(", Done"));
    assert_eq!(
        app.read_with(cx, |app, _| app.tab),
        dory_gpui::app::DetailTab::Transcript,
        "the user is reading the transcript, so it stays"
    );
}

#[gpui_kit::test]
fn a_queued_recording_says_how_many_are_ahead_and_moves_to_top(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    let stt = SlowToLoad::new(speaking_engine());
    rig.stt = vec![stt.clone()];
    let first = id_of("1:1 with Sam");
    seed(&rig, &first, Vec::new());
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    until(cx, "the first job to load the model", |_| {
        matches!(rig.jobs(&first)[0].state, JobState::Running { .. })
    });
    let id = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "P3a", |cx| {
        label(window, "card-headline", cx).as_deref() == Some("Waiting to transcribe")
    });
    assert_eq!(
        label(window, "card-detail", cx).as_deref(),
        Some("1 recording is ahead of this one.")
    );
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some("Waiting to transcribe. 1 recording ahead.")
    );
    assert!(row_word(window, &id, cx).ends_with(", Queued, 1 ahead"));
    assert_eq!(
        label(window, "card-step-1", cx).as_deref(),
        Some("Transcript, 1 ahead")
    );

    click(window, "card-move-to-top", cx);
    settle(cx);
    assert_eq!(
        rig.jobs(&id)[0].state,
        JobState::Queued,
        "Move to Top makes a job next; it never interrupts the running one"
    );
    assert!(matches!(
        rig.jobs(&first)[0].state,
        JobState::Running { .. }
    ));
    stt.load.release();
}

#[gpui_kit::test]
fn running_light_says_why_and_use_fast_this_time_overrides_it(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    rig.machine.set(Conditions {
        power: Power::Battery,
        low_power: false,
        thermal: Thermal::Nominal,
    });
    let engine = speaking_engine();
    engine.hold_from(1);
    rig.stt = vec![Arc::new(FakeSttProvider::new(
        "fake",
        vec![stt_entry(STT, DiarizationSupport::Native)],
        engine.clone(),
    ))];
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    let id = record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "P5", |cx| label(window, "card-light", cx).is_some());
    assert_eq!(
        label(window, "card-light", cx).as_deref(),
        Some("Light, on battery")
    );
    assert_eq!(
        label(window, "card-status", cx).as_deref(),
        Some(
            "Transcribing, 50%. Estimating time left. Running light because the Mac is on battery."
        )
    );
    assert!(row_word(window, &id, cx).ends_with(", 50%, light"));
    assert_eq!(
        label(window, "card-step-1", cx).as_deref(),
        Some("Transcript, 50%, light")
    );

    click(window, "card-run-fast", cx);
    until(cx, "the job to run fast", |_| rig.jobs(&id)[0].run_fast);
    assert_eq!(
        rig.store.settings().unwrap().processing,
        dory_core::domain::Processing::Auto,
        "the setting stays"
    );
    engine.hold(false);
}

/// One UX-13 failure: how the rig makes it, and what the card then says and offers.
struct Failure {
    name: &'static str,
    setup: fn(&mut Rig),
    headline: &'static str,
    sentence: &'static str,
    button: &'static str,
}

fn with_engine(rig: &mut Rig, engine: Arc<dyn Transcriber>) {
    rig.stt = vec![Arc::new(FakeSttProvider::new(
        "fake",
        vec![stt_entry(STT, DiarizationSupport::Native)],
        engine,
    ))];
}

#[gpui_kit::test]
fn each_failure_says_what_to_do_and_its_fix_takes_focus(cx: &mut TestAppContext) {
    let failures = [
        Failure {
            name: "speech model missing",
            setup: |rig| {
                let stt = Arc::new(FakeSttProvider::new(
                    "fake",
                    vec![stt_entry(STT, DiarizationSupport::Native)],
                    Arc::new(FakeTranscriber::new()),
                ));
                stt.set_availability(
                    &STT.parse().unwrap(),
                    Availability::NeedsProvision {
                        bytes: Some(487_211_443),
                    },
                );
                rig.stt = vec![stt];
            },
            headline: "Transcription didn't start",
            sentence: "Transcription didn't start. Your recording is saved, but the speech model isn't on this Mac. Download fake/stt, 487 megabytes.",
            button: "Download fake/stt, 487 megabytes",
        },
        Failure {
            name: "no speech model",
            setup: |rig| rig.stt = Vec::new(),
            headline: "Transcription didn't start",
            sentence: "Transcription didn't start. Your recording is saved, but no speech model is set up. Choose a Speech Model.",
            button: "Choose a Speech Model…",
        },
        Failure {
            name: "disk full",
            setup: |rig| {
                with_engine(rig, Arc::new(FakeTranscriber::new()));
                *rig.takes.clip_error.lock().unwrap() = Some(|| TakeError::DiskFull);
            },
            headline: "Transcription didn't finish",
            sentence: "Transcription didn't finish. Your recording is saved, but there isn't enough space to transcribe it. Free Up Space.",
            button: "Free Up Space…",
        },
        Failure {
            name: "transcriber crashed",
            setup: |rig| with_engine(rig, Arc::new(Crashing)),
            headline: "Transcription didn't finish",
            sentence: "Transcription didn't finish. Transcription stopped partway. Your recording is saved. Try Again.",
            button: "Try Again",
        },
        Failure {
            name: "audio unreadable",
            setup: |rig| {
                with_engine(rig, Arc::new(FakeTranscriber::new()));
                *rig.takes.clip_error.lock().unwrap() =
                    Some(|| TakeError::Decode("the file is cut short".into()));
            },
            headline: "Can't read the audio",
            sentence: "Can't read the audio. This recording's audio can't be read. Show in Finder.",
            button: "Show in Finder",
        },
    ];
    for failure in failures {
        let mut rig = Rig::new();
        (failure.setup)(&mut rig);
        rig.capture.push_script(seconds(10));
        let (window, app) = rig.open(cx, limits());
        let id = record(window, &app, 10, cx);
        stop_into_grace(window, &app, cx);
        end_grace(&rig, &app, cx);
        let name = failure.name;
        until(cx, name, |cx| {
            // Retries wait out a backoff on the manual clock.
            rig.clock.advance_ms(60_000);
            label(window, "card-fix", cx).is_some()
        });
        assert_eq!(
            label(window, "card-headline", cx).as_deref(),
            Some(failure.headline),
            "{name}"
        );
        assert_eq!(
            label(window, "card-status", cx).as_deref(),
            Some(failure.sentence),
            "{name}"
        );
        assert_eq!(
            label(window, "card-fix", cx).as_deref(),
            Some(failure.button),
            "{name}"
        );
        assert!(focused(window, "card-fix", cx), "{name}: the fix has focus");
        assert!(
            row_word(window, &id, cx).ends_with(", Needs attention"),
            "{name}"
        );
    }
}

/// A failure takes focus for its fix button, but never out of a text field someone is typing in.
#[gpui_kit::test]
fn a_failure_does_not_pull_focus_out_of_the_title_being_edited(cx: &mut TestAppContext) {
    let mut rig = Rig::new();
    with_engine(&mut rig, Arc::new(FakeTranscriber::new()));
    *rig.takes.clip_error.lock().unwrap() = Some(|| TakeError::DiskFull);
    rig.capture.push_script(seconds(10));
    let (window, app) = rig.open(cx, limits());
    record(window, &app, 10, cx);
    stop_into_grace(window, &app, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("title", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        label(window, "title-editor", cx).is_some(),
        "editing the title"
    );
    end_grace(&rig, &app, cx);
    until(cx, "the failure", |cx| {
        rig.clock.advance_ms(60_000);
        label(window, "card-fix", cx).is_some()
    });
    assert!(
        label(window, "title-editor", cx).is_some(),
        "the title is still being edited"
    );
    assert!(
        !focused(window, "card-fix", cx),
        "the fix did not take focus"
    );
}
