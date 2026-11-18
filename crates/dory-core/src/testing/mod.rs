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
    Channel, ChannelLayout, Fraction, LangSet, LangTag, LanguagePref, LevelMeter, Levels, Ms,
    RecordingId, TakeContainer, TakeInfo, TimeRange, Timestamp,
};
use crate::models::{
    Availability, Diar, DiarCaps, DiarizationSupport, LangSupport, Llm, LlmCaps, ModelEntry,
    ModelId, ModelKind, ProviderId, Stt, SttCaps,
};
use crate::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, CaptureHandle, CaptureIssue, CaptureSpec,
    InputDevice, Source,
};
use crate::ports::{
    Activity, CancelToken, Clip, Clock, CompletionRequest, Conditions, Diarizer, EngineError,
    LanguageModel, LegacyFile, Machine, ModelProvider, Moved, ProvisionError, ProvisionSink,
    RawSegment, SegmentSink, TakeError, TakeWriter, Takes, TextSink, TranscribeOutcome,
    TranscribeRequest, Transcriber, Trash, TrashError, TrashReceipt, Turn,
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

type SpeechByChannel = HashMap<(RecordingId, Channel), Vec<TimeRange>>;

/// A [`Takes`] that keeps samples in memory. A writer dropped without `finish` leaves its take
/// open, the way a crash leaves a WAV behind, so `orphans` lists it.
///
/// With [`MemTakes::in_dir`], each take also owns one real marker file, and the take exists only
/// while that file does. Moving the marker away, as a [`FakeTrash`] does, makes the take vanish,
/// and moving it back brings the samples back.
#[derive(Clone, Default)]
pub struct MemTakes {
    takes: Arc<Mutex<BTreeMap<RecordingId, MemTake>>>,
    activity: Arc<Mutex<SpeechByChannel>>,
    marker_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct MemTake {
    layout: ChannelLayout,
    rate_hz: u32,
    samples: Vec<f32>,
    sealed: bool,
    /// The old app's file a legacy pointer refers to. Never copied or touched.
    legacy: Option<PathBuf>,
}

impl MemTake {
    fn frames(&self) -> u64 {
        (self.samples.len() / self.layout.channel_count()) as u64
    }

    fn info(&self) -> TakeInfo {
        TakeInfo {
            container: if self.legacy.is_some() {
                TakeContainer::LegacyWebm
            } else if self.sealed {
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

    /// Like [`MemTakes::new`], with a marker file per take under `dir`, which must exist.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            marker_dir: Some(dir.into()),
            ..Self::default()
        }
    }

    /// Whole frames written to the take, or `None` if there is none.
    pub fn frames(&self, id: &RecordingId) -> Option<u64> {
        let takes = lock(&self.takes);
        self.live(&takes, id).map(MemTake::frames)
    }

    pub fn is_sealed(&self, id: &RecordingId) -> bool {
        let takes = lock(&self.takes);
        self.live(&takes, id).is_some_and(|t| t.sealed)
    }

    /// Makes `id` a legacy take: a pointer to the real file `original`, which is read in place and
    /// never listed by [`Takes::files`]. The take is mixed, `duration` long, and silent.
    pub fn link_legacy(
        &self,
        id: &RecordingId,
        original: &Path,
        duration: Ms,
    ) -> Result<TakeInfo, TakeError> {
        const RATE_HZ: u32 = 16_000;
        if !original.is_file() {
            return Err(TakeError::Missing(id.clone()));
        }
        let mut takes = lock(&self.takes);
        self.claim(&takes, id)?;
        let frames = usize::try_from(duration.0 * u64::from(RATE_HZ) / 1000).unwrap_or(0);
        let take = MemTake {
            layout: ChannelLayout::Mixed,
            rate_hz: RATE_HZ,
            samples: vec![0.0; frames],
            sealed: true,
            legacy: Some(original.to_owned()),
        };
        let info = take.info();
        takes.insert(id.clone(), take);
        Ok(info)
    }

    fn marker(&self, id: &RecordingId) -> Option<PathBuf> {
        self.marker_dir
            .as_ref()
            .map(|dir| dir.join(format!("{id}.take")))
    }

    fn live<'a>(
        &self,
        takes: &'a BTreeMap<RecordingId, MemTake>,
        id: &RecordingId,
    ) -> Option<&'a MemTake> {
        takes
            .get(id)
            .filter(|_| self.marker(id).is_none_or(|marker| marker.exists()))
    }

    fn claim(
        &self,
        takes: &BTreeMap<RecordingId, MemTake>,
        id: &RecordingId,
    ) -> Result<(), TakeError> {
        if self.live(takes, id).is_some() {
            return Err(TakeError::Exists(id.clone()));
        }
        if let Some(marker) = self.marker(id) {
            std::fs::write(&marker, id.as_str()).map_err(TakeError::Io)?;
        }
        Ok(())
    }

    /// Where someone talks on one channel, in take time, as a VAD would report it. Clips of a
    /// channel with none set report `Activity::Unmeasured`.
    pub fn set_activity(&self, id: &RecordingId, channel: Channel, spans: &[(u64, u64)]) {
        let spans = spans
            .iter()
            .map(|&(a, b)| TimeRange::new(Ms(a), Ms(b)).expect("start before end"))
            .collect();
        lock(&self.activity).insert((id.clone(), channel), spans);
    }

    /// Like [`MemTakes::with`], but a legacy take whose original is gone is missing, as it is for
    /// a reader that decodes the original in place.
    fn readable<T>(
        &self,
        id: &RecordingId,
        f: impl FnOnce(&mut MemTake) -> T,
    ) -> Result<T, TakeError> {
        self.with(id, |take| match &take.legacy {
            Some(original) if !original.is_file() => None,
            _ => Some(f(take)),
        })?
        .ok_or_else(|| TakeError::Missing(id.clone()))
    }

    fn with<T>(&self, id: &RecordingId, f: impl FnOnce(&mut MemTake) -> T) -> Result<T, TakeError> {
        let mut takes = lock(&self.takes);
        let live = self.live(&takes, id).is_some();
        takes
            .get_mut(id)
            .filter(|_| live)
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
        self.claim(&takes, id)?;
        takes.insert(
            id.clone(),
            MemTake {
                layout,
                rate_hz,
                samples: Vec::new(),
                sealed: false,
                legacy: None,
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
        let info = self.readable(id, |take| {
            take.layout
                .index_of(channel)
                .map(|_| take.info())
                .ok_or_else(|| TakeError::Decode(format!("no {channel:?} channel")))
        })??;
        let origin = span.map_or(Ms(0), |s| s.start());
        let duration = span.map_or(info.duration, |s| s.len());
        let end = Ms(origin.0 + duration.0);
        let activity = match lock(&self.activity).get(&(id.clone(), channel)) {
            None => Activity::Unmeasured,
            Some(spans) => Activity::Speech(
                spans
                    .iter()
                    .filter_map(|s| {
                        TimeRange::new(
                            Ms(s.start().max(origin).0 - origin.0),
                            Ms(s.end().min(end).0.saturating_sub(origin.0)),
                        )
                    })
                    .filter(|s| !s.is_empty())
                    .collect(),
            ),
        };
        Ok(Clip::new(
            PathBuf::from(format!("mem/{id}/{channel:?}")),
            channel,
            duration,
            origin,
            activity,
            None,
        ))
    }

    fn slice(&self, whole: &Clip, span: TimeRange) -> Result<Clip, TakeError> {
        let start = span.start().min(Ms(whole.origin.0 + whole.duration.0));
        let end = span.end().min(Ms(whole.origin.0 + whole.duration.0));
        let local = TimeRange::new(Ms(start.0 - whole.origin.0), Ms(end.0 - whole.origin.0))
            .ok_or_else(|| TakeError::Decode(format!("{span:?} is not a span")))?;
        Ok(Clip::new(
            whole.path.clone(),
            whole.channel,
            local.len(),
            start,
            whole.activity.within(local),
            None,
        ))
    }

    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError> {
        let takes = lock(&self.takes);
        Ok(takes
            .iter()
            .filter(|(id, take)| !take.sealed && self.live(&takes, id).is_some())
            .map(|(id, _)| id.clone())
            .collect())
    }

    fn stored(&self) -> Result<Vec<RecordingId>, TakeError> {
        let takes = lock(&self.takes);
        Ok(takes
            .keys()
            .filter(|id| self.live(&takes, id).is_some())
            .cloned()
            .collect())
    }

    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError> {
        let takes = lock(&self.takes);
        Ok(self
            .live(&takes, id)
            .filter(|take| {
                take.legacy
                    .as_ref()
                    .is_none_or(|original| original.is_file())
            })
            .map(MemTake::info))
    }

    fn legacy_original(&self, id: &RecordingId) -> Result<Option<LegacyFile>, TakeError> {
        let takes = lock(&self.takes);
        Ok(self
            .live(&takes, id)
            .and_then(|take| take.legacy.clone())
            .map(|path| LegacyFile {
                bytes: std::fs::metadata(&path)
                    .ok()
                    .filter(|meta| meta.is_file())
                    .map(|meta| meta.len()),
                path,
            }))
    }

    fn adopt(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        self.with(id, |take| match &take.legacy {
            Some(original) if !original.is_file() => Err(TakeError::Missing(id.clone())),
            _ => {
                take.legacy = None;
                Ok(take.info())
            }
        })?
    }

    fn import(&self, id: &RecordingId, _source: &Path) -> Result<TakeInfo, TakeError> {
        let mut takes = lock(&self.takes);
        self.claim(&takes, id)?;
        let take = MemTake {
            layout: ChannelLayout::Mixed,
            rate_hz: 16_000,
            samples: Vec::new(),
            sealed: true,
            legacy: None,
        };
        let info = take.info();
        takes.insert(id.clone(), take);
        Ok(info)
    }

    fn levels(&self, id: &RecordingId) -> Result<Levels, TakeError> {
        self.readable(id, |take| {
            let mut meter = LevelMeter::new(take.frames(), take.layout.channel_count());
            meter.feed(&take.samples);
            meter.finish()
        })
    }

    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        self.readable(id, |take| {
            take.legacy
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("mem/{id}")))
        })
    }

    fn delete(&self, id: &RecordingId) -> Result<(), TakeError> {
        lock(&self.takes).remove(id);
        if let Some(marker) = self.marker(id) {
            match std::fs::remove_file(marker) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(TakeError::Io(e)),
                _ => {}
            }
        }
        Ok(())
    }

    fn files(&self, id: &RecordingId) -> Result<Vec<PathBuf>, TakeError> {
        Ok(self
            .marker(id)
            .filter(|marker| marker.exists())
            .into_iter()
            .collect())
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

/// A [`Trash`] that moves files into a folder the test owns, and back. Each trashing gets its own
/// numbered subfolder, so two files with one name never collide.
pub struct FakeTrash {
    dir: PathBuf,
    next: AtomicU64,
    fail_move: AtomicBool,
}

impl FakeTrash {
    /// `dir` must exist.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            next: AtomicU64::new(0),
            fail_move: AtomicBool::new(false),
        }
    }

    /// Makes every later `move_to_trash` fail, as a trash that refuses the files would.
    pub fn fail_moves(&self, fail: bool) {
        self.fail_move.store(fail, Ordering::SeqCst);
    }

    /// Everything in the trash folder, however deep.
    pub fn trashed_files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.dir, &mut out);
        out.sort();
        out
    }
}

