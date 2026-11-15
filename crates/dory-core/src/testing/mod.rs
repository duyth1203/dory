//! Fakes for the ports, for this crate's tests and for adapter crates that enable the `testing`
//! feature.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};

use crate::capture::FrameSink;
use crate::domain::{
    Channel, ChannelLayout, Fraction, LangSet, LangTag, LanguagePref, Ms, RecordingId,
    TakeContainer, TakeInfo, TimeRange, Timestamp,
};
use crate::models::{
    Availability, DiarizationSupport, LangSupport, Llm, LlmCaps, ModelEntry, ModelId, ModelKind,
    ProviderId, Stt, SttCaps,
};
use crate::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, CaptureHandle, CaptureIssue, CaptureSpec,
    InputDevice, Source,
};
use crate::ports::{
    CancelToken, Clip, Clock, CompletionRequest, EngineError, LanguageModel, ModelProvider,
    ProvisionError, ProvisionSink, RawSegment, SegmentSink, TakeError, TakeWriter, Takes, TextSink,
    TranscribeOutcome, TranscribeRequest, Transcriber,
};

/// A clock that moves only when a test says so.
#[derive(Debug, Default)]
pub struct ManualClock {
    wall_ms: AtomicI64,
    mono_ms: AtomicU64,
}

impl ManualClock {
    pub fn new(wall: Timestamp) -> Self {
        Self {
            wall_ms: AtomicI64::new(wall.0),
            mono_ms: AtomicU64::new(0),
        }
    }

    /// Advances both clocks by the same amount.
    pub fn advance_ms(&self, ms: u64) {
        self.wall_ms
            .fetch_add(i64::try_from(ms).unwrap_or(i64::MAX), Ordering::SeqCst);
        self.mono_ms.fetch_add(ms, Ordering::SeqCst);
    }

    /// Jumps the wall clock without touching the monotonic one, like a user changing the date.
    pub fn set_wall(&self, wall: Timestamp) {
        self.wall_ms.store(wall.0, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.wall_ms.load(Ordering::SeqCst))
    }

    fn monotonic_ms(&self) -> u64 {
        self.mono_ms.load(Ordering::SeqCst)
    }
}

/// One step of a scripted capture run.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Interleaved samples, pushed as one call.
    Frames(Vec<f32>),
    /// `frames` whole frames of digital silence.
    Silence {
        frames: usize,
    },
    Issue(CaptureIssue),
    /// Waits before the next step, the way a device delivers audio over time.
    Wait(std::time::Duration),
}

type Script = Vec<Step>;

/// A [`CaptureBackend`] that pushes scripted frames into the sink from its own thread, the way a
/// real adapter does from an audio callback. Each `start` consumes the next script, so a test can
/// script what a pause-and-resume delivers in each interval. A start with no script left delivers
/// nothing. The sink stays alive until the handle is stopped, so `FrameSource::is_finished` turns
/// true only after `stop`.
pub struct FakeCapture {
    caps: Mutex<CaptureCaps>,
    scripts: Mutex<VecDeque<Script>>,
    start_error: Mutex<Option<CaptureError>>,
    starts: AtomicUsize,
    live: Arc<AtomicUsize>,
    specs: Mutex<Vec<CaptureSpec>>,
}

impl FakeCapture {
    pub fn new() -> Self {
        Self::scripted(Vec::new())
    }

    pub fn scripted(scripts: Vec<Script>) -> Self {
        Self {
            caps: Mutex::new(CaptureCaps {
                microphone: Access::Granted,
                system_audio: Access::Granted,
                unsupported: None,
            }),
            scripts: Mutex::new(scripts.into()),
            start_error: Mutex::new(None),
            starts: AtomicUsize::new(0),
            live: Arc::new(AtomicUsize::new(0)),
            specs: Mutex::new(Vec::new()),
        }
    }

    pub fn with_caps(self, caps: CaptureCaps) -> Self {
        *lock(&self.caps) = caps;
        self
    }

    /// The next `start` fails with `error`. Later starts succeed.
    pub fn fail_next_start(&self, error: CaptureError) {
        *lock(&self.start_error) = Some(error);
    }

