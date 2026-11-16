//! P5's record flow through the window: a real `Dory` over the sample store, the core's scripted
//! `FakeCapture`, and a `ManualClock` that alone ends the undo grace. The recorder and pipeline
//! run on their own threads, so each wait polls the window until the state arrives.

mod common;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Options, Ports, RecorderStatus};
use dory_core::domain::{
    Blocker, Channel, FailureClass, Goal, Job, JobError, JobEvent, JobState, Ms, Priority,
    RecordingId, Stage, Timestamp,
};
use dory_core::models::{Availability, DiarizationSupport, KindTag, Llm, ModelId, Stt};
use dory_core::ports::{Change, ModelProvider, Store as _, StoreRead as _};
use dory_core::recorder::Limits;
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeSttProvider, FakeTranscriber, FakeTrash,
    ManualClock, MemTakes, Step, llm_entry, stt_entry,
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
        let dory = Dory::open(
            Ports {
                store: self.store.clone(),
                takes: Arc::new(MemTakes::in_dir(&dir)),
                trash: Arc::new(FakeTrash::new(&dir)),
                capture: self.capture.clone(),
                playback: Arc::new(FakePlayback::new()),
                clock: self.clock.clone(),
                stt: self.stt.clone(),
                llm: self.llm.clone(),
                diar: Vec::new(),
                machine: Arc::new(dory_core::testing::FakeMachine::default()),
                guard: None,
            },
            Options {
                limits,
                clock_poll: Duration::from_millis(2),
                level_every: Duration::from_millis(10),
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
        label(window, "detail-state", cx).as_deref(),
        Some("Saving the recording"),
        "the processing view shows from the moment Stop is pressed"
    );
    assert_eq!(
        label(window, "detail-step-0", cx).as_deref(),
        Some("Recorded, done")
    );
    assert_eq!(
        label(window, "detail-step-1", cx).as_deref(),
        Some("Transcript, under way")
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
        label(window, "detail-state", cx).as_deref(),
        Some("Transcription failed: The upload broke.")
    );
    assert_eq!(
        label(window, "detail-fix", cx).as_deref(),
        Some("Try again")
    );

    click(window, "detail-fix", cx);
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
        label(window, "detail-state", cx).as_deref() == Some("No speech model is set up.")
    });
    assert_eq!(
        label(window, "detail-fix", cx).as_deref(),
        Some("Try again")
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
            == Some("fake/llm is not on this Mac (4000 MB).")
    });
    assert_eq!(
        label(window, "detail-fix", cx).as_deref(),
        Some("Set up summaries")
    );
    click(window, "detail-fix", cx);
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
    stt.set_availability(&model, Availability::NeedsProvision { bytes: Some(100) });
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
        label(window, "detail-fix", cx).as_deref() == Some("Download fake/stt")
    });
    assert_eq!(stt.provisions(), 0, "nothing downloads by itself");

    click(window, "detail-fix", cx);
    until(cx, "the download", |_| stt.provisions() == 1);
    until(cx, "the job to run once the model is here", |_| {
        rig.jobs(&id)[0].state == JobState::Succeeded
    });
    until(cx, "the transcript in place of the button", |cx| {
        label(window, "detail-fix", cx).is_none() && label(window, "transcript-tab", cx).is_some()
    });
}

/// A recording is on screen before its take is saved, so the player first knows no length.
/// Once the take lands, the player shows its length, and the skip buttons can move.
#[gpui_kit::test]
fn the_player_learns_the_length_once_the_take_is_saved(cx: &mut TestAppContext) {
    let rig = Rig::new();
    rig.capture.push_script(seconds(30));
    let (window, app) = rig.open(cx, limits());
    record(window, &app, 30, cx);
    stop_into_grace(window, &app, cx);
    end_grace(&rig, &app, cx);
    until(cx, "the player to show the take's length", |cx| {
        label(window, "player-time", cx).is_some_and(|t| t.ends_with("/ 0:03"))
    });
}
