//! The transcribe and summarize policy, driven through the `Dory` facade with fakes: windows,
//! silence skipping, checkpoints across a crash, speaker numbering, merge and bleed, map-reduce
//! summaries, and blocked models.

use std::sync::Arc;
use std::time::Duration;

use dory_core::app::{Dory, DoryError, Event, Options, Ports, Rerun};
use dory_core::domain::{
    Blocker, Boost, Channel, ChannelLayout, FailureCause, FailureClass, Goal, JobEvent, JobState,
    LangSet, LangTag, LanguagePref, Ms, Recording, RecordingId, Recovery, Revision, Segment,
    Settings, SpeakerId, SpeakerNames, Stage, TimeRange, Timestamp, TranslationFlag,
    VocabularyTerm,
};
use dory_core::models::{
    Availability, Choice, DiarizationSupport, KindTag, LangSupport, Llm, Mismatch, ModelEntry,
    ModelId, Stt, Unavailable, UnavailableReason,
};
use dory_core::ports::{Change, CheckpointKey, Store, StoreRead, Takes, TranslationKey};
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeDiarProvider, FakeDiarizer, FakeLanguageModel, FakeLlmProvider,
    FakeSttProvider, FakeTranscriber, ManualClock, MemTakes, Refusal, TranscribeCall, diar_entry,
    eventually, llm_entry, stt_entry,
};

const RATE: u32 = 1_000;
const STT: &str = "fake/stt";
const LLM: &str = "fake/llm";
const SUMMARY: &str = "## Key points\n- Ship it Friday.";
const WINDOW: Option<Ms> = Some(Ms(30_000));

fn stt(id: &str, diarization: DiarizationSupport, window: Option<Ms>) -> ModelEntry<Stt> {
    let mut entry = stt_entry(id, diarization);
    entry.caps.window = window;
    entry
}

fn llm(context: u32) -> ModelEntry<Llm> {
    let mut entry = llm_entry(LLM);
    entry.caps.context_tokens = context;
    entry
}

struct Rig {
    store: Arc<MemStore>,
    takes: MemTakes,
    clock: Arc<ManualClock>,
    transcriber: Arc<FakeTranscriber>,
    llm: Arc<FakeLanguageModel>,
    stt: Arc<FakeSttProvider>,
    llm_provider: Arc<FakeLlmProvider>,
    /// Summary providers after the first, each with its own engine.
    more_llm: Vec<Arc<FakeLlmProvider>>,
    diarizer: Option<(Arc<FakeDiarizer>, Arc<FakeDiarProvider>)>,
}

impl Rig {
    fn new(transcriber: FakeTranscriber, stt: Vec<ModelEntry<Stt>>, llm: ModelEntry<Llm>) -> Rig {
        Self::with_reply(transcriber, stt, llm, SUMMARY)
    }

    fn with_reply(
        transcriber: FakeTranscriber,
        stt: Vec<ModelEntry<Stt>>,
        llm: ModelEntry<Llm>,
        reply: &str,
    ) -> Rig {
        Self::with_model(transcriber, stt, llm, FakeLanguageModel::new(reply))
    }

    fn with_model(
        transcriber: FakeTranscriber,
        stt: Vec<ModelEntry<Stt>>,
        llm: ModelEntry<Llm>,
        model: FakeLanguageModel,
    ) -> Rig {
        let transcriber = Arc::new(transcriber);
        let model = Arc::new(model);
        Rig {
            store: Arc::new(MemStore::new()),
            takes: MemTakes::new(),
            clock: Arc::new(ManualClock::new(Timestamp(1_763_300_000_000))),
            stt: Arc::new(FakeSttProvider::new("fake", stt, transcriber.clone())),
            llm_provider: Arc::new(FakeLlmProvider::new("fake", vec![llm], model.clone())),
            transcriber,
            llm: model,
            more_llm: Vec::new(),
            diarizer: None,
        }
    }

    /// Registers another summary model with its own engine.
    fn with_summary_model(mut self, entry: ModelEntry<Llm>, model: Arc<FakeLanguageModel>) -> Rig {
        self.more_llm
            .push(Arc::new(FakeLlmProvider::new("fake", vec![entry], model)));
        self
    }

    fn with_diarizer(mut self, diarizer: FakeDiarizer) -> Rig {
        let diarizer = Arc::new(diarizer);
        let provider = Arc::new(FakeDiarProvider::new(
            "fake",
            vec![diar_entry("fake/diar")],
            diarizer.clone(),
        ));
        self.diarizer = Some((diarizer, provider));
        self
    }

    fn open(&self) -> Dory {
        let ports = Ports {
            store: self.store.clone(),
            takes: Arc::new(self.takes.clone()),
            trash: Arc::new(dory_core::testing::FakeTrash::new(std::env::temp_dir())),
            capture: Arc::new(FakeCapture::new()),
            clock: self.clock.clone(),
            stt: vec![self.stt.clone()],
            llm: std::iter::once(&self.llm_provider)
                .chain(&self.more_llm)
                .map(|provider| provider.clone() as _)
                .collect(),
            diar: self
                .diarizer
                .iter()
                .map(|(_, provider)| provider.clone() as _)
                .collect(),
            playback: Arc::new(dory_core::testing::FakePlayback::new()),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        };
        let options = Options {
            clock_poll: Duration::from_millis(2),
            event_capacity: 100_000,
            retry_base_ms: 0,
            ..Options::default()
        };
        Dory::open(ports, options).expect("opens")
    }

    /// A sealed recording of `seconds` of audio.
    fn seed(&self, name: &str, layout: ChannelLayout, seconds: u64) -> RecordingId {
        let id = RecordingId::parse(name).unwrap();
        let mut writer = self.takes.begin(&id, layout, RATE).unwrap();
        let samples = seconds as usize * RATE as usize * layout.channel_count();
        writer.write(&vec![0.0; samples]).unwrap();
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

    fn key(&self, id: &RecordingId, model: &str, channel: Channel) -> CheckpointKey {
        CheckpointKey {
            recording: id.clone(),
            model: dory_core::models::ModelRef::parse(model).unwrap(),
            channel,
            language: LanguagePref::Auto,
        }
    }
}

fn r(a: u64, b: u64) -> TimeRange {
    TimeRange::new(Ms(a), Ms(b)).unwrap()
}

fn job_state(dory: &Dory, id: &RecordingId) -> Option<JobState> {
    dory.library().jobs(id).unwrap().pop().map(|j| j.state)
}

fn wait_for(dory: &Dory, id: &RecordingId, state: JobState) {
    eventually(&format!("job of {id} to be {state:?}"), || {
        job_state(dory, id).as_ref() == Some(&state)
    });
}

fn process(dory: &Dory, id: &RecordingId, goal: Goal) {
    dory.pipeline().enqueue(id, goal).unwrap();
    wait_for(dory, id, JobState::Succeeded);
}

fn spans(calls: &[TranscribeCall]) -> Vec<(Channel, TimeRange)> {
    calls.iter().map(|c| (c.channel, c.span)).collect()
}

fn texts(segments: &[Segment]) -> Vec<&str> {
    segments.iter().map(|s| s.text.as_str()).collect()
}

fn drain(events: &dory_core::app::Subscription) -> Vec<Event> {
    std::iter::from_fn(|| events.try_recv()).collect()
}

fn progress(events: &[Event], of: Stage) -> Vec<f32> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::StageProgress {
                stage, fraction, ..
            } if *stage == of => Some(fraction.get()),
            _ => None,
        })
        .collect()
}

fn assert_climbs_to_done(fractions: &[f32]) {
    assert!(
        fractions.windows(2).all(|w| w[0] <= w[1]),
        "progress went back: {fractions:?}"
    );
    assert_eq!(fractions.last(), Some(&1.0), "{fractions:?}");
}

#[test]
fn windows_are_cut_at_silence_and_only_speech_is_sent() {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(2_500, 5_000, 0, "I have one question.")])
            .with_segments(
                Channel::System,
                &[
                    (11_000, 14_000, 0, "Welcome everyone."),
                    (23_000, 30_000, 0, "Here is the plan."),
                    (61_000, 65_000, 0, "Any questions?"),
                ],
            ),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("windows", ChannelLayout::MicSystem, 120);
    rig.takes.set_activity(&id, Channel::Mic, &[(2_000, 6_000)]);
    rig.takes.set_activity(
        &id,
        Channel::System,
        &[(10_000, 20_000), (22_000, 35_000), (60_000, 70_000)],
    );
    let dory = rig.open();
    let events = dory.events();
    process(&dory, &id, Goal::TRANSCRIBE);

    assert_eq!(
        spans(&rig.transcriber.calls()),
        vec![
            (Channel::Mic, r(2_000, 6_000)),
            (Channel::System, r(10_000, 35_000)),
            (Channel::System, r(60_000, 70_000)),
        ],
        "speech runs join up to the 30 s window, and silence is never sent"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        transcript
            .segments
            .iter()
            .map(|s| (s.range, s.channel))
            .collect::<Vec<_>>(),
        vec![
            (Some(r(2_500, 5_000)), Channel::Mic),
            (Some(r(11_000, 14_000)), Channel::System),
            (Some(r(23_000, 30_000)), Channel::System),
            (Some(r(61_000, 65_000)), Channel::System),
        ],
        "window times are back on the take's axis"
    );

    let seen = drain(&events);
    assert!(
        seen.iter().any(|e| matches!(
            e,
            Event::DraftSegment { segment, .. }
                if segment.text == "Any questions?" && segment.range == Some(r(61_000, 65_000))
        )),
        "drafts carry take time"
    );
    assert_climbs_to_done(&progress(&seen, Stage::Transcribe));
}

/// Three windows of speech on a mic-only take, one line each, lasting `lengths` seconds.
fn windows_heard(lengths: [u64; 3], languages: [&str; 3]) -> (Rig, RecordingId) {
    let starts = [0, 40_000, 80_000];
    let lines: Vec<(u64, u64, u16, &str)> = starts
        .iter()
        .zip(lengths)
        .map(|(start, secs)| (*start, start + secs * 1_000, 0, "line"))
        .collect();
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &lines)
            .detecting_each(&languages),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("heard", ChannelLayout::MicOnly, 100);
    let spans: Vec<(u64, u64)> = lines.iter().map(|l| (l.0, l.1)).collect();
    rig.takes.set_activity(&id, Channel::Mic, &spans);
    (rig, id)
}

fn tags(tags: &[&str]) -> Vec<dory_core::domain::LangTag> {
    tags.iter()
        .map(|t| dory_core::domain::LangTag::parse(t).unwrap())
        .collect()
}