    pub fn push_script(&self, script: Script) {
        lock(&self.scripts).push_back(script);
    }

    pub fn start_count(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    /// Handles started and not yet stopped. Zero means the device is released.
    pub fn running(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    pub fn specs(&self) -> Vec<CaptureSpec> {
        lock(&self.specs).clone()
    }
}

impl Default for FakeCapture {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl CaptureBackend for FakeCapture {
    fn probe(&self) -> CaptureCaps {
        lock(&self.caps).clone()
    }

    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        Ok(vec![InputDevice {
            uid: "fake-mic".into(),
            name: "Fake Microphone".into(),
            is_default: true,
        }])
    }

    fn request_access(&self, source: Source) -> Result<Access, CaptureError> {
        let caps = lock(&self.caps);
        Ok(match source {
            Source::Microphone => caps.microphone,
            Source::SystemAudio => caps.system_audio,
        })
    }

    fn start(
        &self,
        spec: &CaptureSpec,
        mut sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        if let Some(error) = lock(&self.start_error).take() {
            return Err(error);
        }
        self.starts.fetch_add(1, Ordering::SeqCst);
        lock(&self.specs).push(spec.clone());
        let script = lock(&self.scripts).pop_front().unwrap_or_default();
        let channels = sink.channels();

        let (stop_tx, stop_rx): (Sender<()>, Receiver<()>) = crossbeam_channel::bounded(1);
        self.live.fetch_add(1, Ordering::SeqCst);
        let thread = std::thread::spawn(move || {
            for step in script {
                match step {
                    Step::Frames(samples) => sink.push(&samples),
                    Step::Silence { frames } => sink.push(&vec![0.0; frames * channels]),
                    Step::Issue(issue) => sink.report(issue),
                    Step::Wait(duration) => std::thread::sleep(duration),
                }
            }
            let _ = stop_rx.recv();
        });
        Ok(Box::new(FakeHandle {
            stop: Some(stop_tx),
            thread: Some(thread),
            live: Arc::clone(&self.live),
        }))
    }
}

struct FakeHandle {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    live: Arc<AtomicUsize>,
}

impl CaptureHandle for FakeHandle {
    fn stop(mut self: Box<Self>) {
        self.release();
    }
}

impl FakeHandle {
    fn release(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for FakeHandle {
    fn drop(&mut self) {
        self.release();
    }
}

/// A [`Takes`] that keeps samples in memory. A writer dropped without `finish` leaves its take
/// open, the way a crash leaves a WAV behind, so `orphans` lists it.
#[derive(Clone, Default)]
pub struct MemTakes {
    takes: Arc<Mutex<BTreeMap<RecordingId, MemTake>>>,
}

#[derive(Clone, Debug)]
struct MemTake {
    layout: ChannelLayout,
    rate_hz: u32,
    samples: Vec<f32>,
    sealed: bool,
}

impl MemTake {
    fn frames(&self) -> u64 {
        (self.samples.len() / self.layout.channel_count()) as u64
    }

    fn info(&self) -> TakeInfo {
        TakeInfo {
            container: if self.sealed {
                TakeContainer::Flac
            } else {
                TakeContainer::Wav
            },
            rate_hz: self.rate_hz,
            duration: NonZeroU32::new(self.rate_hz)
                .map_or(Ms(0), |rate| Ms::from_frames(self.frames(), rate)),
        }
    }
}

impl MemTakes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whole frames written to the take, or `None` if there is none.
    pub fn frames(&self, id: &RecordingId) -> Option<u64> {
        lock(&self.takes).get(id).map(MemTake::frames)
    }

    pub fn is_sealed(&self, id: &RecordingId) -> bool {
        lock(&self.takes).get(id).is_some_and(|t| t.sealed)
    }

    fn with<T>(&self, id: &RecordingId, f: impl FnOnce(&mut MemTake) -> T) -> Result<T, TakeError> {
        lock(&self.takes)
            .get_mut(id)
            .map(f)
            .ok_or_else(|| TakeError::Missing(id.clone()))
    }
}

impl Takes for MemTakes {
    fn begin(
        &self,
        id: &RecordingId,
        layout: ChannelLayout,
        rate_hz: u32,
    ) -> Result<Box<dyn TakeWriter>, TakeError> {
        let mut takes = lock(&self.takes);
        if takes.contains_key(id) {
            return Err(TakeError::Exists(id.clone()));
        }
        takes.insert(
            id.clone(),
            MemTake {
                layout,
                rate_hz,
                samples: Vec::new(),
                sealed: false,
            },
        );
        Ok(Box::new(MemTakeWriter {
            takes: self.clone(),
            id: id.clone(),
            channels: layout.channel_count(),
            frames: 0,
        }))
    }

    fn seal(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        self.with(id, |take| {
            take.sealed = true;
            take.info()
        })
    }

    fn clip(
        &self,
        id: &RecordingId,
        channel: Channel,
        span: Option<TimeRange>,
    ) -> Result<Clip, TakeError> {
        let info = self.with(id, |take| {
            take.layout
                .index_of(channel)
                .map(|_| take.info())
                .ok_or_else(|| TakeError::Decode(format!("no {channel:?} channel")))
        })??;
        let origin = span.map_or(Ms(0), |s| s.start());
        let duration = span.map_or(info.duration, |s| s.len());
        Ok(Clip::new(
            PathBuf::from(format!("mem/{id}/{channel:?}")),
            channel,
            duration,
            origin,
            Vec::new(),
            None,
        ))
    }

    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError> {
        Ok(lock(&self.takes)
            .iter()
            .filter(|(_, take)| !take.sealed)
            .map(|(id, _)| id.clone())
            .collect())
    }

    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError> {
        Ok(lock(&self.takes).get(id).map(MemTake::info))
    }

    fn import(&self, id: &RecordingId, _source: &Path) -> Result<TakeInfo, TakeError> {
        let mut takes = lock(&self.takes);
        if takes.contains_key(id) {
            return Err(TakeError::Exists(id.clone()));
        }
        let take = MemTake {
            layout: ChannelLayout::Mixed,
            rate_hz: 16_000,
            samples: Vec::new(),
            sealed: true,
        };
        let info = take.info();
        takes.insert(id.clone(), take);
        Ok(info)
    }

    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        self.with(id, |_| PathBuf::from(format!("mem/{id}")))
    }

    fn delete(&self, id: &RecordingId) -> Result<(), TakeError> {
        lock(&self.takes).remove(id);
        Ok(())
    }
}

struct MemTakeWriter {
    takes: MemTakes,
    id: RecordingId,
    channels: usize,
    frames: u64,
}

impl TakeWriter for MemTakeWriter {
    fn write(&mut self, interleaved: &[f32]) -> Result<(), TakeError> {
        let whole = interleaved.len() / self.channels * self.channels;
        self.takes.with(&self.id, |take| {
            take.samples.extend_from_slice(&interleaved[..whole]);
        })?;
        self.frames += (whole / self.channels) as u64;
        Ok(())
    }

    fn checkpoint(&mut self) -> Result<(), TakeError> {
        Ok(())
    }

    fn frames_written(&self) -> u64 {
        self.frames
    }

    fn finish(self: Box<Self>) -> Result<TakeInfo, TakeError> {
        self.takes.with(&self.id, |take| take.info())
    }
}

/// A [`ModelProvider`] of any kind that hands out one shared engine. Every model starts Ready
/// unless a test says otherwise. Provisioning makes a model Ready.
pub struct FakeProvider<K: ModelKind> {
    id: ProviderId,
    entries: Vec<ModelEntry<K>>,
    availability: Mutex<HashMap<ModelId<K>, Availability>>,
    engine: Arc<K::Engine>,
    /// References to the engine that existed when the provider was built.
    baseline: usize,
    opens: AtomicUsize,
    provisions: AtomicUsize,
}

pub type FakeSttProvider = FakeProvider<Stt>;
pub type FakeLlmProvider = FakeProvider<Llm>;

impl<K: ModelKind> FakeProvider<K> {
    pub fn new(provider: &str, entries: Vec<ModelEntry<K>>, engine: Arc<K::Engine>) -> Self {
        Self {
            id: ProviderId::parse(provider).expect("a valid provider id"),
            entries,
            availability: Mutex::new(HashMap::new()),
            baseline: Arc::strong_count(&engine),
            engine,
            opens: AtomicUsize::new(0),
            provisions: AtomicUsize::new(0),
        }
    }

    pub fn set_availability(&self, model: &ModelId<K>, availability: Availability) {
        lock(&self.availability).insert(model.clone(), availability);
    }

    pub fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    pub fn provisions(&self) -> usize {
        self.provisions.load(Ordering::SeqCst)
    }

    /// Engines opened and not yet dropped by whoever opened them. Counted against the
    /// references that existed when the provider was built, so a test that clones the engine
    /// afterwards skews it.
    pub fn live_engines(&self) -> usize {
        Arc::strong_count(&self.engine).saturating_sub(self.baseline)
    }
}

impl<K: ModelKind> ModelProvider<K> for FakeProvider<K> {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn catalog(&self) -> Vec<ModelEntry<K>> {
        self.entries.clone()
    }

    fn availability(&self, model: &ModelId<K>) -> Availability {
        lock(&self.availability)
            .get(model)
            .cloned()
            .unwrap_or(Availability::Ready)
    }

    fn provision(
        &self,
        model: &ModelId<K>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError> {
        self.provisions.fetch_add(1, Ordering::SeqCst);
        progress.step("Downloading");
        for done in [0, 50, 100] {
            if cancel.is_cancelled() {
                return Err(ProvisionError::Cancelled);
            }
            progress.bytes(done, Some(100));
        }
        self.set_availability(model, Availability::Ready);
        Ok(())
    }

    fn open(
        &self,
        model: &ModelId<K>,
        _cancel: &CancelToken,
    ) -> Result<Arc<K::Engine>, EngineError> {
        if self.availability(model) != Availability::Ready {
            return Err(EngineError::ModelMissing(model.to_string()));
        }
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::clone(&self.engine))
    }
}

/// A speech engine that returns scripted segments per channel.
#[derive(Default)]
pub struct FakeTranscriber {
    segments: HashMap<Channel, Vec<RawSegment>>,
    detected: Option<LangTag>,
    hold: AtomicBool,
    running: AtomicBool,
    calls: Mutex<Vec<TranscribeCall>>,
}

/// What one `transcribe` call asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscribeCall {
    pub channel: Channel,
    pub language: LanguagePref,
    pub diarize: bool,
}

impl FakeTranscriber {
    pub fn new() -> Self {
        Self::default()
    }