fn move_err(path: &Path, source: std::io::Error) -> TrashError {
    TrashError::Io {
        path: path.to_owned(),
        source,
    }
}

impl Trash for FakeTrash {
    fn move_to_trash(&self, paths: &[PathBuf]) -> Result<TrashReceipt, TrashError> {
        if self.fail_move.load(Ordering::SeqCst) {
            return Err(TrashError::Platform("the fake trash refuses".into()));
        }
        if paths.is_empty() {
            return Ok(TrashReceipt::default());
        }
        let slot = self
            .dir
            .join(self.next.fetch_add(1, Ordering::SeqCst).to_string());
        std::fs::create_dir_all(&slot).map_err(|e| move_err(&slot, e))?;
        let mut moved = Vec::new();
        for original in paths {
            let trashed = slot.join(original.file_name().unwrap_or_default());
            if let Err(e) = std::fs::rename(original, &trashed) {
                let _ = self.put_back(&TrashReceipt::new(moved));
                return Err(move_err(original, e));
            }
            moved.push(Moved {
                original: original.clone(),
                trashed,
            });
        }
        Ok(TrashReceipt::new(moved))
    }

    fn put_back(&self, receipt: &TrashReceipt) -> Result<(), TrashError> {
        for item in receipt.moved() {
            if !item.trashed.exists() {
                return Err(TrashError::Missing(item.trashed.clone()));
            }
            if item.original.exists() {
                return Err(TrashError::Occupied(item.original.clone()));
            }
        }
        for item in receipt.moved() {
            std::fs::rename(&item.trashed, &item.original)
                .map_err(|e| move_err(&item.original, e))?;
        }
        Ok(())
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

/// A machine whose conditions a test sets.
#[derive(Debug, Default)]
pub struct FakeMachine {
    conditions: Mutex<Conditions>,
}

impl FakeMachine {
    pub fn new(conditions: Conditions) -> Self {
        Self {
            conditions: Mutex::new(conditions),
        }
    }

    pub fn set(&self, conditions: Conditions) {
        *lock(&self.conditions) = conditions;
    }
}

impl Machine for FakeMachine {
    fn conditions(&self) -> Conditions {
        *lock(&self.conditions)
    }
}

/// A speech engine that returns scripted segments per channel. A call returns the segments that
/// start inside its clip, on the clip's time axis, the way an engine reads only the audio it gets.
pub struct FakeTranscriber {
    segments: HashMap<Channel, Vec<RawSegment>>,
    detected: Option<LangTag>,
    /// The language call `i` reports, which wins over `detected`.
    detected_per_call: Vec<LangTag>,
    /// A call on Auto keeps only the segments that start in the first half of its clip, the way
    /// Whisper keeps one language and drops a turn in the other.
    drops_second_half: bool,
    /// Calls from this index on wait until they are cancelled.
    hold_from: AtomicUsize,
    running: AtomicBool,
    calls: Mutex<Vec<TranscribeCall>>,
}

impl Default for FakeTranscriber {
    fn default() -> Self {
        Self {
            segments: HashMap::new(),
            detected: None,
            detected_per_call: Vec::new(),
            drops_second_half: false,
            hold_from: AtomicUsize::new(usize::MAX),
            running: AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
        }
    }
}

/// What one `transcribe` call asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscribeCall {
    pub channel: Channel,
    /// The clip on the take's time axis.
    pub span: TimeRange,
    pub language: LanguagePref,
    pub diarize: bool,
    pub threads: u8,
    pub sentence_times: bool,
    pub locale: Option<LangTag>,
}

impl FakeTranscriber {
    pub fn new() -> Self {
        Self::default()
    }