#[test]
fn auto_transcript_records_the_heard_languages() {
    let (rig, id) = windows_heard([20, 20, 20], ["tl", "en", "tl"]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(transcript.detected_languages, tags(&["tl", "en"]));
}

#[test]
fn the_language_heard_longest_ranks_first_not_the_one_heard_in_most_windows() {
    let (rig, id) = windows_heard([5, 5, 25], ["en", "en", "tl"]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        transcript.detected_languages,
        tags(&["tl", "en"]),
        "25 s of tl outweighs two 5 s windows of en"
    );
}

#[test]
fn a_heard_language_outside_the_meeting_languages_is_kept() {
    let (rig, id) = windows_heard([20, 5, 5], ["tl", "es", "tl"]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(transcript.detected_languages, tags(&["tl", "es"]));
}

fn nothing_recognized() -> Blocker {
    Blocker::Incompatible {
        model: STT.parse::<ModelId<Stt>>().unwrap().erase(),
        why: Mismatch::NothingRecognized,
    }
}

#[test]
fn speech_with_no_words_blocks_instead_of_saving_empty() {
    let rig = Rig::new(
        FakeTranscriber::new(),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("heard-nothing", ChannelLayout::MicOnly, 30);
    rig.takes.set_activity(&id, Channel::Mic, &[(0, 30_000)]);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: nothing_recognized(),
        },
    );
    assert_eq!(rig.transcriber.calls().len(), 1);
    assert_eq!(
        dory.library().transcript(&id).unwrap(),
        None,
        "no empty transcript is saved"
    );
    assert!(rig.llm.prompts().is_empty());
}

#[test]
fn under_ten_seconds_of_speech_with_no_words_is_still_a_quiet_take() {
    let rig = Rig::new(
        FakeTranscriber::new(),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("breath", ChannelLayout::MicOnly, 30);
    rig.takes.set_activity(&id, Channel::Mic, &[(0, 9_000)]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert!(transcript.segments.is_empty());
}

#[test]
fn words_on_the_other_channel_keep_a_silent_channel_from_blocking() {
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::System, &[(1_000, 4_000, 0, "hello there")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("one-sided", ChannelLayout::MicSystem, 30);
    rig.takes.set_activity(&id, Channel::Mic, &[(0, 20_000)]);
    rig.takes.set_activity(&id, Channel::System, &[(0, 20_000)]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&transcript.segments), ["hello there"]);
}

#[test]
fn a_silent_channel_costs_no_engine_call() {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(1_000, 2_000, 0, "hallucinated thanks")])
            .with_segments(Channel::System, &[(5_000, 9_000, 0, "Remote talk.")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("muted-mic", ChannelLayout::MicSystem, 60);
    rig.takes.set_activity(&id, Channel::Mic, &[]);
    rig.takes
        .set_activity(&id, Channel::System, &[(4_000, 10_000)]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);

    assert_eq!(
        spans(&rig.transcriber.calls()),
        vec![(Channel::System, r(4_000, 10_000))]
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&transcript.segments), ["Remote talk."]);
}

#[test]
fn a_clip_with_unmeasured_activity_is_all_speech() {
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(1_000, 2_000, 0, "hello")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("unmeasured", ChannelLayout::MicOnly, 50);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(
        spans(&rig.transcriber.calls()),
        vec![
            (Channel::Mic, r(0, 25_000)),
            (Channel::Mic, r(25_000, 50_000))
        ]
    );
}

#[test]
fn native_diarization_sends_the_whole_diarized_clip_in_one_call() {
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(1_000, 2_000, 0, "hello")]),
        vec![stt(STT, DiarizationSupport::Native, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("native", ChannelLayout::MicSystem, 90);
    rig.takes
        .set_activity(&id, Channel::Mic, &[(0, 10_000), (50_000, 60_000)]);
    rig.takes
        .set_activity(&id, Channel::System, &[(0, 10_000), (50_000, 60_000)]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let calls = rig.transcriber.calls();
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.channel, c.span, c.diarize))
            .collect::<Vec<_>>(),
        vec![
            (Channel::Mic, r(0, 10_000), false),
            (Channel::Mic, r(50_000, 60_000), false),
            (Channel::System, r(0, 90_000), true),
        ],
        "the mic is one known speaker, so it still gets windows"
    );
}

/// Four windows of speech on a mic-only take, one line each.
fn four_windows(stt_entries: Vec<ModelEntry<Stt>>) -> (Rig, RecordingId) {
    four_windows_on(FakeTranscriber::new(), stt_entries)
}

fn four_windows_on(
    transcriber: FakeTranscriber,
    stt_entries: Vec<ModelEntry<Stt>>,
) -> (Rig, RecordingId) {
    let rig = Rig::new(
        transcriber.with_segments(
            Channel::Mic,
            &[
                (1_000, 5_000, 0, "first"),
                (41_000, 45_000, 0, "second"),
                (81_000, 85_000, 0, "third"),
                (121_000, 125_000, 0, "fourth"),
            ],
        ),
        stt_entries,
        llm(8_192),
    );
    let id = rig.seed("long-meeting", ChannelLayout::MicOnly, 150);
    rig.takes.set_activity(
        &id,
        Channel::Mic,
        &[
            (0, 20_000),
            (40_000, 60_000),
            (80_000, 100_000),
            (120_000, 140_000),
        ],
    );
    (rig, id)
}

/// Starts the job, lets two windows finish, and drops `Dory` while the third runs.
fn crash_during_the_third_window(rig: &Rig, id: &RecordingId) {
    rig.transcriber.hold_from(2);
    let dory = rig.open();
    dory.pipeline().enqueue(id, Goal::TRANSCRIBE).unwrap();
    eventually("the third window to run", || {
        rig.transcriber.calls().len() == 3 && rig.transcriber.is_running()
    });
    drop(dory);
    rig.transcriber.hold_from(usize::MAX);
}

#[test]
fn a_rerun_after_a_crash_resumes_at_the_first_unfinished_window() {
    let (rig, id) = four_windows(vec![stt(STT, DiarizationSupport::None, WINDOW)]);
    crash_during_the_third_window(&rig, &id);
    let key = rig.key(&id, STT, Channel::Mic);
    assert_eq!(
        rig.store
            .checkpoints(&key)
            .unwrap()
            .iter()
            .map(|c| c.span)
            .collect::<Vec<_>>(),
        vec![r(0, 20_000), r(40_000, 60_000)],
        "the two finished windows are saved"
    );
    // A crash leaves the job Running, which startup sends back to the queue.
    let job = rig.store.jobs_for(&id).unwrap().remove(0);
    rig.store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::Transcribe,
            },
        }])
        .unwrap();

    let dory = rig.open();
    wait_for(&dory, &id, JobState::Succeeded);
    assert_eq!(
        spans(&rig.transcriber.calls()[3..]),
        vec![
            (Channel::Mic, r(80_000, 100_000)),
            (Channel::Mic, r(120_000, 140_000)),
        ],
        "only the unfinished windows run again"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        texts(&transcript.segments),
        ["first", "second", "third", "fourth"]
    );
    assert_eq!(
        rig.store.checkpoints(&key).unwrap(),
        vec![],
        "cleared with the transcript"
    );
}

#[test]
fn each_window_hears_the_lead_in_before_it_and_keeps_only_its_own_sentences() {
    let mut entry = stt(STT, DiarizationSupport::None, WINDOW);
    entry.caps.lead_in = Ms(40_000);
    let (rig, id) = four_windows(vec![entry]);
    crash_during_the_third_window(&rig, &id);
    assert_eq!(
        rig.store
            .checkpoints(&rig.key(&id, STT, Channel::Mic))
            .unwrap()
            .iter()
            .map(|c| (c.span, c.segments.iter().map(|s| s.text.clone()).collect()))
            .collect::<Vec<_>>(),
        vec![
            (r(0, 20_000), vec!["first".to_owned()]),
            (r(40_000, 60_000), vec!["second".to_owned()]),
        ] as Vec<(TimeRange, Vec<String>)>,
        "checkpoints cover the windows alone"
    );
    let job = rig.store.jobs_for(&id).unwrap().remove(0);
    rig.store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::Transcribe,
            },
        }])
        .unwrap();
    let dory = rig.open();
    wait_for(&dory, &id, JobState::Succeeded);
    assert_eq!(
        spans(&rig.transcriber.calls()),
        vec![
            (Channel::Mic, r(0, 20_000)),
            (Channel::Mic, r(0, 60_000)),
            (Channel::Mic, r(40_000, 100_000)),
            (Channel::Mic, r(40_000, 100_000)),
            (Channel::Mic, r(80_000, 140_000)),
        ],
        "a window starts up to the lead-in early, never before the take, and a resume does the same"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        texts(&transcript.segments),
        ["first", "second", "third", "fourth"]
    );
}

#[test]
fn a_checkpoint_from_another_model_is_never_reused() {
    let mut first = stt("fake/stt-a", DiarizationSupport::None, WINDOW);
    first.recommended = true;
    let second = stt("fake/stt-b", DiarizationSupport::None, WINDOW);
    let (rig, id) = four_windows(vec![first, second]);
    crash_during_the_third_window(&rig, &id);
    assert_eq!(
        rig.store
            .checkpoints(&rig.key(&id, "fake/stt-a", Channel::Mic))
            .unwrap()
            .len(),
        2
    );

    let b: ModelId<Stt> = "fake/stt-b".parse().unwrap();
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            stt: Choice::Pinned(b.clone()),
            ..Settings::default()
        })])
        .unwrap();
    let dory = rig.open();
    wait_for(&dory, &id, JobState::Succeeded);
    assert_eq!(
        spans(&rig.transcriber.calls()[3..]),
        vec![
            (Channel::Mic, r(0, 20_000)),
            (Channel::Mic, r(40_000, 60_000)),
            (Channel::Mic, r(80_000, 100_000)),
            (Channel::Mic, r(120_000, 140_000)),
        ],
        "the new model starts over"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(transcript.produced_by, b.erase());
    assert_eq!(
        rig.store
            .checkpoints(&rig.key(&id, "fake/stt-a", Channel::Mic))
            .unwrap(),
        vec![],
        "the old model's checkpoints go with the new transcript"
    );
}

#[test]
fn speakers_are_numbered_by_first_appearance_across_the_whole_clip() {
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(
            Channel::System,
            &[
                (1_000, 4_000, 0, "a1"),
                (5_000, 8_000, 0, "b1"),
                (41_000, 44_000, 0, "b2"),
                (45_000, 48_000, 0, "a2"),
            ],
        ),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    )
    .with_diarizer(FakeDiarizer::new(&[
        (0, 4_500, 7),
        (4_500, 9_000, 3),
        (40_000, 44_500, 3),
        (44_500, 49_000, 7),
    ]));
    let id = rig.seed("two-remote", ChannelLayout::SystemOnly, 60);
    rig.takes
        .set_activity(&id, Channel::System, &[(0, 9_000), (40_000, 49_000)]);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);

    assert_eq!(rig.transcriber.calls().len(), 2, "two windows");
    let (diarizer, _) = rig.diarizer.as_ref().unwrap();
    assert_eq!(
        diarizer.calls(),
        vec![(Channel::System, r(0, 60_000))],
        "speakers are separated over the whole clip, once"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        transcript
            .segments
            .iter()
            .map(|s| (s.text.as_str(), s.speaker))
            .collect::<Vec<_>>(),
        vec![
            ("a1", SpeakerId(1)),
            ("b1", SpeakerId(2)),
            ("b2", SpeakerId(2)),
            ("a2", SpeakerId(1)),
        ]
    );
}

#[test]
fn channels_merge_by_time_and_mic_bleed_is_dropped() {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(
                Channel::System,
                &[
                    (1_000, 4_000, 4, "Can everyone see my screen?"),
                    (9_000, 12_000, 9, "The launch moves to March."),
                ],
            )
            .with_segments(
                Channel::Mic,
                &[
                    (1_200, 4_100, 0, "can everyone see my screen"),
                    (5_000, 7_000, 0, "Yes, I can see it."),
                    (9_100, 12_000, 0, "the launch moves to march"),
                    (13_000, 14_000, 0, "Great."),
                ],
            ),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(8_192),
    );
    let id = rig.seed("no-headphones", ChannelLayout::MicSystem, 20);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        transcript
            .segments
            .iter()
            .map(|s| (s.text.as_str(), s.channel, s.speaker))
            .collect::<Vec<_>>(),
        vec![
            ("Can everyone see my screen?", Channel::System, SpeakerId(1)),
            ("Yes, I can see it.", Channel::Mic, SpeakerId::LOCAL),
            ("The launch moves to March.", Channel::System, SpeakerId(2)),
            ("Great.", Channel::Mic, SpeakerId::LOCAL),
        ]
    );
}

fn words(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

/// Sixty ten-word lines from one remote speaker, in Vietnamese.
fn long_meeting(context: u32, reply: &str) -> (Rig, RecordingId) {
    let lines: Vec<(u64, u64, u16, String)> = (0..60)
        .map(|n| {
            let text: Vec<String> = (0..10).map(|w| format!("l{n}w{w}")).collect();
            (n * 1_000, n * 1_000 + 900, 0, text.join(" "))
        })
        .collect();
    let borrowed: Vec<(u64, u64, u16, &str)> = lines
        .iter()
        .map(|(a, b, s, t)| (*a, *b, *s, t.as_str()))
        .collect();
    let rig = Rig::with_reply(
        FakeTranscriber::new()
            .with_segments(Channel::Mixed, &borrowed)
            .detecting("vi"),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(context),
        reply,
    );
    let id = rig.seed("long-summary", ChannelLayout::Mixed, 61);
    (rig, id)
}

#[test]
fn a_take_with_no_speech_gets_no_summary_and_never_calls_the_summary_model() {
    let rig = Rig::new(
        FakeTranscriber::new(),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let id = rig.seed("silence", ChannelLayout::MicSystem, 60);
    rig.takes.set_activity(&id, Channel::Mic, &[]);
    rig.takes.set_activity(&id, Channel::System, &[]);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);

    assert!(
        dory.library()
            .transcript(&id)
            .unwrap()
            .unwrap()
            .segments
            .is_empty()
    );
    assert!(
        rig.llm.prompts().is_empty(),
        "the summary model was asked to summarize nothing"
    );
    assert_eq!(dory.library().summary(&id).unwrap(), None);
}

#[test]
fn a_transcript_that_fits_the_context_is_summarized_in_one_pass() {
    let (rig, id) = long_meeting(8_192, SUMMARY);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    let prompts = rig.llm.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].user.lines().count(), 60);
    assert_eq!(
        dory.library().summary(&id).unwrap().unwrap().markdown,
        SUMMARY
    );
}

