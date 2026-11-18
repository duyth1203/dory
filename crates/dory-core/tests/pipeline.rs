//! The transcribe and summarize policy, driven through the `Dory` facade with fakes: windows,
//! silence skipping, checkpoints across a crash, speaker numbering, merge and bleed, map-reduce
//! summaries, and blocked models.

use std::sync::Arc;
use std::time::Duration;

use dory_core::app::{Dory, Event, Options, Ports};
use dory_core::domain::{
    Blocker, Channel, ChannelLayout, FailureCause, Goal, JobEvent, JobState, LanguagePref, Ms,
    Recording, RecordingId, Segment, Settings, SpeakerId, Stage, TimeRange, Timestamp,
};
use dory_core::models::{
    Availability, Choice, DiarizationSupport, KindTag, Llm, ModelEntry, ModelId, Stt,
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
        FakeTranscriber::new(),
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
        FakeTranscriber::new(),
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

/// Gemma stops a note at `max_output_tokens` of its own tokens, but the Ollama adapter counts
/// a third or more above that, so a note at its cap counts past the cap. Merged notes reach the
/// cap more often than notes on transcript, so a run that merges twice can be left with notes
/// no two of which fit one capped pass. A two-hour meeting in a 32k context hit that in 3 of 9
/// live runs.
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