    /// `(start_ms, end_ms, engine speaker, text)` on the take's time axis, for clips of `channel`.
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

    /// Like [`Self::with_segments`], with each segment detected as `language`.
    pub fn with_segments_in(
        mut self,
        channel: Channel,
        language: &str,
        segments: &[(u64, u64, u16, &str)],
    ) -> Self {
        let language = LangTag::parse(language).expect("a valid language tag");
        self = self.with_segments(channel, segments);
        for segment in self
            .segments
            .entry(channel)
            .or_default()
            .iter_mut()
            .rev()
            .take(segments.len())
        {
            segment.language = Some(language.clone());
        }
        self
    }

    pub fn detecting(mut self, language: &str) -> Self {
        self.detected = Some(LangTag::parse(language).expect("a valid language tag"));
        self
    }

    /// Call `i` reports the `i`th language, the way a multilingual engine hears each window.
    pub fn detecting_each(mut self, languages: &[&str]) -> Self {
        self.detected_per_call = languages
            .iter()
            .map(|l| LangTag::parse(l).expect("a valid language tag"))
            .collect();
        self
    }

    /// A call on Auto drops the segments that start in the second half of its clip.
    pub fn dropping_second_half(mut self) -> Self {
        self.drops_second_half = true;
        self
    }

    /// Labels each channel's segments so far with `languages` in turn, as a model that hears a
    /// meeting switch languages would.
    pub fn speaking(mut self, languages: &[LangTag]) -> Self {
        if languages.is_empty() {
            return self;
        }
        for segments in self.segments.values_mut() {
            for (segment, language) in segments.iter_mut().zip(languages.iter().cycle()) {
                segment.language = Some(language.clone());
            }
        }
        self
    }