#[test]
fn a_transcript_too_long_for_the_context_is_summarized_by_map_reduce() {
    let context = 400;
    let (rig, id) = long_meeting(context, SUMMARY);
    let dory = rig.open();
    let events = dory.events();
    process(&dory, &id, Goal::ALL);

    let prompts = rig.llm.prompts();
    for prompt in &prompts {
        assert!(
            words(&prompt.system) + words(&prompt.user) + prompt.max_output_tokens <= context,
            "a prompt overflows the {context}-token context: {prompt:?}"
        );
        assert!(prompt.system.contains("tag vi"), "{}", prompt.system);
    }
    let (last, maps) = prompts.split_last().unwrap();
    assert!(maps.len() >= 2, "{} passes", prompts.len());
    for n in 0..60 {
        let line = format!("l{n}w0 ");
        assert_eq!(
            maps.iter().filter(|p| p.user.contains(&line)).count(),
            1,
            "line {n} is read by exactly one notes pass"
        );
    }
    assert!(!last.user.contains("l0w0"), "the last pass reads notes");
    assert_eq!(last.user.matches("Ship it Friday.").count(), maps.len());
    assert!(
        last.system
            .contains("key points, decisions, and action items")
    );
    assert_eq!(
        dory.library().summary(&id).unwrap().unwrap().markdown,
        SUMMARY
    );
    let seen = drain(&events);
    assert_climbs_to_done(&progress(&seen, Stage::Summarize));
    let drafted: String = seen
        .iter()
        .filter_map(|e| match e {
            Event::DraftSummary { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(drafted, SUMMARY, "only the last pass streams as a draft");
}

#[test]
fn notes_too_long_for_one_summary_pass_are_merged_first() {
    let context = 400;
    let long_note = vec!["note"; 45].join(" ");
    let (rig, id) = long_meeting(context, &long_note);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    let prompts = rig.llm.prompts();
    let reads_notes = |p: &&dory_core::testing::Completion| p.user.contains("note note");
    let note_passes = prompts.iter().filter(reads_notes).count();
    assert!(
        note_passes >= 2,
        "a merge pass and the summary pass both read notes: {prompts:#?}"
    );
    for prompt in &prompts {
        assert!(words(&prompt.system) + words(&prompt.user) + prompt.max_output_tokens <= context);
    }
}

/// Gemma stops a note at `max_output_tokens` of its own tokens, but the Ollama adapter counts
/// a third or more above that, so a note at its cap counts past the cap. Merged notes reach the
/// cap more often than notes on transcript, so a run that merges twice can be left with notes
/// no two of which fit one capped pass. Live, gemma4:e4b stopped a merged note at 1,024 tokens
/// that the adapter counted as 1,509, and a two-hour meeting failed this way in 3 of 9 runs.
#[test]
fn a_two_hour_meeting_whose_merged_notes_count_past_their_cap_is_still_summarized() {
    let context = 32_768;
    let lines: Vec<(u64, u64, u16, String)> = (0..1_200)
        .map(|n| {
            let text: Vec<String> = (0..15).map(|w| format!("l{n}w{w}")).collect();
            (n * 6_000, n * 6_000 + 5_000, 0, text.join(" "))
        })
        .collect();
    let borrowed: Vec<(u64, u64, u16, &str)> = lines
        .iter()
        .map(|(a, b, s, t)| (*a, *b, *s, t.as_str()))
        .collect();
    let model = FakeLanguageModel::replying(|req| {
        if req
            .system
            .contains("key points, decisions, and action items")
        {
            SUMMARY.to_owned()
        } else if req.user.contains("noted") {
            vec!["noted"; req.max_output_tokens as usize * 8 / 5].join(" ")
        } else {
            vec!["noted"; req.max_output_tokens as usize * 3 / 4].join(" ")
        }
    });
    let rig = Rig::with_model(
        FakeTranscriber::new()
            .with_segments(Channel::Mixed, &borrowed)
            .detecting("vi"),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(context),
        model,
    );
    let id = rig.seed("two-hours", ChannelLayout::Mixed, 7_201);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the job to end", || {
        matches!(
            job_state(&dory, &id),
            Some(JobState::Succeeded | JobState::Failed { .. })
        )
    });

    assert_eq!(job_state(&dory, &id), Some(JobState::Succeeded));
    assert_eq!(
        dory.library().summary(&id).unwrap().unwrap().markdown,
        SUMMARY
    );
    for prompt in &rig.llm.prompts() {
        assert!(
            words(&prompt.system) + words(&prompt.user) + prompt.max_output_tokens <= context,
            "a prompt overflows the {context}-token context"
        );
    }
}

#[test]
fn a_missing_speech_model_blocks_transcription_without_running_anything() {
    let rig = Rig::new(
        FakeTranscriber::new(),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let model: ModelId<Stt> = STT.parse().unwrap();
    rig.stt
        .set_availability(&model, Availability::NeedsProvision { bytes: Some(42) });
    let id = rig.seed("no-speech-model", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NeedsProvision {
                model: model.erase(),
                bytes: Some(42),
            },
        },
    );
    assert_eq!(rig.transcriber.calls(), vec![]);
    assert_eq!(dory.library().transcript(&id).unwrap(), None);
}

#[test]
fn a_missing_summary_model_blocks_the_summary_after_the_transcript_commits() {
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(0, 1_000, 0, "hello")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let model: ModelId<Llm> = LLM.parse().unwrap();
    rig.llm_provider
        .set_availability(&model, Availability::NeedsProvision { bytes: None });
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            llm: Choice::Pinned(model.clone()),
            ..Settings::default()
        })])
        .unwrap();
    let id = rig.seed("no-summary-model", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::NeedsProvision {
                model: model.erase(),
                bytes: None,
            },
        },
    );
    assert!(dory.library().transcript(&id).unwrap().is_some());
    assert_eq!(rig.llm.prompts(), vec![]);
}

#[test]
fn audio_the_take_cannot_give_fails_the_job_as_unreadable() {
    let rig = Rig::new(
        FakeTranscriber::new(),
        vec![stt(STT, DiarizationSupport::Native, WINDOW)],
        llm(8_192),
    );
    let id = RecordingId::parse("no-system-channel").unwrap();
    let mut writer = rig.takes.begin(&id, ChannelLayout::MicOnly, RATE).unwrap();
    writer.write(&vec![0.0; 10 * RATE as usize]).unwrap();
    writer.finish().unwrap();
    let take = rig.takes.seal(&id).unwrap();
    rig.store
        .apply(vec![Change::RecordingCreated(Recording {
            id: id.clone(),
            title: None,
            created_at: Timestamp(1_000),
            language: LanguagePref::Auto,
            layout: ChannelLayout::SystemOnly,
            take: Some(take),
        })])
        .unwrap();
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::TRANSCRIBE).unwrap();
    eventually("the job to fail", || {
        matches!(job_state(&dory, &id), Some(JobState::Failed { .. }))
    });
    let Some(JobState::Failed { error, .. }) = job_state(&dory, &id) else {
        unreachable!()
    };
    assert_eq!(error.cause, FailureCause::AudioUnreadable, "{error:?}");
}

#[test]
fn an_empty_speech_catalog_blocks_on_no_model() {
    let rig = Rig::new(FakeTranscriber::new(), vec![], llm(8_192));
    let id = rig.seed("no-catalog", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NoModel { kind: KindTag::Stt },
        },
    );
}

#[test]
fn a_summary_instruction_reaches_the_final_pass_and_the_old_summary_is_kept() {
    let (rig, id) = long_meeting(8_192, SUMMARY);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    rig.store
        .apply(vec![Change::SummaryInstructionSet {
            id: id.clone(),
            instruction: Some("focus on the budget".into()),
        }])
        .unwrap();
    dory.pipeline().enqueue(&id, Goal::SUMMARIZE).unwrap();
    eventually("the second summary job to succeed", || {
        let jobs = rig.store.jobs_for(&id).unwrap();
        jobs.len() == 2 && jobs.iter().all(|j| j.state == JobState::Succeeded)
    });
    let prompts = rig.llm.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(!prompts[0].system.contains("focus on the budget"));
    assert!(
        prompts[1]
            .system
            .ends_with("within the rules above: \"focus on the budget\""),
        "{}",
        prompts[1].system
    );
    assert!(rig.store.previous_summary(&id).unwrap().is_some());
}

/// A mic-only take whose windows each hold a Tagalog turn and then an English one.
fn switching_turns(
    windows: &[u64],
    engine: FakeTranscriber,
    settings: Settings,
) -> (Rig, RecordingId) {
    let mut lines = Vec::new();
    let mut activity = Vec::new();
    for (n, start) in windows.iter().enumerate() {
        let (tl, en) = if n == 0 {
            ("isa", "one")
        } else {
            ("dalawa", "two")
        };
        lines.push((*start, start + 8_000, 0, tl));
        lines.push((start + 11_000, start + 18_000, 0, en));
        activity.push((*start, start + 8_000));
        activity.push((start + 11_000, start + 18_000));
    }
    let mut entry = stt(STT, DiarizationSupport::None, WINDOW);
    entry.caps.sentence_times = true;
    let rig = Rig::new(
        engine.with_segments(Channel::Mic, &lines),
        vec![entry],
        llm(8_192),
    );
    let id = rig.seed("switching", ChannelLayout::MicOnly, 70);
    rig.takes.set_activity(&id, Channel::Mic, &activity);
    rig.store
        .apply(vec![Change::SettingsReplaced(settings)])
        .unwrap();
    (rig, id)
}

fn two_languages() -> Settings {
    Settings {
        meeting_languages: tags(&["tl", "en"]),
        switch_recovery: Recovery::Auto,
        ..Settings::default()
    }
}

#[test]
fn a_dropped_tail_is_decoded_again() {
    let engine = FakeTranscriber::new()
        .dropping_second_half()
        .detecting_each(&["tl", "en"]);
    let (rig, id) = switching_turns(&[0], engine, two_languages());
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let calls = rig.transcriber.calls();
    assert_eq!(
        spans(&calls),
        vec![
            (Channel::Mic, r(0, 18_000)),
            (Channel::Mic, r(11_000, 18_000))
        ],
        "the English turn the first pass left without text goes again on its own"
    );
    assert!(calls.iter().all(|c| c.sentence_times));
    assert!(calls.iter().all(|c| c.language == LanguagePref::Auto));
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&transcript.segments), ["isa", "one"]);
    assert_eq!(transcript.detected_languages, tags(&["tl", "en"]));
}

#[test]
fn an_off_list_language_is_redone_pinned() {
    let engine = FakeTranscriber::new().detecting_each(&["es"]);
    let (rig, id) = switching_turns(&[0], engine, two_languages());
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    let languages: Vec<LanguagePref> = rig
        .transcriber
        .calls()
        .into_iter()
        .map(|c| c.language)
        .collect();
    assert_eq!(
        languages,
        [
            LanguagePref::Auto,
            LanguagePref::Pinned(tags(&["tl"]).remove(0))
        ],
        "heard as es in a tl and en meeting, the window goes again pinned to tl"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&transcript.segments), ["isa", "one"]);
    assert_eq!(transcript.detected_languages, tags(&["tl"]));
}

#[test]
fn recovery_off_sends_no_sentence_times() {
    let off = Settings {
        switch_recovery: Recovery::Off,
        ..two_languages()
    };
    let one_language = Settings {
        meeting_languages: tags(&["en"]),
        switch_recovery: Recovery::Auto,
        ..Settings::default()
    };
    for settings in [off, one_language] {
        let engine = FakeTranscriber::new()
            .dropping_second_half()
            .detecting("es");
        let (rig, id) = switching_turns(&[0], engine, settings.clone());
        let dory = rig.open();
        process(&dory, &id, Goal::TRANSCRIBE);
        let calls = rig.transcriber.calls();
        assert_eq!(
            spans(&calls),
            vec![(Channel::Mic, r(0, 18_000))],
            "{settings:?}"
        );
        assert!(!calls[0].sentence_times, "{settings:?}");
        let transcript = dory.library().transcript(&id).unwrap().unwrap();
        assert_eq!(texts(&transcript.segments), ["isa"], "dropped as before");
        assert_eq!(
            transcript.detected_languages,
            tags(&["es"]),
            "kept as heard"
        );
    }
}

