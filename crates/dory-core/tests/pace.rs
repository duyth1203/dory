//! The processing pace (UX-30): the pure policy over every setting, condition, and recorder
//! state, the 2-minute hold, and the worker applying it before each window and pass.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dory_core::app::{
    Budget, Dory, Event, HOLD_MS, LIGHT_THREADS, LightReason, Options, Pacer, Ports, StartOptions,
};
use dory_core::domain::{
    ChannelLayout, Fraction, Goal, JobState, LanguagePref, Ms, Processing, Recording, RecordingId,
    Settings, Stage, TimeRange, Timestamp,
};
use dory_core::models::DiarizationSupport;
use dory_core::ports::{
    CancelToken, Change, Clock, Conditions, EngineError, Pace, Power, RawSegment, SegmentSink,
    Store, StoreRead, Takes, Thermal, TranscribeOutcome, TranscribeRequest, Transcriber,
};
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeLanguageModel, FakeLlmProvider, FakeMachine, FakePlayback, FakeSttProvider,
    FakeTrash, ManualClock, MemTakes, eventually, llm_entry, stt_entry,
};

const FAST: u8 = 4;

fn conditions(power: Power, low_power: bool, thermal: Thermal) -> Conditions {
    Conditions {
        power,
        low_power,
        thermal,
    }
}

fn cool_on_ac() -> Conditions {
    conditions(Power::Ac, false, Thermal::Nominal)
}

fn on_battery() -> Conditions {
    conditions(Power::Battery, false, Thermal::Nominal)
}

fn light(reason: LightReason) -> Budget {
    Budget::Light { reason }
}

fn fast() -> Budget {
    Budget::Fast { threads: FAST }
}

#[test]
fn every_setting_condition_and_recorder_row_gets_its_budget() {
    use LightReason::*;
    let rows: [(&str, Conditions, Option<LightReason>); 10] = [
        ("cool on power", cool_on_ac(), None),
        (
            "power unknown",
            conditions(Power::Unknown, false, Thermal::Nominal),
            None,
        ),
        ("warm", conditions(Power::Ac, false, Thermal::Fair), None),
        ("battery", on_battery(), Some(Battery)),
        (
            "low power on power",
            conditions(Power::Ac, true, Thermal::Nominal),
            Some(LowPower),
        ),
        (
            "serious",
            conditions(Power::Ac, false, Thermal::Serious),
            Some(Hot),
        ),
        (
            "critical",
            conditions(Power::Unknown, false, Thermal::Critical),
            Some(Hot),
        ),
        (
            "battery wins over the rest",
            conditions(Power::Battery, true, Thermal::Critical),
            Some(Battery),
        ),
        (
            "low power wins over heat",
            conditions(Power::Ac, true, Thermal::Serious),
            Some(LowPower),
        ),
        (
            "heat on battery is still battery",
            conditions(Power::Battery, false, Thermal::Serious),
            Some(Battery),
        ),
    ];
    for (name, now, condition) in rows {
        for recording in [false, true] {
            let auto = match (condition, recording) {
                (Some(reason), _) => light(reason),
                (None, true) => light(Recording),
                (None, false) => fast(),
            };
            for (setting, want) in [
                (Processing::Auto, auto),
                (Processing::Fast, fast()),
                (Processing::Light, light(Chosen)),
            ] {
                let got = Pacer::new(FAST).next(setting, now, recording, 0);
                assert_eq!(got, want, "{setting:?}, {name}, recording {recording}");
            }
        }
    }
}

#[test]
fn light_uses_two_threads_and_fast_keeps_its_count() {
    assert_eq!(light(LightReason::Battery).threads(), LIGHT_THREADS);
    assert_eq!(LIGHT_THREADS, 2);
    assert_eq!(light(LightReason::Chosen).pace(), Pace::Light);
    assert_eq!(fast().threads(), FAST);
    assert_eq!(fast().pace(), Pace::Fast);
    assert_eq!(fast().reason(), None);
    assert_eq!(
        Pacer::new(1)
            .next(Processing::Fast, cool_on_ac(), false, 0)
            .threads(),
        1,
        "a single-core machine keeps its one thread"
    );
}