    /// While held, `transcribe` waits until it is cancelled.
    pub fn hold(&self, hold: bool) {
        self.hold_from(if hold { 0 } else { usize::MAX });
    }

    /// Holds the call with index `call`, counted over every call so far, and every later one.
    pub fn hold_from(&self, call: usize) {
        self.hold_from.store(call, Ordering::SeqCst);
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
        let origin = req.clip.origin;
        let end = Ms(origin.0 + req.clip.duration.0);
        let index = {
            let mut calls = lock(&self.calls);
            calls.push(TranscribeCall {
                channel: req.clip.channel,
                span: TimeRange::new(origin, end).expect("start before end"),
                language: req.language.clone(),
                diarize: req.diarize,
                threads: req.threads,
                sentence_times: req.sentence_times,
                locale: req.locale.clone(),
            });
            calls.len() - 1
        };
        self.running.store(true, Ordering::SeqCst);
        while index >= self.hold_from.load(Ordering::SeqCst) && !cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let heard_until = if self.drops_second_half && req.language == LanguagePref::Auto {
            Ms(origin.0 + req.clip.duration.0 / 2)
        } else {
            end
        };
        let result = if cancel.is_cancelled() {
            Err(EngineError::Cancelled)
        } else {
            let segments: Vec<RawSegment> = self
                .segments
                .get(&req.clip.channel)
                .into_iter()
                .flatten()
                .filter(|s| s.range.start() >= origin && s.range.start() < heard_until)
                .map(|s| RawSegment {
                    range: TimeRange::new(
                        Ms(s.range.start().0 - origin.0),
                        Ms(s.range.end().0 - origin.0),
                    )
                    .expect("start before end"),
                    ..s.clone()
                })
                .collect();
            let total = segments.len().max(1) as f32;
            for (i, segment) in segments.into_iter().enumerate() {
                sink.segment(segment);
                sink.progress(Fraction::new((i + 1) as f32 / total));
            }
            Ok(TranscribeOutcome {
                detected_language: self
                    .detected_per_call
                    .get(index)
                    .or(self.detected.as_ref())
                    .cloned(),
            })
        };
        self.running.store(false, Ordering::SeqCst);
        result
    }
}

type Reply = Box<dyn Fn(&CompletionRequest<'_>) -> String + Send + Sync>;

/// A summary model that answers every prompt with the same text, or with what `replying` picks,
/// and keeps the prompts. Tokens are words.
pub struct FakeLanguageModel {
    reply: Reply,
    /// Calls from this index on wait until they are cancelled.
    hold_from: AtomicUsize,
    prompts: Mutex<Vec<Completion>>,
    /// The first this many calls fail this way.
    refusal: Option<(Refusal, usize)>,
}

/// How a fake summary model turns a call away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `UnsupportedLanguage` with this tag, after streaming the first word of its reply, the way
    /// a model that gets partway into a meeting would.
    Language(&'static str),
    /// `InputRejected`, after streaming the first word of its reply.
    Input,
    /// `Busy`, at once.
    Busy,
}

/// One completion asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub system: String,
    pub user: String,
    pub max_output_tokens: u32,
}

impl FakeLanguageModel {
    pub fn new(reply: &str) -> Self {
        let reply = reply.to_owned();
        Self::replying(move |_| reply.clone())
    }