#[test]
fn resume_does_not_repeat_a_recovered_stretch() {
    let engine = FakeTranscriber::new()
        .dropping_second_half()
        .detecting("tl");
    let (rig, id) = switching_turns(&[0, 40_000], engine, two_languages());
    rig.transcriber.hold_from(2);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::TRANSCRIBE).unwrap();
    eventually("the second window to run", || {
        rig.transcriber.calls().len() == 3 && rig.transcriber.is_running()
    });
    drop(dory);
    rig.transcriber.hold_from(usize::MAX);
    let saved = rig
        .store
        .checkpoints(&rig.key(&id, STT, Channel::Mic))
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0]
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>(),
        ["isa", "one"],
        "the first window's checkpoint holds its recovered stretch"
    );
    let job = rig.store.jobs_for(&id).unwrap().remove(0);
    rig.store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::Transcribe,
            },
        }])
        .unwrap();

    let dory = rig.open();
    wait_for(&dory, &id, JobState::Succeeded);
    assert_eq!(
        spans(&rig.transcriber.calls()[3..]),
        vec![
            (Channel::Mic, r(40_000, 58_000)),
            (Channel::Mic, r(51_000, 58_000)),
        ],
        "neither the first window nor its stretch runs again"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&transcript.segments), ["isa", "one", "dalawa", "two"]);
}

#[test]
fn a_recovered_stretch_that_drops_its_own_tail_is_decoded_again() {
    let mut entry = stt(STT, DiarizationSupport::None, WINDOW);
    entry.caps.sentence_times = true;
    let lines = [
        (0, 3_000, 0, "isa"),
        (10_000, 13_000, 0, "dalawa"),
        (16_000, 19_000, 0, "tatlo"),
    ];
    let rig = Rig::new(
        FakeTranscriber::new()
            .dropping_second_half()
            .with_segments(Channel::Mic, &lines),
        vec![entry],
        llm(8_192),
    );
    let id = rig.seed("three-turns", ChannelLayout::MicOnly, 70);
    rig.takes.set_activity(
        &id,
        Channel::Mic,
        &[(0, 3_000), (10_000, 13_000), (16_000, 19_000)],
    );
    rig.store
        .apply(vec![Change::SettingsReplaced(two_languages())])
        .unwrap();
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(
        spans(&rig.transcriber.calls()),
        vec![
            (Channel::Mic, r(0, 19_000)),
            (Channel::Mic, r(10_000, 19_000)),
            (Channel::Mic, r(16_000, 19_000)),
        ],
        "each call keeps its first turn, so what it leaves goes again in the next round"
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&transcript.segments), ["isa", "dalawa", "tatlo"]);
}

/// What `a_transcript_that_fits_the_context_is_summarized_in_one_pass` sends with no output
/// language set: the prompt names the spoken language and must stay these exact bytes.
const SAME_AS_SPOKEN_SYSTEM: &str = "You summarize meeting transcripts. Write the whole summary in Vietnamese (BCP 47 tag vi). Use Markdown with three sections in this order: key points, decisions, and action items. Start each section with a \"## \" heading and add no title. Write the three headings in Vietnamese (BCP 47 tag vi) too, even though this instruction is in English. Keep each key point to one short line and merge points that repeat, with at most eight key points. List every decision and every action item, each once, with its owner and due date when the input names them. A task someone will do is an action item, not a decision, even when the meeting calls it a decision. Under a section with nothing to report, write a short Vietnamese (BCP 47 tag vi) phrase that says there is none. Use only what the transcript says.";

#[test]
fn a_summary_that_loops_is_stored_once_and_tidy() {
    let looped = "## Key points\n- Ship it.\n- Ship it.\n## Decisions\n- Ship it.\n## Action items\n- Ana: notes\n## Key points\n- Ship it.\n";
    let (rig, id) = long_meeting(8_192, looped);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    let summary = dory.library().summary(&id).unwrap().unwrap();
    assert_eq!(
        summary.markdown,
        "## Key points\n- Ship it.\n## Decisions\n- Ship it.\n## Action items\n- Ana: notes"
    );
}

#[test]
fn same_as_spoken_sends_todays_summary_prompt_and_runs_todays_stages() {
    let (rig, id) = long_meeting(8_192, SUMMARY);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);

    let prompts = rig.llm.prompts();
    assert_eq!(prompts.len(), 1, "one summary pass and nothing else");
    assert_eq!(prompts[0].system, SAME_AS_SPOKEN_SYSTEM);
    assert!(prompts[0].user.starts_with("[00:00] Speaker 1: l0w0 l0w1"));
    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    assert_eq!(job.goal, Goal::ALL);
    assert_eq!(job.done, [Stage::Transcribe, Stage::Summarize]);
    let summary = dory.library().summary(&id).unwrap().unwrap();
    assert_eq!(summary.language.as_ref().map(|l| l.as_str()), Some("vi"));
}

const TRANSLATED: &str = "## Key points\n- Ship it Friday, in English.";

/// A summary model that translates every `[n] text` line to `[n] EN: text` and writes
/// `TRANSLATED` for anything else.
fn translator() -> FakeLanguageModel {
    FakeLanguageModel::answering(|system, user| {
        if !system.contains("You translate") {
            return TRANSLATED.to_owned();
        }
        user.lines()
            .map(|line| match line.split_once("] ") {
                Some((n, text)) => format!("{n}] EN: {text}"),
                None => line.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn english() -> LangTag {
    LangTag::parse("en").unwrap()
}

fn set_output_language(dory: &Dory, language: Option<LangTag>) {
    dory.update_settings(|s| s.output_language = language)
        .unwrap();
}

/// A mostly English meeting with two Tagalog turns, on one diarized channel.
fn taglish(model: FakeLanguageModel, context: u32) -> (Rig, RecordingId) {
    let rig = Rig::with_model(
        FakeTranscriber::new()
            .with_segments_in(
                Channel::Mixed,
                "en",
                &[
                    (0, 900, 0, "Ship it Friday."),
                    (2_000, 2_900, 0, "Sounds good."),
                ],
            )
            .with_segments_in(
                Channel::Mixed,
                "tl",
                &[
                    (1_000, 1_900, 1, "Sige, ako na bahala."),
                    (3_000, 3_900, 1, "Salamat po."),
                ],
            ),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(context),
        model,
    );
    let id = rig.seed("taglish", ChannelLayout::Mixed, 5);
    (rig, id)
}

fn translate_prompts(rig: &Rig) -> Vec<String> {
    rig.llm
        .prompts()
        .into_iter()
        .filter(|p| p.system.contains("You translate"))
        .map(|p| p.user)
        .collect()
}

#[test]
fn an_output_language_translates_what_is_not_in_it_and_summarizes_in_it() {
    let (rig, id) = taglish(translator(), 8_192);
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    let events = dory.events();
    process(&dory, &id, Goal::ALL);

    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    assert!(job.goal.wants(Stage::Translate));
    assert_eq!(
        job.done,
        [Stage::Transcribe, Stage::Translate, Stage::Summarize]
    );

    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(
        texts(&transcript.segments),
        [
            "Ship it Friday.",
            "Sige, ako na bahala.",
            "Sounds good.",
            "Salamat po."
        ],
        "the spoken transcript is untouched"
    );
    let translation = dory.library().translation(&id).unwrap().unwrap();
    assert_eq!(translation.target, english());
    assert_eq!(translation.transcript_revision, transcript.revision);
    assert_eq!(
        translation.texts,
        [
            None,
            Some("EN: Sige, ako na bahala.".to_owned()),
            None,
            Some("EN: Salamat po.".to_owned())
        ],
        "English turns are not sent to the model"
    );
    assert_eq!(
        translate_prompts(&rig),
        ["[1] Sige, ako na bahala.\n[2] Salamat po.\n"]
    );

    let summary_call = rig.llm.prompts().pop().unwrap();
    assert!(
        summary_call.system.contains("BCP 47 tag en"),
        "{}",
        summary_call.system
    );
    assert!(
        summary_call
            .user
            .contains("Speaker 2: EN: Sige, ako na bahala.")
    );
    assert!(summary_call.user.contains("Speaker 1: Ship it Friday."));
    let summary = dory.library().summary(&id).unwrap().unwrap();
    assert_eq!(summary.language, Some(english()));
    assert_eq!(summary.markdown, TRANSLATED);

    assert_climbs_to_done(&progress(&drain(&events), Stage::Translate));
    assert_eq!(
        rig.store
            .translation_batches(&TranslationKey {
                recording: id.clone(),
                revision: transcript.revision,
                target: english(),
                model: dory_core::models::ModelRef::parse(LLM).unwrap(),
            })
            .unwrap(),
        vec![],
        "batches are cleared with the translation"
    );
}

const TAGLISH: &str = "Bagaman wala ako present sa meeting, I will join later.";

/// An English line and a Taglish line whose grammar is Tagalog, which the engine tags `tl`.
fn mixed_sentence() -> (Rig, RecordingId) {
    let rig = Rig::with_model(
        FakeTranscriber::new()
            .with_segments_in(Channel::Mixed, "en", &[(0, 900, 0, "Ship it Friday.")])
            .with_segments_in(Channel::Mixed, "tl", &[(1_000, 1_900, 1, TAGLISH)]),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(8_192),
        translator(),
    );
    let id = rig.seed("taglish-sentence", ChannelLayout::Mixed, 3);
    (rig, id)
}

#[test]
fn taglish_segment_is_translated_whole_or_not_at_all() {
    let (rig, id) = mixed_sentence();
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    process(&dory, &id, Goal::TRANSCRIBE);
    let spoken = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(texts(&spoken.segments), ["Ship it Friday.", TAGLISH]);
    assert_eq!(
        translate_prompts(&rig),
        [format!("[1] {TAGLISH}\n")],
        "into English the whole Taglish sentence goes to the model and the English one stays"
    );
    let translation = dory.library().translation(&id).unwrap().unwrap();
    assert_eq!(translation.texts, [None, Some(format!("EN: {TAGLISH}"))]);

    let (rig, id) = mixed_sentence();
    let dory = rig.open();
    set_output_language(&dory, Some(LangTag::parse("tl").unwrap()));
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(
        translate_prompts(&rig),
        ["[1] Ship it Friday.\n"],
        "into Tagalog the Taglish sentence is kept whole, with no word of it sent"
    );
    let translation = dory.library().translation(&id).unwrap().unwrap();
    assert_eq!(
        translation.texts,
        [Some("EN: Ship it Friday.".to_owned()), None]
    );
}

#[test]
fn a_meeting_already_in_the_output_language_translates_nothing() {
    let rig = Rig::with_model(
        FakeTranscriber::new().with_segments_in(
            Channel::Mixed,
            "en",
            &[(0, 900, 0, "Ship it Friday.")],
        ),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(8_192),
        translator(),
    );
    let id = rig.seed("english-only", ChannelLayout::Mixed, 3);
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    process(&dory, &id, Goal::ALL);

    assert_eq!(translate_prompts(&rig), Vec::<String>::new());
    assert_eq!(dory.library().translation(&id).unwrap(), None);
    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    assert_eq!(
        job.done,
        [Stage::Transcribe, Stage::Translate, Stage::Summarize]
    );
}

#[test]
fn a_context_too_small_for_the_whole_transcript_translates_in_batches() {
    let lines: Vec<(u64, u64, u16, String)> = (0..40)
        .map(|n| {
            (
                n * 1_000,
                n * 1_000 + 900,
                0,
                format!("kumusta {n} {}", "ka ".repeat(8)),
            )
        })
        .collect();
    let borrowed: Vec<(u64, u64, u16, &str)> = lines
        .iter()
        .map(|(a, b, s, t)| (*a, *b, *s, t.as_str()))
        .collect();
    let rig = Rig::with_model(
        FakeTranscriber::new().with_segments_in(Channel::Mixed, "tl", &borrowed),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(400),
        translator(),
    );
    let id = rig.seed("batches", ChannelLayout::Mixed, 41);
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    process(&dory, &id, Goal::TRANSCRIBE);

    let prompts = translate_prompts(&rig);
    assert!(prompts.len() > 3, "{} batches", prompts.len());
    for call in rig.llm.prompts() {
        let sent = words(&call.system) + words(&call.user);
        assert!(
            sent + call.max_output_tokens <= 400,
            "{sent} + {}",
            call.max_output_tokens
        );
    }
    let translation = dory.library().translation(&id).unwrap().unwrap();
    assert_eq!(translation.texts.len(), 40);
    assert!(
        translation
            .texts
            .iter()
            .all(|t| t.as_deref().is_some_and(|t| t.starts_with("EN: kumusta")))
    );
    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    assert_eq!(job.done, [Stage::Transcribe, Stage::Translate]);
}

#[test]
fn a_quit_mid_translation_resumes_at_the_first_unfinished_batch() {
    let lines: Vec<(u64, u64, u16, String)> = (0..40)
        .map(|n| {
            (
                n * 1_000,
                n * 1_000 + 900,
                0,
                format!("kumusta {n} {}", "ka ".repeat(8)),
            )
        })
        .collect();
    let borrowed: Vec<(u64, u64, u16, &str)> = lines
        .iter()
        .map(|(a, b, s, t)| (*a, *b, *s, t.as_str()))
        .collect();
    let rig = Rig::with_model(
        FakeTranscriber::new().with_segments_in(Channel::Mixed, "tl", &borrowed),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(400),
        translator(),
    );
    let id = rig.seed("resume", ChannelLayout::Mixed, 41);
    rig.llm.hold_from(2);
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    dory.pipeline().enqueue(&id, Goal::TRANSCRIBE).unwrap();
    eventually("the third batch to start", || rig.llm.prompts().len() == 3);
    drop(dory);
    rig.llm.hold_from(usize::MAX);

    let transcript = rig.store.transcript(&id).unwrap().unwrap();
    let key = TranslationKey {
        recording: id.clone(),
        revision: transcript.revision,
        target: english(),
        model: dory_core::models::ModelRef::parse(LLM).unwrap(),
    };
    let saved: usize = rig
        .store
        .translation_batches(&key)
        .unwrap()
        .iter()
        .map(|b| b.texts.len())
        .sum();
    assert!(saved > 0 && saved < 40, "{saved} segments saved");
    assert_eq!(rig.store.translation(&id).unwrap(), None);
    let before = rig.llm.prompts().len();
    let job = rig.store.jobs_for(&id).unwrap().remove(0);
    rig.store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::Translate,
            },
        }])
        .unwrap();

    let dory = rig.open();
    wait_for(&dory, &id, JobState::Succeeded);
    let again = &rig.llm.prompts()[before..];
    let resent: usize = again.iter().map(|p| p.user.lines().count()).sum();
    assert_eq!(resent, 40 - saved, "only what was not saved is sent again");
    let translation = dory.library().translation(&id).unwrap().unwrap();
    assert!(translation.texts.iter().all(Option::is_some));
    assert_eq!(rig.store.translation_batches(&key).unwrap(), vec![]);
}

#[test]
fn cancelling_a_translation_keeps_the_transcript_and_stores_no_translation() {
    let (rig, id) = taglish(translator(), 8_192);
    rig.llm.hold_from(0);
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the translation to start", || !rig.llm.prompts().is_empty());
    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    assert_eq!(
        job.state,
        JobState::Running {
            stage: Stage::Translate
        }
    );
    dory.pipeline().cancel(job.id).unwrap();
    wait_for(&dory, &id, JobState::Cancelled);
    assert!(dory.library().transcript(&id).unwrap().is_some());
    assert_eq!(dory.library().translation(&id).unwrap(), None);
    assert_eq!(dory.library().summary(&id).unwrap(), None);
}

#[test]
fn a_missing_summary_model_blocks_translation_the_way_it_blocks_a_summary() {
    let (rig, id) = taglish(translator(), 8_192);
    let model: ModelId<Llm> = LLM.parse().unwrap();
    rig.llm_provider
        .set_availability(&model, Availability::NeedsProvision { bytes: None });
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            llm: Choice::Pinned(model.clone()),
            output_language: Some(english()),
            ..Settings::default()
        })])
        .unwrap();
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Translate,
            blocker: Blocker::NeedsProvision {
                model: model.erase(),
                bytes: None,
            },
        },
    );
    assert!(dory.library().transcript(&id).unwrap().is_some());
    assert_eq!(rig.llm.prompts(), vec![]);
    assert_eq!(dory.library().translation(&id).unwrap(), None);
}

