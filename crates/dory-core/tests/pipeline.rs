//! The transcribe and summarize policy, driven through the `Dory` facade with fakes: windows,
//! silence skipping, checkpoints across a crash, speaker numbering, merge and bleed, map-reduce
//! summaries, and blocked models.

use std::sync::Arc;
use std::time::Duration;

use dory_core::app::{Dory, Event, Options, Ports};
use dory_core::domain::{
    Blocker, Channel, ChannelLayout, FailureCause, Goal, JobEvent, JobState, LanguagePref, Ms,
    Recording, RecordingId, Recovery, Segment, Settings, SpeakerId, Stage, TimeRange, Timestamp,
};
use dory_core::models::{
    Availability, Choice, DiarizationSupport, KindTag, Llm, Mismatch, ModelEntry, ModelId, Stt,
};
use dory_core::ports::{Change, CheckpointKey, Store, StoreRead, Takes};
use dory_core::testing::store::MemStore;
use dory_core::testing::{
    FakeCapture, FakeDiarProvider, FakeDiarizer, FakeLanguageModel, FakeLlmProvider,
    FakeSttProvider, FakeTranscriber, ManualClock, MemTakes, TranscribeCall, diar_entry,
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
        let transcriber = Arc::new(transcriber);
        let model = Arc::new(FakeLanguageModel::new(reply));
        Rig {
            store: Arc::new(MemStore::new()),
            takes: MemTakes::new(),
            clock: Arc::new(ManualClock::new(Timestamp(1_763_300_000_000))),
            stt: Arc::new(FakeSttProvider::new("fake", stt, transcriber.clone())),
            llm_provider: Arc::new(FakeLlmProvider::new("fake", vec![llm], model.clone())),
            transcriber,
            llm: model,
            diarizer: None,
        }
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
            llm: vec![self.llm_provider.clone()],
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
    let rig = Rig::new(
        FakeTranscriber::new().with_segments(
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