#[test]
fn automatic_goes_light_at_once_and_back_to_fast_two_minutes_after_the_last_condition() {
    let clock = ManualClock::new(Timestamp(0));
    let mut pacer = Pacer::new(FAST);
    let mut at = |now: Conditions| pacer.next(Processing::Auto, now, false, clock.monotonic_ms());
    assert_eq!(at(cool_on_ac()), fast());
    clock.advance_ms(10_000);
    assert_eq!(at(on_battery()), light(LightReason::Battery), "at once");

    clock.advance_ms(30_000);
    assert_eq!(
        at(cool_on_ac()),
        light(LightReason::Battery),
        "plugged in, held"
    );
    clock.advance_ms(30_000);
    assert_eq!(at(on_battery()), light(LightReason::Battery), "unplugged");
    clock.advance_ms(1_000);
    assert_eq!(at(cool_on_ac()), light(LightReason::Battery), "replugged");

    clock.advance_ms(HOLD_MS - 1_001);
    assert_eq!(
        at(cool_on_ac()),
        light(LightReason::Battery),
        "1 ms short of 2 minutes after the last unplug"
    );
    clock.advance_ms(1);
    assert_eq!(at(cool_on_ac()), fast(), "2 minutes after the last unplug");
    clock.advance_ms(1_000);
    assert_eq!(at(cool_on_ac()), fast(), "and it stays fast");
}

#[test]
fn the_hold_keeps_the_last_reason_and_a_new_condition_replaces_it() {
    let mut pacer = Pacer::new(FAST);
    let hot = conditions(Power::Ac, false, Thermal::Serious);
    assert_eq!(
        pacer.next(Processing::Auto, hot, false, 0),
        light(LightReason::Hot)
    );
    assert_eq!(
        pacer.next(Processing::Auto, cool_on_ac(), true, 1_000),
        light(LightReason::Recording)
    );
    assert_eq!(
        pacer.next(Processing::Auto, cool_on_ac(), false, 2_000),
        light(LightReason::Recording),
        "held after Stop"
    );
    assert_eq!(
        pacer.next(Processing::Auto, cool_on_ac(), false, 1_000 + HOLD_MS),
        fast(),
        "2 minutes after Stop"
    );
}

#[test]
fn fast_and_light_settings_ignore_the_hold() {
    let mut pacer = Pacer::new(FAST);
    pacer.next(Processing::Auto, on_battery(), false, 0);
    assert_eq!(pacer.next(Processing::Fast, on_battery(), true, 1), fast());
    assert_eq!(
        pacer.next(Processing::Light, cool_on_ac(), false, HOLD_MS * 10),
        light(LightReason::Chosen)
    );
}

/// What one engine call saw: the threads it was given.
#[derive(Clone, Debug)]
struct Seen {
    threads: u8,
}

/// A speech engine that records what each call saw, and unplugs the Mac during one call.
struct Watching {
    machine: Arc<FakeMachine>,
    unplug_during: Option<usize>,
    seen: Mutex<Vec<Seen>>,
}

impl Transcriber for Watching {
    fn transcribe(
        &self,
        req: &TranscribeRequest<'_>,
        sink: &mut dyn SegmentSink,
        _: &CancelToken,
    ) -> Result<TranscribeOutcome, EngineError> {
        let mut seen = self.seen.lock().unwrap();
        seen.push(Seen {
            threads: req.threads,
        });
        if self.unplug_during == Some(seen.len() - 1) {
            self.machine.set(on_battery());
        }
        sink.segment(RawSegment {
            range: TimeRange::new(Ms(0), Ms(1_000)).unwrap(),
            text: "We ship on Friday.".into(),
            speaker: None,
            language: None,
        });
        sink.progress(Fraction::new(1.0));
        Ok(TranscribeOutcome::default())
    }
}

struct Rig {
    store: Arc<MemStore>,
    takes: MemTakes,
    machine: Arc<FakeMachine>,
    engine: Arc<Watching>,
    llm: Arc<FakeLanguageModel>,
    capture: Arc<FakeCapture>,
}

impl Rig {
    fn new(now: Conditions, unplug_during: Option<usize>) -> Rig {
        let machine = Arc::new(FakeMachine::new(now));
        Rig {
            store: Arc::new(MemStore::new()),
            takes: MemTakes::new(),
            engine: Arc::new(Watching {
                machine: machine.clone(),
                unplug_during,
                seen: Mutex::new(Vec::new()),
            }),
            machine,
            llm: Arc::new(FakeLanguageModel::new("## Key points\n- Ship it.")),
            capture: Arc::new(FakeCapture::new()),
        }
    }