#[test]
fn turning_the_summary_model_off_skips_translation_with_the_summary() {
    let (rig, id) = taglish(translator(), 8_192);
    let dory = rig.open();
    dory.update_settings(|s| {
        s.output_language = Some(english());
        s.llm = Choice::Off;
    })
    .unwrap();
    process(&dory, &id, Goal::ALL);
    assert_eq!(rig.llm.prompts(), vec![]);
    assert_eq!(dory.library().translation(&id).unwrap(), None);
    assert_eq!(dory.library().summary(&id).unwrap(), None);
}

#[test]
fn a_new_transcript_revision_makes_the_translation_stale_until_the_stage_replaces_it() {
    let (rig, id) = taglish(translator(), 8_192);
    let dory = rig.open();
    set_output_language(&dory, Some(english()));
    process(&dory, &id, Goal::ALL);
    let first = dory.library().translation(&id).unwrap().unwrap();
    assert_eq!(first.transcript_revision, Revision(1));

    // Commit revision 2 by hand: the translation of revision 1 is still stored, and stale.
    let mut next = dory.library().transcript(&id).unwrap().unwrap();
    next.revision = Revision(2);
    rig.store
        .apply(vec![Change::TranscriptReplaced {
            id: id.clone(),
            transcript: next.clone(),
        }])
        .unwrap();
    let stored = dory.library().translation(&id).unwrap().unwrap();
    assert_eq!(
        stored, first,
        "a new revision does not touch the translation"
    );
    assert!(!stored.applies_to(&next));
    assert_eq!(
        TranslationFlag::derive(
            Some((stored.transcript_revision, &stored.target)),
            Some(next.revision),
            Some(&english())
        ),
        TranslationFlag::Stale
    );

    // A re-transcribe makes revision 3 and the stage translates it afresh.
    dory.pipeline().enqueue(&id, Goal::TRANSCRIBE).unwrap();
    eventually("the re-transcribe and its translation", || {
        let jobs = rig.store.jobs_for(&id).unwrap();
        jobs.len() == 2 && jobs.iter().all(|j| j.state == JobState::Succeeded)
    });
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(transcript.revision, Revision(3));
    let translation = dory.library().translation(&id).unwrap().unwrap();
    assert!(translation.applies_to(&transcript));
    assert_eq!(
        TranslationFlag::derive(
            Some((translation.transcript_revision, &translation.target)),
            Some(transcript.revision),
            Some(&english())
        ),
        TranslationFlag::Current
    );
    assert_eq!(translate_prompts(&rig).len(), 2);
}

#[test]
fn a_summary_only_job_writes_in_the_output_language_without_translating() {
    let (rig, id) = taglish(translator(), 8_192);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    assert_eq!(
        translate_prompts(&rig).len(),
        0,
        "no target, no translation"
    );

    set_output_language(&dory, Some(english()));
    dory.pipeline().enqueue(&id, Goal::SUMMARIZE).unwrap();
    eventually("the second summary", || {
        rig.llm.prompts().len() == 2
            && rig
                .store
                .jobs_for(&id)
                .unwrap()
                .iter()
                .all(|j| j.state == JobState::Succeeded)
    });
    assert_eq!(translate_prompts(&rig).len(), 0);
    let summary_call = rig.llm.prompts().pop().unwrap();
    assert!(summary_call.system.contains("BCP 47 tag en"));
    assert!(
        summary_call.user.contains("Sige, ako na bahala."),
        "no translation exists, so the spoken lines are summarized"
    );
}

#[test]
fn an_engine_that_detects_nothing_still_labels_segments_with_the_pinned_language() {
    let transcribe_in = |pinned: &str| {
        let rig = Rig::with_model(
            FakeTranscriber::new().with_segments(
                Channel::Mixed,
                &[
                    (0, 900, 0, "Sige, ako na bahala."),
                    (1_000, 1_900, 0, "Salamat po."),
                ],
            ),
            vec![stt(STT, DiarizationSupport::Native, None)],
            llm(8_192),
            translator(),
        );
        let id = rig.seed("pinned", ChannelLayout::Mixed, 3);
        let dory = rig.open();
        dory.library()
            .set_language(&id, LanguagePref::Pinned(LangTag::parse(pinned).unwrap()))
            .unwrap();
        set_output_language(&dory, Some(english()));
        process(&dory, &id, Goal::ALL);
        (rig, dory, id)
    };

    let (rig, dory, id) = transcribe_in("tl");
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert!(
        transcript
            .segments
            .iter()
            .all(|s| s.language == LangTag::parse("tl").ok())
    );
    assert_eq!(translate_prompts(&rig).len(), 1);

    let (rig, dory, id) = transcribe_in("en");
    assert_eq!(translate_prompts(&rig), Vec::<String>::new());
    assert_eq!(dory.library().translation(&id).unwrap(), None);
}

const TAGALOG_SUMMARY: &str = "# Buod\n\n## Mga pangunahing punto\n- Ihatid sa **Biyernes**.\n- Ayos na ang login.\n\n## Mga desisyon\nWala\n\n## Mga gawain\n1. Susuriin ni Maria ang mga invoice.\n   - bago mag-lunes\n";

/// A recording whose Tagalog summary is stored and nothing else.
fn summarized_in_tagalog(model: FakeLanguageModel, context: u32) -> (Rig, RecordingId) {
    let rig = Rig::with_model(
        FakeTranscriber::new(),
        vec![stt(STT, DiarizationSupport::Native, None)],
        llm(context),
        model,
    );
    let id = rig.seed("buod", ChannelLayout::Mixed, 3);
    write_summary(&rig, &id, TAGALOG_SUMMARY, Some("tl"));
    (rig, id)
}

fn write_summary(rig: &Rig, id: &RecordingId, markdown: &str, language: Option<&str>) {
    rig.store
        .apply(vec![Change::SummaryReplaced {
            id: id.clone(),
            summary: dory_core::domain::Summary {
                markdown: markdown.into(),
                language: language.map(|l| LangTag::parse(l).unwrap()),
                produced_by: dory_core::models::ModelRef::parse(LLM).unwrap(),
                transcript_revision: Revision(1),
            },
        }])
        .unwrap();
}

fn translate_job(dory: &Dory, id: &RecordingId) -> Option<dory_core::domain::Job> {
    dory.library()
        .jobs(id)
        .unwrap()
        .into_iter()
        .find(|j| j.goal == Goal::TRANSLATE_SUMMARY)
}

fn wait_for_translation_job(dory: &Dory, id: &RecordingId, state: JobState) {
    eventually(&format!("the translate job to be {state:?}"), || {
        translate_job(dory, id).is_some_and(|j| j.state == state)
    });
}

fn wait_for_translation_to_block(dory: &Dory, id: &RecordingId) {
    eventually("the translation to block", || {
        translate_job(dory, id).is_some_and(|j| matches!(j.state, JobState::Blocked { .. }))
    });
}

#[test]
fn summary_is_translated_line_by_line() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    let dory = rig.open();
    let events = dory.events();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    wait_for_translation_job(&dory, &id, JobState::Succeeded);

    let job = translate_job(&dory, &id).unwrap();
    assert_eq!(job.done, [Stage::TranslateSummary]);
    assert_eq!(
        job.priority,
        dory_core::domain::Priority::Front,
        "the user is waiting"
    );
    assert_eq!(
        translate_prompts(&rig),
        [
            "[1] Buod\n[2] Mga pangunahing punto\n[3] Ihatid sa **Biyernes**.\n[4] Ayos na ang login.\n[5] Mga desisyon\n[6] Wala\n[7] Mga gawain\n[8] Susuriin ni Maria ang mga invoice.\n[9] bago mag-lunes\n"
        ],
        "the model sees each line's text and none of its markup"
    );
    let translation = dory.library().summary_translation(&id).unwrap().unwrap();
    assert_eq!(translation.target, english());
    assert_eq!(translation.produced_by.to_string(), LLM);
    assert_eq!(
        translation.markdown,
        "# EN: Buod\n\n## EN: Mga pangunahing punto\n- EN: Ihatid sa **Biyernes**.\n- EN: Ayos na ang login.\n\n## EN: Mga desisyon\nEN: Wala\n\n## EN: Mga gawain\n1. EN: Susuriin ni Maria ang mga invoice.\n   - EN: bago mag-lunes\n"
    );
    assert_eq!(
        translation.markdown.lines().count(),
        TAGALOG_SUMMARY.lines().count(),
        "every line of the original has a translated line"
    );
    assert_eq!(
        dory.library().summary(&id).unwrap().unwrap().markdown,
        TAGALOG_SUMMARY,
        "the original is untouched"
    );
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        None
    );
    assert_climbs_to_done(&progress(&drain(&events), Stage::TranslateSummary));
    assert_eq!(
        dory.library().transcript(&id).unwrap(),
        None,
        "a summary translation touches no transcript"
    );
}