    /// `(start_ms, end_ms, engine speaker, text)` for clips of `channel`.
    pub fn with_segments(mut self, channel: Channel, segments: &[(u64, u64, u16, &str)]) -> Self {
        let raw = segments
            .iter()
            .map(|(start, end, speaker, text)| RawSegment {
                range: TimeRange::new(Ms(*start), Ms(*end)).expect("start before end"),
                text: (*text).to_owned(),
                speaker: Some(*speaker),
                language: None,
            });
        self.segments.entry(channel).or_default().extend(raw);
        self
    }

    pub fn detecting(mut self, language: &str) -> Self {
        self.detected = Some(LangTag::parse(language).expect("a valid language tag"));
        self
    }

    /// While held, `transcribe` waits until it is cancelled.
    pub fn hold(&self, hold: bool) {
        self.hold.store(hold, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn calls(&self) -> Vec<TranscribeCall> {
        lock(&self.calls).clone()
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(
        &self,
        req: &TranscribeRequest<'_>,
        sink: &mut dyn SegmentSink,
        cancel: &CancelToken,
    ) -> Result<TranscribeOutcome, EngineError> {
        lock(&self.calls).push(TranscribeCall {
            channel: req.clip.channel,
            language: req.language.clone(),
            diarize: req.diarize,
        });
        self.running.store(true, Ordering::SeqCst);
        while self.hold.load(Ordering::SeqCst) && !cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let result = if cancel.is_cancelled() {
            Err(EngineError::Cancelled)
        } else {
            let segments = self
                .segments
                .get(&req.clip.channel)
                .cloned()
                .unwrap_or_default();
            let total = segments.len().max(1) as f32;
            for (i, segment) in segments.into_iter().enumerate() {
                sink.segment(segment);
                sink.progress(Fraction::new((i + 1) as f32 / total));
            }
            Ok(TranscribeOutcome {
                detected_language: self.detected.clone(),
            })
        };
        self.running.store(false, Ordering::SeqCst);
        result
    }
}

/// A summary model that answers every prompt with the same text and keeps the prompts.
pub struct FakeLanguageModel {
    reply: String,
    prompts: Mutex<Vec<(String, String)>>,
}

impl FakeLanguageModel {
    pub fn new(reply: &str) -> Self {
        Self {
            reply: reply.to_owned(),
            prompts: Mutex::new(Vec::new()),
        }
    }

    /// `(system, user)` for each completion asked for.
    pub fn prompts(&self) -> Vec<(String, String)> {
        lock(&self.prompts).clone()
    }
}

impl LanguageModel for FakeLanguageModel {
    fn count_tokens(&self, text: &str) -> u32 {
        u32::try_from(text.split_whitespace().count()).unwrap_or(u32::MAX)
    }

    fn complete(
        &self,
        req: &CompletionRequest<'_>,
        sink: &mut dyn TextSink,
        cancel: &CancelToken,
    ) -> Result<String, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        lock(&self.prompts).push((req.system.to_owned(), req.user.to_owned()));
        for piece in self.reply.split_inclusive(' ') {
            sink.text(piece);
        }
        Ok(self.reply.clone())
    }
}

/// A multilingual speech model that detects the language itself.
pub fn stt_entry(id: &str, diarization: DiarizationSupport) -> ModelEntry<Stt> {
    ModelEntry {
        id: id.parse().expect("a valid model id"),
        display_name: id.to_owned(),
        caps: SttCaps {
            languages: LangSupport::Multilingual {
                auto_detect: true,
                tags: LangSet::Any,
            },
            diarization,
            max_input: None,
        },
        recommended: false,
        rank: 0,
        disk_bytes: None,
    }
}

/// A summary model for every language.
pub fn llm_entry(id: &str) -> ModelEntry<Llm> {
    ModelEntry {
        id: id.parse().expect("a valid model id"),
        display_name: id.to_owned(),
        caps: LlmCaps {
            context_tokens: 8_192,
            languages: LangSet::Any,
        },
        recommended: false,
        rank: 0,
        disk_bytes: None,
    }
}

/// Drives a future to completion on this thread.
pub fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        std::thread::park();
    }
}

/// Polls `condition` until it holds, and panics naming `what` after ten seconds.
pub fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let give_up = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !condition() {
        assert!(
            std::time::Instant::now() < give_up,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Drained, frame_channel};
    use crate::domain::ChannelLayout;
    use crate::ports::capture::MicSelection;

    fn spec(layout: ChannelLayout) -> CaptureSpec {
        CaptureSpec {
            layout,
            rate_hz: 48_000,
            mic: MicSelection::SystemDefault,
        }
    }

    #[test]
    fn manual_clock_moves_only_when_told() {
        let clock = ManualClock::new(Timestamp(1_000));
        assert_eq!((clock.now(), clock.monotonic_ms()), (Timestamp(1_000), 0));
        clock.advance_ms(250);
        assert_eq!((clock.now(), clock.monotonic_ms()), (Timestamp(1_250), 250));
        clock.set_wall(Timestamp(5));
        assert_eq!((clock.now(), clock.monotonic_ms()), (Timestamp(5), 250));
    }

    #[test]
    fn scripted_frames_reach_the_source_and_stop_releases_the_device() {
        let capture = FakeCapture::scripted(vec![vec![
            Step::Frames(vec![0.1, 0.2, 0.3, 0.4]),
            Step::Silence { frames: 2 },
            Step::Issue(CaptureIssue::SystemAudioSilent),
        ]]);
        let (sink, mut source) = frame_channel(ChannelLayout::MicSystem, 64);
        let handle = capture
            .start(&spec(ChannelLayout::MicSystem), sink)
            .unwrap();
        assert_eq!(capture.running(), 1);

        let mut out = Vec::new();
        let give_up = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut frames = 0;
        while frames < 4 && std::time::Instant::now() < give_up {
            frames += source.drain(&mut out).frames;
            std::thread::yield_now();
        }
        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0]);
        assert!(!source.is_finished(), "the device is still running");