    pub fn replying(
        reply: impl Fn(&CompletionRequest<'_>) -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            reply: Box::new(reply),
            hold_from: AtomicUsize::new(usize::MAX),
            prompts: Mutex::new(Vec::new()),
            refusal: None,
        }
    }

    /// A model that answers each prompt with `answer(system, user)`.
    pub fn answering(answer: impl Fn(&str, &str) -> String + Send + Sync + 'static) -> Self {
        Self::replying(move |req| answer(req.system, req.user))
    }

    /// Holds the call with index `call`, counted over every call so far, and every later one,
    /// until it is cancelled.
    pub fn hold_from(&self, call: usize) {
        self.hold_from.store(call, Ordering::SeqCst);
    }

    /// Fails the first `calls` completions with `refusal`. Every call is kept in `prompts`.
    pub fn refusing(mut self, refusal: Refusal, calls: usize) -> Self {
        self.refusal = Some((refusal, calls));
        self
    }

    pub fn prompts(&self) -> Vec<Completion> {
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
        let index = {
            let mut prompts = lock(&self.prompts);
            prompts.push(Completion {
                system: req.system.to_owned(),
                user: req.user.to_owned(),
                max_output_tokens: req.max_output_tokens,
            });
            prompts.len() - 1
        };
        while index >= self.hold_from.load(Ordering::SeqCst) {
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let reply = (self.reply)(req);
        if let Some((refusal, calls)) = self.refusal
            && index < calls
        {
            let first = reply.split_inclusive(' ').next().unwrap_or_default();
            return Err(match refusal {
                Refusal::Busy => EngineError::Busy("the fake model is busy".into()),
                Refusal::Language(tag) => {
                    sink.text(first);
                    EngineError::UnsupportedLanguage(LangTag::parse(tag).expect("a valid tag"))
                }
                Refusal::Input => {
                    sink.text(first);
                    EngineError::InputRejected("the fake model refused".into())
                }
            });
        }
        for piece in reply.split_inclusive(' ') {
            sink.text(piece);
        }
        Ok(reply)
    }
}

/// A speaker separator that returns scripted turns and keeps the clips it was given.
pub struct FakeDiarizer {
    turns: Vec<Turn>,
    calls: Mutex<Vec<(Channel, TimeRange)>>,
}

pub type FakeDiarProvider = FakeProvider<Diar>;

impl FakeDiarizer {
    /// `(start_ms, end_ms, speaker)` relative to the clip.
    pub fn new(turns: &[(u64, u64, u16)]) -> Self {
        Self {
            turns: turns
                .iter()
                .map(|&(a, b, speaker)| Turn {
                    range: TimeRange::new(Ms(a), Ms(b)).expect("start before end"),
                    speaker,
                })
                .collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The channel and the take-time span of each clip diarized.
    pub fn calls(&self) -> Vec<(Channel, TimeRange)> {
        lock(&self.calls).clone()
    }
}

impl Diarizer for FakeDiarizer {
    fn diarize(
        &self,
        clip: &Clip,
        _progress: &mut dyn FnMut(Fraction),
        cancel: &CancelToken,
    ) -> Result<Vec<Turn>, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let span = TimeRange::new(clip.origin, Ms(clip.origin.0 + clip.duration.0))
            .expect("start before end");
        lock(&self.calls).push((clip.channel, span));
        Ok(self.turns.clone())
    }
}

/// A speaker separator for any number of speakers.
pub fn diar_entry(id: &str) -> ModelEntry<Diar> {
    ModelEntry {
        id: id.parse().expect("a valid model id"),
        display_name: id.to_owned(),
        about: Default::default(),
        caps: DiarCaps { max_speakers: None },
        recommended: false,
        rank: 0,
        disk_bytes: None,
        built_in: false,
    }
}

/// A multilingual speech model that detects the language itself.
pub fn stt_entry(id: &str, diarization: DiarizationSupport) -> ModelEntry<Stt> {
    ModelEntry {
        id: id.parse().expect("a valid model id"),
        display_name: id.to_owned(),
        about: Default::default(),
        caps: SttCaps {
            languages: LangSupport::Multilingual {
                auto_detect: true,
                tags: LangSet::Any,
            },
            diarization,
            max_input: None,
            window: None,
            sentence_times: false,
            lead_in: Ms(0),
        },
        recommended: false,
        rank: 0,
        disk_bytes: None,
        built_in: false,
    }
}

/// A summary model for every language.
pub fn llm_entry(id: &str) -> ModelEntry<Llm> {
    ModelEntry {
        id: id.parse().expect("a valid model id"),
        display_name: id.to_owned(),
        about: Default::default(),
        caps: LlmCaps {
            context_tokens: 8_192,
            languages: LangSet::Any,
        },
        recommended: false,
        rank: 0,
        disk_bytes: None,
        built_in: false,
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

pub mod assistants;
pub mod playback;
pub mod store;
pub use playback::FakePlayback;