#[test]
fn asking_again_for_another_language_replaces_the_translation_and_a_new_summary_drops_it() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    wait_for_translation_job(&dory, &id, JobState::Succeeded);
    let german = LangTag::parse("de").unwrap();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: german.clone(),
                llm: None,
            },
        )
        .unwrap();
    eventually("the German translation", || {
        dory.library()
            .summary_translation(&id)
            .unwrap()
            .is_some_and(|t| t.target == german)
    });
    wait_for_translation_job(&dory, &id, JobState::Succeeded);
    assert_eq!(dory.library().jobs(&id).unwrap().len(), 2);

    write_summary(&rig, &id, "## Bago", Some("tl"));
    assert_eq!(dory.library().summary_translation(&id).unwrap(), None);
}

#[test]
fn translating_into_the_summary_language_is_refused() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    let dory = rig.open();
    for target in ["tl", "tl-PH"] {
        let error = dory
            .library()
            .rerun(
                &id,
                Rerun::TranslateSummary {
                    target: LangTag::parse(target).unwrap(),
                    llm: None,
                },
            )
            .unwrap_err();
        assert!(
            matches!(
                error,
                dory_core::app::DoryError::SummaryRefused(
                    dory_core::domain::SummaryRefusal::AlreadyWritten(_)
                )
            ),
            "{error:?}"
        );
    }
    let nothing = rig.seed("walang-buod", ChannelLayout::Mixed, 3);
    let error = dory
        .library()
        .rerun(
            &nothing,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "There is no summary to translate yet.");
    assert!(
        dory.library()
            .rerun(
                &RecordingId::parse("ghost").unwrap(),
                Rerun::TranslateSummary {
                    target: english(),
                    llm: None
                }
            )
            .is_err()
    );
    assert_eq!(dory.library().jobs(&id).unwrap(), vec![]);
    assert_eq!(dory.library().jobs(&nothing).unwrap(), vec![]);
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        None
    );
    assert_eq!(rig.llm.prompts(), vec![]);
}

#[test]
fn a_second_request_while_one_runs_is_refused_and_the_wish_stays() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    rig.llm.hold_from(0);
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    eventually("the translation to start", || !rig.llm.prompts().is_empty());
    let error = dory
        .library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: LangTag::parse("de").unwrap(),
                llm: None,
            },
        )
        .unwrap_err();
    assert!(
        matches!(error, dory_core::app::DoryError::Underway(_)),
        "{error:?}"
    );
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        Some(english())
    );
}

#[test]
fn translating_waits_for_the_job_that_is_transcribing_again() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    rig.transcriber.hold_from(0);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the transcription to start", || {
        rig.transcriber.is_running()
    });
    let error = dory
        .library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Dory is still processing this recording. Try again when it finishes."
    );
    assert_eq!(
        translate_job(&dory, &id),
        None,
        "nothing queued to translate a summary the job is about to replace"
    );
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        None
    );

    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    dory.pipeline().cancel(job.id).unwrap();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        Some(english()),
        "a cancelled job no longer holds the recording"
    );
}

#[test]
fn no_summary_model_blocks_translation_like_summary() {
    let model: ModelId<Llm> = LLM.parse().unwrap();
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    rig.llm_provider
        .set_availability(&model, Availability::NeedsProvision { bytes: None });
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            llm: Choice::Pinned(model.clone()),
            ..Settings::default()
        })])
        .unwrap();
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    wait_for_translation_to_block(&dory, &id);
    assert_eq!(
        translate_job(&dory, &id).unwrap().state,
        JobState::Blocked {
            stage: Stage::TranslateSummary,
            blocker: Blocker::NeedsProvision {
                model: model.erase(),
                bytes: None,
            },
        },
        "the blocker is the one a missing model gives Summarize"
    );
    assert_eq!(rig.llm.prompts(), vec![]);
    assert_eq!(dory.library().summary_translation(&id).unwrap(), None);

    rig.llm_provider
        .set_availability(&model, Availability::Ready);
    let job = translate_job(&dory, &id).unwrap();
    dory.pipeline().unblock(job.id).unwrap();
    wait_for_translation_job(&dory, &id, JobState::Succeeded);
    assert!(dory.library().summary_translation(&id).unwrap().is_some());
}

#[test]
fn turning_the_summary_model_off_blocks_a_requested_translation_instead_of_skipping_it() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            llm: Choice::Off,
            ..Settings::default()
        })])
        .unwrap();
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    wait_for_translation_job(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::TranslateSummary,
            blocker: Blocker::NoModel { kind: KindTag::Llm },
        },
    );
    assert_eq!(rig.llm.prompts(), vec![]);
}

#[test]
fn a_quit_mid_translation_resumes_once() {
    let lines: String = (0..40)
        .map(|n| format!("- puntong numero {n} {}\n", "ka ".repeat(8)))
        .collect();
    let (rig, id) = summarized_in_tagalog(translator(), 400);
    write_summary(&rig, &id, &lines, Some("tl"));
    rig.llm.hold_from(2);
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    eventually("the third batch to start", || rig.llm.prompts().len() == 3);
    drop(dory);
    rig.llm.hold_from(usize::MAX);

    assert_eq!(rig.store.summary_translation(&id).unwrap(), None);
    assert_eq!(
        rig.store.summary_translation_wanted(&id).unwrap(),
        Some(english()),
        "the wish survives the quit, so the next run knows its target"
    );
    let before = rig.llm.prompts().len();
    let job = rig.store.jobs_for(&id).unwrap().remove(0);
    rig.store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::TranslateSummary,
            },
        }])
        .unwrap();

    let dory = rig.open();
    wait_for_translation_job(&dory, &id, JobState::Succeeded);
    let jobs = dory.library().jobs(&id).unwrap();
    assert_eq!(jobs.len(), 1, "the same job finishes, no second one");
    assert_eq!(jobs[0].done, [Stage::TranslateSummary]);
    let translation = dory.library().summary_translation(&id).unwrap().unwrap();
    assert_eq!(translation.markdown.lines().count(), 40);
    assert!(
        translation
            .markdown
            .lines()
            .all(|l| l.starts_with("- EN: puntong numero"))
    );
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        None
    );
    assert!(rig.llm.prompts().len() > before);
}

#[test]
fn cancelling_a_translation_keeps_the_summary_and_stores_nothing() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    rig.llm.hold_from(0);
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    eventually("the translation to start", || !rig.llm.prompts().is_empty());
    let job = translate_job(&dory, &id).unwrap();
    assert_eq!(
        job.state,
        JobState::Running {
            stage: Stage::TranslateSummary
        }
    );
    dory.pipeline().cancel(job.id).unwrap();
    wait_for_translation_job(&dory, &id, JobState::Cancelled);
    assert_eq!(dory.library().summary_translation(&id).unwrap(), None);
    assert_eq!(
        dory.library().summary(&id).unwrap().unwrap().markdown,
        TAGALOG_SUMMARY
    );
}

#[test]
fn a_summary_written_while_translating_never_gets_the_old_translation() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    rig.llm.hold_from(0);
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    eventually("the translation to start", || !rig.llm.prompts().is_empty());
    write_summary(&rig, &id, "## Bago", Some("tl"));
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        None,
        "the new summary took the wish with it"
    );
    let job = translate_job(&dory, &id).unwrap();
    dory.pipeline().cancel(job.id).unwrap();
    wait_for_translation_job(&dory, &id, JobState::Cancelled);
    assert_eq!(dory.library().summary_translation(&id).unwrap(), None);
}

#[test]
fn trashing_and_restoring_a_recording_brings_its_summary_translation_back() {
    let (rig, id) = summarized_in_tagalog(translator(), 8_192);
    let dory = rig.open();
    dory.library()
        .rerun(
            &id,
            Rerun::TranslateSummary {
                target: english(),
                llm: None,
            },
        )
        .unwrap();
    wait_for_translation_job(&dory, &id, JobState::Succeeded);
    let kept = dory.library().summary_translation(&id).unwrap().unwrap();
    let trashed = dory.library().trash(&id).unwrap();
    assert_eq!(dory.library().summary_translation(&id).unwrap(), None);
    dory.library().restore(trashed).unwrap();
    assert_eq!(dory.library().summary_translation(&id).unwrap(), Some(kept));
    assert_eq!(
        dory.library().summary_translation_wanted(&id).unwrap(),
        None
    );
}

const FIRST_REPLY: &str = "First model words that must not survive.";

fn tag(s: &str) -> LangTag {
    LangTag::parse(s).unwrap()
}

/// A summary model ranked above `fake/llm`, so `Auto` tries it first.
fn preferred(id: &str, languages: LangSet) -> ModelEntry<Llm> {
    let mut entry = llm_entry(id);
    entry.caps.languages = languages;
    entry.rank = 10;
    entry
}

/// One English line on the mic, and a first summary model that refuses as asked.
fn refused_meeting(
    refusal: Refusal,
    calls: usize,
    choice: Choice<Llm>,
) -> (Rig, RecordingId, Arc<FakeLanguageModel>) {
    let first = Arc::new(FakeLanguageModel::new(FIRST_REPLY).refusing(refusal, calls));
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(0, 1_000, 0, "hello")])
            .detecting("en"),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    )
    .with_summary_model(preferred("fake/first", LangSet::Any), first.clone());
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            llm: choice,
            ..Settings::default()
        })])
        .unwrap();
    let id = rig.seed("refused", ChannelLayout::MicOnly, 10);
    (rig, id, first)
}

#[test]
fn auto_summary_falls_back_when_the_engine_rejects_the_language() {
    for refusal in [Refusal::Language("und"), Refusal::Input] {
        let (rig, id, first) = refused_meeting(refusal, usize::MAX, Choice::Auto);
        let dory = rig.open();
        process(&dory, &id, Goal::ALL);
        let summary = dory.library().summary(&id).unwrap().unwrap();
        assert_eq!(summary.produced_by.as_str(), LLM, "{refusal:?}");
        assert_eq!(summary.markdown, SUMMARY, "{refusal:?}");
        assert_eq!(first.prompts().len(), 1, "the first model was tried once");
        assert_eq!(rig.llm.prompts().len(), 1, "the fallback summarized once");
    }
}

#[test]
fn the_draft_resets_on_fallback() {
    let (rig, id, _) = refused_meeting(Refusal::Language("und"), usize::MAX, Choice::Auto);
    let dory = rig.open();
    let events = dory.events();
    process(&dory, &id, Goal::ALL);

    let seen = drain(&events);
    let resets: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, Event::DraftReset { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(resets.len(), 1, "one reset for one fallback");
    let drafted = |events: &[Event]| -> String {
        events
            .iter()
            .filter_map(|e| match e {
                Event::DraftSummary { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        drafted(&seen[..resets[0]]),
        "First ",
        "the first model streamed before it refused"
    );
    assert_eq!(drafted(&seen[resets[0]..]), SUMMARY);
    let stored = dory.library().summary(&id).unwrap().unwrap().markdown;
    assert!(!stored.contains("First"), "{stored}");
}

#[test]
fn a_pinned_summary_does_not_fall_back() {
    let first: ModelId<Llm> = "fake/first".parse().unwrap();
    let (rig, id, _) = refused_meeting(
        Refusal::Language("und"),
        usize::MAX,
        Choice::Pinned(first.clone()),
    );
    let dory = rig.open();
    let events = dory.events();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::Incompatible {
                model: first.erase(),
                why: Mismatch::LanguageUnsupported(Vec::new()),
            },
        },
    );
    assert_eq!(rig.llm.prompts(), vec![], "fake/llm was never asked");
    assert!(
        !drain(&events)
            .iter()
            .any(|e| matches!(e, Event::DraftReset { .. }))
    );
    assert_eq!(dory.library().summary(&id).unwrap(), None);

    let (rig, id, _) = refused_meeting(Refusal::Input, usize::MAX, Choice::Pinned(first));
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the pinned summary to fail", || {
        matches!(job_state(&dory, &id), Some(JobState::Failed { .. }))
    });
    assert_eq!(rig.llm.prompts(), vec![]);
}

#[test]
fn a_summary_no_model_can_read_blocks_and_names_the_language() {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(
                Channel::Mic,
                &[(0, 900, 0, "hello"), (1_000, 1_900, 0, "kumusta")],
            )
            .speaking(&[tag("en"), tag("tl")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        preferred("fake/llm", LangSet::Only(vec![tag("en")])),
    );
    let id = rig.seed("taglish", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::Incompatible {
                model: dory_core::models::ModelRef::parse(LLM).unwrap(),
                why: Mismatch::LanguageUnsupported(vec![tag("tl")]),
            },
        },
    );
    let transcript = dory.library().transcript(&id).unwrap().unwrap();
    assert_eq!(transcript.detected_languages, vec![tag("en"), tag("tl")]);
    assert_eq!(
        rig.llm.prompts(),
        vec![],
        "an English-only model never sees Tagalog"
    );
}