    fn open(&self, processing: Option<Processing>) -> Dory {
        let mut stt = stt_entry("fake/stt", DiarizationSupport::None);
        stt.caps.window = Some(Ms(30_000));
        let stt = Arc::new(FakeSttProvider::new(
            "fake",
            vec![stt],
            self.engine.clone() as Arc<dyn Transcriber>,
        ));
        let llm = Arc::new(FakeLlmProvider::new(
            "fake",
            vec![llm_entry("fake/llm")],
            self.llm.clone(),
        ));
        let ports = Ports {
            store: self.store.clone(),
            takes: Arc::new(self.takes.clone()),
            trash: Arc::new(FakeTrash::new(std::env::temp_dir())),
            capture: self.capture.clone(),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(1_763_300_000_000))),
            machine: self.machine.clone(),
            stt: vec![stt as _],
            llm: vec![llm],
            diar: Vec::new(),
            guard: None,
        };
        let options = Options {
            clock_poll: Duration::from_millis(2),
            event_capacity: 100_000,
            processing,
            ..Options::default()
        };
        Dory::open(ports, options).expect("opens")
    }

    /// A sealed mic-only recording of `seconds`, cut into 30-second windows.
    fn seed(&self, seconds: u64) -> RecordingId {
        let id = RecordingId::parse("paced").unwrap();
        let layout = ChannelLayout::MicOnly;
        let mut writer = self.takes.begin(&id, layout, 1_000).unwrap();
        writer.write(&vec![0.1; seconds as usize * 1_000]).unwrap();
        writer.finish().unwrap();
        let take = self.takes.seal(&id).unwrap();
        self.store
            .apply(vec![Change::RecordingCreated(Recording {
                id: id.clone(),
                title: None,
                created_at: Timestamp(1_000),
                language: LanguagePref::Auto,
                layout,
                take: Some(take),
            })])
            .unwrap();
        id
    }

    fn seen(&self) -> Vec<Seen> {
        self.engine.seen.lock().unwrap().clone()
    }
}

fn fast_count() -> u8 {
    std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as u8
}

fn finish(dory: &Dory, id: &RecordingId, goal: Goal) {
    dory.pipeline().enqueue(id, goal).unwrap();
    eventually("the job to succeed", || {
        dory.library().jobs(id).unwrap().pop().map(|j| j.state) == Some(JobState::Succeeded)
    });
}

fn budgets(events: &dory_core::app::Subscription, of: Stage) -> Vec<Option<Budget>> {
    std::iter::from_fn(|| events.try_recv())
        .filter_map(|e| match e {
            Event::StageProgress { stage, budget, .. } if stage == of => Some(budget),
            _ => None,
        })
        .collect()
}

#[test]
fn a_pace_change_mid_transcribe_changes_threads_once_from_the_next_window() {
    let rig = Rig::new(cool_on_ac(), Some(1));
    let id = rig.seed(120);
    let dory = rig.open(None);
    let events = dory.events();
    finish(&dory, &id, Goal::TRANSCRIBE);

    let threads: Vec<u8> = rig.seen().iter().map(|s| s.threads).collect();
    let fast = fast_count();
    assert_eq!(threads, [fast, fast, 2, 2], "unplugged during window 2");
    let rebuilds = threads.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(
        rebuilds,
        usize::from(fast != 2),
        "one change, so one rebuild"
    );
    assert!(
        budgets(&events, Stage::Transcribe).contains(&Some(light(LightReason::Battery))),
        "progress says why it runs light"
    );
}

#[test]
fn threads_reach_the_speech_requests_and_the_summary_reports_no_budget() {
    for (processing, want) in [
        (Processing::Fast, fast_count()),
        (Processing::Light, LIGHT_THREADS),
    ] {
        let rig = Rig::new(on_battery(), None);
        let id = rig.seed(60);
        let dory = rig.open(Some(processing));
        let events = dory.events();
        finish(&dory, &id, Goal::ALL);

        let speech: Vec<u8> = rig.seen().iter().map(|s| s.threads).collect();
        assert_eq!(speech, [want, want], "{processing:?}");
        assert!(!rig.llm.prompts().is_empty());
        let shown = budgets(&events, Stage::Summarize);
        assert!(
            !shown.is_empty() && shown.iter().all(Option::is_none),
            "{processing:?}: the summary runs unlimited, {shown:?}"
        );
    }
}

#[test]
fn the_setting_is_read_for_each_job() {
    let rig = Rig::new(cool_on_ac(), None);
    let id = rig.seed(30);
    let mut settings: Settings = rig.store.settings().unwrap();
    settings.processing = Processing::Light;
    rig.store
        .apply(vec![Change::SettingsReplaced(settings)])
        .unwrap();
    let dory = rig.open(None);
    finish(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(
        rig.seen().iter().map(|s| s.threads).collect::<Vec<_>>(),
        [LIGHT_THREADS]
    );
}

#[test]
fn automatic_runs_light_while_the_recorder_is_busy() {
    let rig = Rig::new(cool_on_ac(), None);
    let id = rig.seed(30);
    let dory = rig.open(None);
    dory.recorder().start(StartOptions::default()).unwrap();
    eventually("the recorder to start", || {
        !matches!(
            dory.recorder().status(),
            dory_core::app::RecorderStatus::Idle
        )
    });
    let events = dory.events();
    finish(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(
        rig.seen().iter().map(|s| s.threads).collect::<Vec<_>>(),
        [LIGHT_THREADS]
    );
    assert!(budgets(&events, Stage::Transcribe).contains(&Some(light(LightReason::Recording))));
}