        handle.stop();
        assert_eq!(capture.running(), 0);
        assert!(source.is_finished());
        assert_eq!(
            source.issues().collect::<Vec<_>>(),
            vec![CaptureIssue::SystemAudioSilent]
        );
    }

    #[test]
    fn each_start_takes_the_next_script_and_a_pause_resume_feeds_one_writer() {
        let capture = FakeCapture::scripted(vec![
            vec![Step::Frames(vec![1.0, 1.0])],
            vec![Step::Frames(vec![2.0, 2.0])],
        ]);
        let layout = ChannelLayout::MicOnly;
        let mut written: Vec<f32> = Vec::new();
        for _ in 0..3 {
            let (sink, mut source) = frame_channel(layout, 16);
            capture.start(&spec(layout), sink).unwrap().stop();
            let mut chunk = Vec::new();
            assert_eq!(source.drain(&mut chunk).padded_frames, 0);
            written.extend(chunk);
        }
        assert_eq!(written, vec![1.0, 1.0, 2.0, 2.0]);
        assert_eq!(capture.start_count(), 3);
        assert_eq!(capture.specs().len(), 3);
    }

    #[test]
    fn overflowing_the_ring_is_padded_like_a_real_adapter() {
        let capture = FakeCapture::scripted(vec![vec![Step::Frames(vec![0.5; 10])]]);
        let (sink, mut source) = frame_channel(ChannelLayout::MicOnly, 4);
        capture
            .start(&spec(ChannelLayout::MicOnly), sink)
            .unwrap()
            .stop();
        let mut out = Vec::new();
        assert_eq!(
            source.drain(&mut out),
            Drained {
                frames: 4,
                padded_frames: 6
            }
        );
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn start_can_be_made_to_fail_once() {
        let capture = FakeCapture::new();
        capture.fail_next_start(CaptureError::AccessDenied(Source::Microphone));
        let (sink, _source) = frame_channel(ChannelLayout::MicOnly, 4);
        assert!(matches!(
            capture.start(&spec(ChannelLayout::MicOnly), sink),
            Err(CaptureError::AccessDenied(Source::Microphone))
        ));
        assert_eq!((capture.start_count(), capture.running()), (0, 0));
        let (sink, _source) = frame_channel(ChannelLayout::MicOnly, 4);
        capture
            .start(&spec(ChannelLayout::MicOnly), sink)
            .unwrap()
            .stop();
        assert_eq!(capture.start_count(), 1);
    }

    #[test]
    fn probe_and_access_follow_the_configured_caps() {
        let capture = FakeCapture::new().with_caps(CaptureCaps {
            microphone: Access::Denied,
            system_audio: Access::Undetermined,
            unsupported: None,
        });
        assert_eq!(capture.probe().microphone, Access::Denied);
        assert_eq!(
            capture.request_access(Source::SystemAudio).unwrap(),
            Access::Undetermined
        );
        assert_eq!(capture.input_devices().unwrap().len(), 1);
    }

    #[test]
    fn dropping_a_handle_releases_the_device_too() {
        let capture = FakeCapture::new();
        let (sink, source) = frame_channel(ChannelLayout::MicOnly, 4);
        let handle = capture.start(&spec(ChannelLayout::MicOnly), sink).unwrap();
        drop(handle);
        assert_eq!(capture.running(), 0);
        assert!(source.is_finished());
    }
}

pub mod store;