#[test]
fn busy_is_retried_then_fails_after_max_attempts() {
    let (rig, id, first) = refused_meeting(Refusal::Busy, 2, Choice::Auto);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    let summary = dory.library().summary(&id).unwrap().unwrap();
    assert_eq!(
        summary.produced_by.as_str(),
        "fake/first",
        "busy never falls back"
    );
    assert_eq!(
        first.prompts().len(),
        3,
        "two busy answers, then the summary"
    );

    let (rig, id, first) = refused_meeting(Refusal::Busy, 5, Choice::Auto);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the busy summary to fail", || {
        matches!(job_state(&dory, &id), Some(JobState::Failed { .. }))
    });
    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    let JobState::Failed { stage, error } = &job.state else {
        unreachable!()
    };
    assert_eq!(*stage, Stage::Summarize);
    assert_eq!(error.class, FailureClass::Transient, "{error:?}");
    assert!(error.message.contains("busy"), "{}", error.message);
    assert_eq!(job.attempts, dory_core::domain::MAX_ATTEMPTS);
    assert_eq!(
        first.prompts().len(),
        usize::from(dory_core::domain::MAX_ATTEMPTS),
        "each attempt reruns the whole summary"
    );
    assert_eq!(rig.llm.prompts(), vec![], "busy never falls back");
}

#[test]
fn preparing_unblocks_on_check_again() {
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(0, 1_000, 0, "hello")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    );
    let model: ModelId<Llm> = LLM.parse().unwrap();
    let preparing = Unavailable {
        reason: UnavailableReason::Preparing {
            detail: "the system is getting the model ready".into(),
        },
        fix: None,
    };
    rig.llm_provider
        .set_availability(&model, Availability::Unavailable(preparing.clone()));
    let id = rig.seed("preparing", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::Unavailable {
                model: model.erase(),
                why: preparing,
            },
        },
    );
    rig.llm_provider
        .set_availability(&model, Availability::Ready);
    let job = dory.library().jobs(&id).unwrap().pop().unwrap();
    dory.pipeline().unblock(job.id).unwrap();
    wait_for(&dory, &id, JobState::Succeeded);
    assert_eq!(
        dory.library().summary(&id).unwrap().unwrap().produced_by,
        model.erase()
    );
}

#[test]
fn a_fixed_locale_model_runs_in_the_one_meeting_language() {
    let mut english = stt("fake/english", DiarizationSupport::None, WINDOW);
    english.caps.languages = LangSupport::FixedLocale {
        tags: LangSet::Only(vec![tag("en-US")]),
    };
    let (rig, id) = four_windows(vec![english]);
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            meeting_languages: vec![tag("en")],
            ..Settings::default()
        })])
        .unwrap();
    crash_during_the_third_window(&rig, &id);
    let pinned = LanguagePref::Pinned(tag("en"));
    assert!(
        rig.transcriber.calls().iter().all(|c| c.language == pinned),
        "{:?}",
        rig.transcriber.calls()
    );
    let key = CheckpointKey {
        language: pinned,
        ..rig.key(&id, "fake/english", Channel::Mic)
    };
    assert_eq!(
        rig.store.checkpoints(&key).unwrap().len(),
        2,
        "the checkpoint key names the language the model ran in"
    );
}

/// The engine runs en-US until the crash and would pick en-GB after it, as when en-GB's assets
/// arrive while the meeting waits.
#[test]
fn a_resumed_meeting_keeps_the_locale_its_first_window_ran_in() {
    let mut english = stt("fake/english", DiarizationSupport::None, WINDOW);
    english.caps.languages = LangSupport::FixedLocale {
        tags: LangSet::Only(vec![tag("en-GB"), tag("en-US")]),
    };
    let heard =
        FakeTranscriber::new().detecting_each(&["en-US", "en-US", "en-US", "en-GB", "en-GB"]);
    let (rig, id) = four_windows_on(heard, vec![english]);
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            meeting_languages: vec![tag("en")],
            ..Settings::default()
        })])
        .unwrap();
    crash_during_the_third_window(&rig, &id);
    let job = rig.store.jobs_for(&id).unwrap().remove(0);
    rig.store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::Transcribe,
            },
        }])
        .unwrap();
    let dory = rig.open();
    wait_for(&dory, &id, JobState::Succeeded);

    let locales: Vec<_> = rig
        .transcriber
        .calls()
        .into_iter()
        .map(|c| c.locale.map(|l| l.to_string()))
        .collect();
    let us = Some("en-US".to_owned());
    assert_eq!(
        locales,
        [None, us.clone(), us.clone(), us.clone(), us],
        "the first window picks, every later window and the resume keep it"
    );
}

#[test]
fn an_automatic_meeting_never_keeps_a_locale() {
    let heard = FakeTranscriber::new().detecting_each(&["en", "tl", "en", "tl"]);
    let (rig, id) = four_windows_on(heard, vec![stt(STT, DiarizationSupport::None, WINDOW)]);
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::TRANSCRIBE).unwrap();
    wait_for(&dory, &id, JobState::Succeeded);
    let calls = rig.transcriber.calls();
    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|c| c.locale.is_none()), "{calls:?}");
}

#[test]
fn a_fallback_that_cannot_run_now_reports_the_refusal() {
    let llm_model: ModelId<Llm> = LLM.parse().unwrap();
    for (refusal, want_failed) in [(Refusal::Language("und"), false), (Refusal::Input, true)] {
        let (rig, id, _) = refused_meeting(refusal, usize::MAX, Choice::Auto);
        rig.llm_provider
            .set_availability(&llm_model, Availability::NeedsProvision { bytes: Some(7) });
        let dory = rig.open();
        dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
        eventually("the summary to stop", || {
            matches!(
                job_state(&dory, &id),
                Some(JobState::Blocked { .. } | JobState::Failed { .. })
            )
        });
        match job_state(&dory, &id).unwrap() {
            JobState::Failed { stage, error } if want_failed => {
                assert_eq!(stage, Stage::Summarize);
                assert_eq!(error.class, FailureClass::Permanent, "{error:?}");
            }
            JobState::Blocked { stage, blocker } if !want_failed => assert_eq!(
                (stage, blocker),
                (
                    Stage::Summarize,
                    Blocker::Incompatible {
                        model: dory_core::models::ModelRef::parse("fake/first").unwrap(),
                        why: Mismatch::LanguageUnsupported(Vec::new()),
                    }
                ),
                "never a download prompt for a model that did not see the meeting"
            ),
            other => panic!("{refusal:?}: {other:?}"),
        }
        assert_eq!(rig.llm.prompts(), vec![]);
    }
}

#[test]
fn a_refused_language_rules_out_the_next_model_that_cannot_read_it() {
    let first =
        Arc::new(FakeLanguageModel::new(FIRST_REPLY).refusing(Refusal::Language("tl"), usize::MAX));
    let english = Arc::new(FakeLanguageModel::new("English only."));
    let mut english_entry = preferred("fake/english", LangSet::Only(vec![tag("en")]));
    english_entry.rank = 5;
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(0, 1_000, 0, "kumusta")]),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    )
    .with_summary_model(preferred("fake/first", LangSet::Any), first.clone())
    .with_summary_model(english_entry, english.clone());
    let id = rig.seed("undetected-tagalog", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    assert_eq!(first.prompts().len(), 1);
    assert_eq!(
        english.prompts(),
        vec![],
        "the English-only model never got the meeting"
    );
    assert_eq!(
        dory.library()
            .summary(&id)
            .unwrap()
            .unwrap()
            .produced_by
            .as_str(),
        LLM
    );
}

/// A Taglish meeting in a library whose meeting languages were filled from the Mac's English.
fn guessed_english_meeting(choice: Choice<Llm>) -> (Rig, RecordingId, Arc<FakeLanguageModel>) {
    let english_only = Arc::new(FakeLanguageModel::new(FIRST_REPLY));
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(
            Channel::Mic,
            &[(0, 900, 0, "hello"), (1_000, 1_900, 0, "kumusta")],
        ),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    )
    .with_summary_model(
        preferred("fake/english-only", LangSet::Only(vec![tag("en")])),
        english_only.clone(),
    );
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            llm: choice,
            meeting_languages: vec![tag("en")],
            meeting_languages_guessed: true,
            ..Settings::default()
        })])
        .unwrap();
    let id = rig.seed("taglish", ChannelLayout::MicOnly, 10);
    (rig, id, english_only)
}

#[test]
fn a_guessed_meeting_language_leaves_auto_on_a_model_for_any_language() {
    let (rig, id, english_only) = guessed_english_meeting(Choice::Auto);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    let summary = dory.library().summary(&id).unwrap().unwrap();
    assert_eq!(summary.produced_by.as_str(), LLM);
    assert!(
        english_only.prompts().is_empty(),
        "the English-only model never saw Tagalog"
    );
}

#[test]
fn a_guessed_meeting_language_blocks_a_pinned_limited_model_as_unknown() {
    let pinned: ModelId<Llm> = "fake/english-only".parse().unwrap();
    let (rig, id, english_only) = guessed_english_meeting(Choice::Pinned(pinned.clone()));
    let dory = rig.open();
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    wait_for(
        &dory,
        &id,
        JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::Incompatible {
                model: pinned.erase(),
                why: Mismatch::LanguagesUnknown,
            },
        },
    );
    assert!(
        english_only.prompts().is_empty(),
        "the English-only model never saw Tagalog"
    );
}

const ENGLISH_LOCALE: &str = "fake/english-locale";
const ANY_LANGUAGE: &str = "fake/any-language";

/// A speech row that runs in English only, ranked above one that detects any language, as Apple
/// Speech ranks above Whisper.
fn english_locale_row() -> ModelEntry<Stt> {
    let mut entry = stt(ENGLISH_LOCALE, DiarizationSupport::None, WINDOW);
    entry.caps.languages = LangSupport::FixedLocale {
        tags: LangSet::Only(vec![tag("en-US")]),
    };
    entry.rank = 20;
    entry
}

fn any_language_row() -> ModelEntry<Stt> {
    let mut entry = stt(ANY_LANGUAGE, DiarizationSupport::None, WINDOW);
    entry.rank = 10;
    entry
}

/// A two-sided Taglish meeting in a library whose English meeting language is `guessed` or chosen.
/// The engine reports English for every window, which is all a one-locale model can report.
fn english_meeting_with(
    rows: Vec<ModelEntry<Stt>>,
    stt_choice: Choice<Stt>,
    guessed: bool,
) -> (Rig, RecordingId) {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(0, 900, 0, "hello kumusta")])
            .with_segments(Channel::System, &[(1_000, 1_900, 1, "salamat po")])
            .detecting("en"),
        rows,
        llm(8_192),
    );
    rig.store
        .apply(vec![Change::SettingsReplaced(Settings {
            stt: stt_choice,
            meeting_languages: vec![tag("en")],
            meeting_languages_guessed: guessed,
            ..Settings::default()
        })])
        .unwrap();
    let id = rig.seed("taglish-two-sided", ChannelLayout::MicSystem, 10);
    (rig, id)
}

fn transcribed_by(dory: &Dory, id: &RecordingId) -> String {
    dory.library()
        .transcript(id)
        .unwrap()
        .unwrap()
        .produced_by
        .to_string()
}

#[test]
fn a_guessed_meeting_language_keeps_auto_on_the_speech_model_that_detects_languages() {
    let (rig, id) = english_meeting_with(
        vec![english_locale_row(), any_language_row()],
        Choice::Auto,
        true,
    );
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(transcribed_by(&dory, &id), ANY_LANGUAGE);
    assert!(
        rig.transcriber
            .calls()
            .iter()
            .all(|c| c.language == LanguagePref::Auto),
        "the Tagalog half is heard, not read as English: {:?}",
        rig.transcriber.calls()
    );
}

#[test]
fn a_chosen_meeting_language_still_hands_auto_to_the_one_locale_speech_model() {
    let (rig, id) = english_meeting_with(
        vec![english_locale_row(), any_language_row()],
        Choice::Auto,
        false,
    );
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(transcribed_by(&dory, &id), ENGLISH_LOCALE);
    let pinned = LanguagePref::Pinned(tag("en"));
    assert!(rig.transcriber.calls().iter().all(|c| c.language == pinned));
}

#[test]
fn a_guessed_meeting_language_still_runs_a_one_locale_model_when_it_is_all_there_is() {
    let (rig, id) = english_meeting_with(vec![english_locale_row()], Choice::Auto, true);
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(transcribed_by(&dory, &id), ENGLISH_LOCALE);
    let pinned = LanguagePref::Pinned(tag("en"));
    assert!(rig.transcriber.calls().iter().all(|c| c.language == pinned));
}

#[test]
fn a_guessed_meeting_language_does_not_stop_a_pinned_one_locale_model() {
    let pinned: ModelId<Stt> = ENGLISH_LOCALE.parse().unwrap();
    let (rig, id) = english_meeting_with(
        vec![english_locale_row(), any_language_row()],
        Choice::Pinned(pinned),
        true,
    );
    let dory = rig.open();
    process(&dory, &id, Goal::TRANSCRIBE);
    assert_eq!(transcribed_by(&dory, &id), ENGLISH_LOCALE);
}

/// What a guessed English meeting is summarized by after `rows` transcribed it, with an English-only
/// model ranked above `fake/llm`.
fn summarized_after(rows: Vec<ModelEntry<Stt>>) -> (Dory, RecordingId, Arc<FakeLanguageModel>) {
    let english_only = Arc::new(FakeLanguageModel::new(FIRST_REPLY));
    let (rig, id) = english_meeting_with(rows, Choice::Auto, true);
    let rig = rig.with_summary_model(
        preferred("fake/english-only", LangSet::Only(vec![tag("en")])),
        english_only.clone(),
    );
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    (dory, id, english_only)
}

#[test]
fn what_a_one_locale_model_heard_does_not_make_a_guessed_meeting_known() {
    let (dory, id, english_only) = summarized_after(vec![english_locale_row()]);
    assert_eq!(transcribed_by(&dory, &id), ENGLISH_LOCALE);
    assert_eq!(
        dory.library()
            .summary(&id)
            .unwrap()
            .unwrap()
            .produced_by
            .as_str(),
        LLM
    );
    assert!(
        english_only.prompts().is_empty(),
        "the engine could only report English, so the Tagalog half is unknown"
    );
}

#[test]
fn what_a_detecting_model_heard_still_counts_when_the_meeting_is_guessed() {
    let (dory, id, english_only) = summarized_after(vec![any_language_row()]);
    assert_eq!(transcribed_by(&dory, &id), ANY_LANGUAGE);
    assert_eq!(
        dory.library()
            .summary(&id)
            .unwrap()
            .unwrap()
            .produced_by
            .as_str(),
        "fake/english-only"
    );
    assert_eq!(english_only.prompts().len(), 1);
}

/// One English line, with `fake/first` ranked above `fake/llm`, so `Auto` picks it, and the
/// meeting already transcribed and summarized by it.
fn summarized_by_first(first: FakeLanguageModel) -> (Rig, RecordingId, Dory) {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(0, 1_000, 0, "hello")])
            .detecting("en"),
        vec![
            stt(STT, DiarizationSupport::None, WINDOW),
            stt("fake/zz-stt", DiarizationSupport::None, WINDOW),
        ],
        llm(8_192),
    )
    .with_summary_model(preferred("fake/first", LangSet::Any), Arc::new(first));
    let id = rig.seed("picked", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    (rig, id, dory)
}

fn summarize_with(dory: &Dory, id: &RecordingId, llm: Option<&str>) -> Result<(), DoryError> {
    dory.library().rerun(
        id,
        Rerun::Summarize {
            llm: llm.map(|m| m.parse().unwrap()),
        },
    )
}

fn summary_jobs(dory: &Dory, id: &RecordingId) -> Vec<dory_core::domain::Job> {
    dory.library()
        .jobs(id)
        .unwrap()
        .into_iter()
        .filter(|j| j.goal == Goal::SUMMARIZE)
        .collect()
}

#[test]
fn a_summary_rerun_runs_the_picked_model_once_and_settings_stay() {
    let (rig, id, dory) = summarized_by_first(FakeLanguageModel::new(FIRST_REPLY));
    let by = |dory: &Dory| dory.library().summary(&id).unwrap().unwrap().produced_by;
    assert_eq!(by(&dory).as_str(), "fake/first", "Auto's pick");

    summarize_with(&dory, &id, Some(LLM)).unwrap();
    eventually("the picked re-summary", || {
        summary_jobs(&dory, &id)
            .iter()
            .any(|j| j.state == JobState::Succeeded)
    });
    assert_eq!(
        by(&dory).as_str(),
        LLM,
        "produced_by names the model that ran"
    );
    assert_eq!(
        summary_jobs(&dory, &id)[0].picks.llm,
        Some(LLM.parse().unwrap())
    );
    assert_eq!(rig.llm.prompts().len(), 1);
    assert_eq!(
        dory.settings().unwrap().llm,
        Choice::Auto,
        "Settings never change"
    );

    summarize_with(&dory, &id, None).unwrap();
    eventually("the second re-summary", || {
        summary_jobs(&dory, &id)
            .iter()
            .filter(|j| j.state == JobState::Succeeded)
            .count()
            == 2
    });
    assert_eq!(
        by(&dory).as_str(),
        "fake/first",
        "without a pick, Auto again"
    );
}

#[test]
fn a_picked_model_that_refuses_never_falls_back_and_another_pick_replaces_its_job() {
    let first = FakeLanguageModel::new(FIRST_REPLY).refusing(Refusal::Language("und"), usize::MAX);
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(0, 1_000, 0, "hello")])
            .detecting("en"),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    )
    .with_summary_model(llm_entry("fake/first"), Arc::new(first));
    let id = rig.seed("refusing-pick", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    assert_eq!(
        dory.library()
            .summary(&id)
            .unwrap()
            .unwrap()
            .produced_by
            .as_str(),
        LLM
    );

    summarize_with(&dory, &id, Some("fake/first")).unwrap();
    eventually("the picked model to block the job", || {
        summary_jobs(&dory, &id)
            .iter()
            .any(|j| matches!(j.state, JobState::Blocked { .. }))
    });
    assert_eq!(rig.llm.prompts().len(), 1, "fake/llm was not asked again");
    let blocked = summary_jobs(&dory, &id)[0].clone();

    summarize_with(&dory, &id, Some("fake/first")).unwrap();
    assert_eq!(
        summary_jobs(&dory, &id),
        std::slice::from_ref(&blocked),
        "the same pick keeps the waiting job"
    );

    summarize_with(&dory, &id, Some(LLM)).unwrap();
    eventually("the new pick to run", || {
        summary_jobs(&dory, &id)
            .iter()
            .any(|j| j.state == JobState::Succeeded)
    });
    let jobs = summary_jobs(&dory, &id);
    assert_eq!(jobs.len(), 2);
    assert_eq!(
        (jobs[0].id, &jobs[0].state),
        (blocked.id, &JobState::Cancelled)
    );
    assert_eq!(rig.llm.prompts().len(), 2);
}

#[test]
fn a_pick_the_catalog_lacks_or_that_cannot_read_the_meeting_is_refused() {
    let rig = Rig::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &[(0, 1_000, 0, "kumusta")])
            .detecting("tl"),
        vec![stt(STT, DiarizationSupport::None, WINDOW)],
        llm(8_192),
    )
    .with_summary_model(
        preferred("fake/en-only", LangSet::Only(vec![tag("en")])),
        Arc::new(FakeLanguageModel::new(FIRST_REPLY)),
    );
    let id = rig.seed("tagalog", ChannelLayout::MicOnly, 10);
    let dory = rig.open();
    process(&dory, &id, Goal::ALL);
    for pick in ["fake/en-only", "fake/missing"] {
        assert!(
            matches!(
                summarize_with(&dory, &id, Some(pick)),
                Err(DoryError::Invalid(_))
            ),
            "{pick}"
        );
    }
    assert_eq!(summary_jobs(&dory, &id).len(), 0, "nothing was queued");
}

#[test]
fn a_transcribe_rerun_runs_the_picked_speech_model_in_the_language_asked() {
    let (_rig, id, dory) = summarized_by_first(FakeLanguageModel::new(FIRST_REPLY));
    assert_eq!(
        dory.library()
            .transcript(&id)
            .unwrap()
            .unwrap()
            .produced_by
            .as_str(),
        STT
    );
    dory.library()
        .rerun(
            &id,
            Rerun::Transcribe {
                language: LanguagePref::Pinned(tag("en")),
                stt: Some("fake/zz-stt".parse().unwrap()),
            },
        )
        .unwrap();
    eventually("the re-transcribe", || {
        dory.library()
            .transcript(&id)
            .unwrap()
            .is_some_and(|t| t.produced_by.as_str() == "fake/zz-stt")
    });
    assert_eq!(
        dory.library().recording(&id).unwrap().unwrap().language,
        LanguagePref::Pinned(tag("en"))
    );
    assert_eq!(dory.settings().unwrap().stt, Choice::Auto);
    let menu = dory_core::testing::block_on(dory.library().speech_models(&id)).unwrap();
    assert_eq!(
        menu.default,
        dory_core::models::Resolution::Use(STT.parse().unwrap())
    );
    assert_eq!(menu.models.len(), 2);
}

#[test]
fn a_rerun_with_a_picked_model_waits_for_the_job_underway() {
    let (rig, id, dory) = summarized_by_first(FakeLanguageModel::new(FIRST_REPLY));
    rig.transcriber.hold_from(0);
    dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
    eventually("the transcription to start", || {
        rig.transcriber.is_running()
    });
    let before = dory.library().jobs(&id).unwrap();
    for rerun in [
        Rerun::Summarize {
            llm: Some(LLM.parse().unwrap()),
        },
        Rerun::Summarize { llm: None },
        Rerun::Transcribe {
            language: LanguagePref::Auto,
            stt: Some("fake/zz-stt".parse().unwrap()),
        },
    ] {
        let error = dory.library().rerun(&id, rerun.clone()).unwrap_err();
        assert!(
            matches!(error, DoryError::Underway(_)),
            "{rerun:?}: {error:?}"
        );
    }
    assert_eq!(
        dory.library().jobs(&id).unwrap(),
        before,
        "nothing queued or cancelled beside the job underway"
    );

    let running = before.last().unwrap().id;
    dory.pipeline().cancel(running).unwrap();
    summarize_with(&dory, &id, Some(LLM)).unwrap();
    eventually("the picked re-summary once nothing is underway", || {
        dory.library()
            .summary(&id)
            .unwrap()
            .is_some_and(|s| s.produced_by.as_str() == LLM)
    });
}

/// A recording whose speaker was renamed, a Settings term, and a second recording to transcribe.
fn vocabulary_rig(engine_takes_vocabulary: bool) -> (Rig, RecordingId) {
    let mut entry = stt(STT, DiarizationSupport::None, WINDOW);
    entry.caps.vocabulary = engine_takes_vocabulary;
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(Channel::Mic, &[(1_000, 2_000, 0, "hello")]),
        vec![entry],
        llm(8_192),
    );
    let earlier = rig.seed("earlier", ChannelLayout::MicOnly, 10);
    let next = rig.seed("next", ChannelLayout::MicOnly, 10);
    rig.store
        .apply(vec![
            Change::SpeakerNamesReplaced {
                id: earlier,
                names: SpeakerNames {
                    for_revision: Revision(1),
                    names: [(SpeakerId(1), "Lina".to_owned())].into(),
                },
            },
            Change::SettingsReplaced(Settings {
                vocabulary: vec![VocabularyTerm::new("Priya", Boost::Strong)],
                ..Settings::default()
            }),
        ])
        .unwrap();
    (rig, next)
}

#[test]
fn a_renamed_speaker_and_a_settings_term_reach_the_engine_on_the_next_recording() {
    let (rig, next) = vocabulary_rig(true);
    let dory = rig.open();
    assert_eq!(
        dory.speech_vocabulary().unwrap(),
        [
            VocabularyTerm::new("Priya", Boost::Strong),
            VocabularyTerm::new("Lina", Boost::Normal)
        ]
    );
    process(&dory, &next, Goal::TRANSCRIBE);
    let calls = rig.transcriber.calls();
    assert!(!calls.is_empty());
    assert!(
        calls.iter().all(|c| c.vocabulary == ["Priya", "Lina"]),
        "every decode gets the terms, then the names: {calls:?}"
    );
}

#[test]
fn an_engine_without_vocabulary_gets_none() {
    let (rig, next) = vocabulary_rig(false);
    let dory = rig.open();
    process(&dory, &next, Goal::TRANSCRIBE);
    let calls = rig.transcriber.calls();
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|c| c.vocabulary.is_empty()), "{calls:?}");
}
