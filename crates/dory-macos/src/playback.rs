//! `AvPlayback`: plays a take through `AVAudioEngine` and one `AVAudioPlayerNode` on the system's
//! default output device, which it never changes.
//!
//! The file is read with `AVAudioFile` (which decodes FLAC) in quarter-second chunks, mixed to
//! one mono signal by the core's [`mix_down`] and fed to both ears. A few chunks are scheduled
//! ahead and each completion refills one, so memory holds about a second and a half of audio
//! whatever the take's length.
//!
//! A completion handler only sends its generation to the feeder thread. It must not schedule: a
//! handler that calls `scheduleBuffer` deadlocks against a concurrent `stop`, which holds the
//! node's lock while waiting for the handler queue.
//!
//! Seeking stops the node, which drops everything scheduled and resets its sample clock, then
//! schedules again from the target frame. Position is that start frame plus the node's sample
//! time. A generation counter makes the completions of dropped buffers do nothing.

use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;

use block2::RcBlock;
use dory_core::domain::Ms;
use dory_core::ports::{
    PlayState, Playback, PlaybackError, PlaybackSource, Player, PlayerStatus, channel_gains,
    mix_down,
};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioEngine, AVAudioFile, AVAudioFormat, AVAudioPCMBuffer, AVAudioPlayerNode,
    AVAudioPlayerNodeCompletionCallbackType,
};
use objc2_foundation::{NSError, NSString, NSURL};

const CHUNK_DIVISOR: u32 = 4;
const CHUNKS_AHEAD: u32 = 6;

#[derive(Default)]
pub struct AvPlayback;

impl AvPlayback {
    pub fn new() -> Self {
        Self
    }
}

impl Playback for AvPlayback {
    fn open(&self, source: &PlaybackSource) -> Result<Box<dyn Player>, PlaybackError> {
        Ok(Box::new(AvPlayer::open(source)?))
    }
}

/// An AVFAudio object that this module moves between threads.
struct Objc<T>(Retained<T>);

// SAFETY: every engine and node call is serialized by `Shared::playhead`, except `scheduleBuffer`
// from a completion handler and the file reads, which `Shared::reader` serializes. AVFAudio
// documents completion handlers as the place to schedule more buffers, and the file is touched
// only under `reader`.
unsafe impl<T> Send for Objc<T> {}
// SAFETY: see `Send` above. No `&T` is used outside those locks.
unsafe impl<T> Sync for Objc<T> {}

struct Reader {
    file: Objc<AVAudioFile>,
    read_buffer: Objc<AVAudioPCMBuffer>,
    next_frame: i64,
    outstanding: u32,
    eof: bool,
}

struct Playhead {
    playing: bool,
    start_frame: i64,
    /// Where the listener is while paused, or the last known position if the node cannot say.
    frozen: i64,
}

struct Shared {
    node: Objc<AVAudioPlayerNode>,
    out_format: Objc<AVAudioFormat>,
    reader: Mutex<Reader>,
    playhead: Mutex<Playhead>,
    gains: &'static [f32],
    rate_hz: u32,
    length: i64,
    chunk_frames: u32,
    /// Bumped by every reposition. A handler for an older generation does nothing.
    generation: AtomicU64,
    /// The generation whose last buffer finished playing, or 0.
    ended: AtomicU64,
    /// Completion handlers send their generation here. `QUIT` ends the feeder.
    wake: Sender<u64>,
}

const QUIT: u64 = 0;

struct AvPlayer {
    engine: Objc<AVAudioEngine>,
    shared: Arc<Shared>,
    feeder: Option<JoinHandle<()>>,
}

fn feed(shared: Weak<Shared>, wake: Receiver<u64>) {
    while let Ok(generation) = wake.recv() {
        if generation == QUIT {
            return;
        }
        let Some(shared) = shared.upgrade() else {
            return;
        };
        shared.on_played(generation);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

impl AvPlayer {
    fn open(source: &PlaybackSource) -> Result<Self, PlaybackError> {
        let gains = channel_gains(source.layout);
        let (file, length, rate_hz, channels) = open_file(&source.path)?;
        if channels != gains.len() {
            return Err(PlaybackError::Open(format!(
                "the file has {channels} channel(s), a {:?} take has {}",
                source.layout,
                gains.len()
            )));
        }
        // SAFETY: allocating and initializing an AVAudioFormat with a valid rate and 2 channels.
        let out_format = unsafe {
            AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                f64::from(rate_hz),
                2,
            )
        }
        .ok_or_else(|| PlaybackError::Open(format!("no stereo format at {rate_hz} Hz")))?;
        let chunk_frames = (rate_hz / CHUNK_DIVISOR).max(1);
        // SAFETY: the processing format belongs to the file and is valid PCM.
        let read_buffer = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &file.processingFormat(),
                chunk_frames,
            )
        }
        .ok_or_else(|| PlaybackError::Open("cannot allocate a read buffer".into()))?;

        // SAFETY: plain construction, attach and connect of nodes that are not yet running.
        // `mainMixerNode` is created on first use and connects to the default output.
        let (engine, node) = unsafe {
            let engine = AVAudioEngine::new();
            let node = AVAudioPlayerNode::new();
            engine.attachNode(&node);
            engine.connect_to_format(&node, &engine.mainMixerNode(), Some(&out_format));
            engine.prepare();
            (engine, node)
        };
        let (wake, woken) = mpsc::channel();
        let shared = Arc::new(Shared {
            wake,
            node: Objc(node),
            out_format: Objc(out_format),
            reader: Mutex::new(Reader {
                file: Objc(file),
                read_buffer: Objc(read_buffer),
                next_frame: 0,
                outstanding: 0,
                eof: false,
            }),
            playhead: Mutex::new(Playhead {
                playing: false,
                start_frame: 0,
                frozen: 0,
            }),
            gains,
            rate_hz,
            length,
            chunk_frames,
            generation: AtomicU64::new(0),
            ended: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&shared);
        let feeder = std::thread::Builder::new()
            .name("dory-playback-feed".into())
            .spawn(move || feed(weak, woken))
            .map_err(|e| PlaybackError::Open(e.to_string()))?;
        let player = AvPlayer {
            engine: Objc(engine),
            shared,
            feeder: Some(feeder),
        };
        player.shared.reposition(0);
        Ok(player)
    }

    fn frames_of(&self, ms: Ms) -> i64 {
        let frames = u128::from(ms.0) * u128::from(self.shared.rate_hz) / 1000;
        i64::try_from(frames)
            .unwrap_or(i64::MAX)
            .min(self.shared.length)
    }
}

fn open_file(path: &Path) -> Result<(Retained<AVAudioFile>, i64, u32, usize), PlaybackError> {
    let path = path
        .to_str()
        .ok_or_else(|| PlaybackError::Open("the path is not UTF-8".into()))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // SAFETY: `url` is a valid file URL, kept alive for the call.
    let file = unsafe { AVAudioFile::initForReading_error(AVAudioFile::alloc(), &url) }
        .map_err(|e| PlaybackError::Open(describe(&e)))?;
    // SAFETY: plain property reads on a file that opened.
    let (length, rate, channels) = unsafe {
        let format = file.processingFormat();
        (file.length(), format.sampleRate(), format.channelCount())
    };
    if length <= 0 || !(1.0..=768_000.0).contains(&rate) {
        return Err(PlaybackError::Open("the file holds no audio".into()));
    }
    Ok((file, length, rate as u32, channels as usize))
}

impl Shared {
    /// Drops everything scheduled and schedules again from `frame`. The caller holds `playhead`
    /// or has not published the player yet, and starts the node afterwards if it should play.
    fn reposition(self: &Arc<Self>, frame: i64) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        // Taken before stopping, so a refill in flight finishes first and cannot schedule a stale
        // buffer after the stop.
        let mut reader = lock(&self.reader);
        // SAFETY: the node is attached to an engine. Stopping needs the engine in no state.
        unsafe { self.node.0.stop() };
        // SAFETY: the frame is within the file, and no read is in flight under the lock.
        unsafe { reader.file.0.setFramePosition(frame) };
        reader.next_frame = frame;
        reader.outstanding = 0;
        reader.eof = false;
        self.refill(&mut reader, generation);
    }

    /// Schedules chunks until `CHUNKS_AHEAD` are queued or the file ends.
    fn refill(self: &Arc<Self>, reader: &mut Reader, generation: u64) {
        while reader.outstanding < CHUNKS_AHEAD && !reader.eof {
            let frames = i64::from(self.chunk_frames).min(self.length - reader.next_frame);
            if frames <= 0 || !self.schedule_chunk(reader, frames as u32, generation) {
                reader.eof = true;
                break;
            }
            reader.next_frame += frames;
            reader.outstanding += 1;
            reader.eof = reader.next_frame >= self.length;
        }
        if reader.eof && reader.outstanding == 0 {
            self.ended.store(generation, Ordering::Release);
        }
    }

    /// Reads, mixes and schedules `frames` frames. False if the read or allocation failed, which
    /// ends the take early.
    fn schedule_chunk(self: &Arc<Self>, reader: &Reader, frames: u32, generation: u64) -> bool {
        // SAFETY: the buffer has the file's processing format and capacity for a chunk.
        let read = unsafe {
            reader
                .file
                .0
                .readIntoBuffer_frameCount_error(&reader.read_buffer.0, frames)
        };
        if read.is_err() {
            return false;
        }
        // SAFETY: the stereo format is valid PCM.
        let out = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &self.out_format.0,
                frames,
            )
        };
        let Some(out) = out else { return false };
        // SAFETY: the read buffer holds `frame_length` valid samples per channel, `channels` of
        // them as the file's deinterleaved float data. The output buffer is new, stereo, and has
        // room for `frames` samples per channel. The slices do not outlive this block.
        unsafe {
            let got = reader.read_buffer.0.frameLength() as usize;
            let inputs = reader.read_buffer.0.floatChannelData();
            if inputs.is_null() || got == 0 {
                return false;
            }
            let channels: Vec<&[f32]> = (0..self.gains.len())
                .map(|c| std::slice::from_raw_parts((*inputs.add(c)).as_ptr(), got))
                .collect();
            out.setFrameLength(got as u32);
            let outputs = out.floatChannelData();
            if outputs.is_null() {
                return false;
            }
            let left = std::slice::from_raw_parts_mut((*outputs).as_ptr(), got);
            mix_down(self.gains, &channels, left);
            let right: NonNull<f32> = *outputs.add(1);
            std::ptr::copy_nonoverlapping(left.as_ptr(), right.as_ptr(), got);
        }
        let wake = self.wake.clone();
        let block = RcBlock::new(move |_: AVAudioPlayerNodeCompletionCallbackType| {
            let _ = wake.send(generation);
        });
        // SAFETY: the node is attached, the buffer's format matches the connection, and the block
        // is copied by AVFAudio before this call returns.
        unsafe {
            self.node
                .0
                .scheduleBuffer_completionCallbackType_completionHandler(
                    &out,
                    AVAudioPlayerNodeCompletionCallbackType::DataPlayedBack,
                    RcBlock::as_ptr(&block),
                );
        }
        true
    }

    fn on_played(self: &Arc<Self>, generation: u64) {
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let mut reader = lock(&self.reader);
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        reader.outstanding = reader.outstanding.saturating_sub(1);
        self.refill(&mut reader, generation);
    }

    fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire) == self.generation.load(Ordering::Acquire)
    }

    /// The listener's frame. Needs `playhead` held, so the node is not being restarted.
    fn frame_now(&self, playhead: &Playhead) -> i64 {
        if self.is_ended() {
            return self.length;
        }
        if !playhead.playing {
            return playhead.frozen;
        }
        // SAFETY: reads of the node's render clock, which is nil until it has rendered.
        let played = unsafe {
            self.node
                .0
                .lastRenderTime()
                .and_then(|render| self.node.0.playerTimeForNodeTime(&render))
                .filter(|time| time.isSampleTimeValid())
                .map(|time| time.sampleTime())
        };
        match played {
            Some(samples) => (playhead.start_frame + samples.max(0)).min(self.length),
            None => playhead.frozen,
        }
    }
}

impl Player for AvPlayer {
    fn play(&self) -> Result<(), PlaybackError> {
        let shared = &self.shared;
        let mut playhead = lock(&shared.playhead);
        if shared.is_ended() {
            shared.reposition(0);
            playhead.start_frame = 0;
            playhead.frozen = 0;
            playhead.playing = false;
        }
        if playhead.playing {
            return Ok(());
        }
        // SAFETY: the engine is configured; a started engine is required before the node plays.
        unsafe {
            if !self.engine.0.isRunning() {
                self.engine
                    .0
                    .startAndReturnError()
                    .map_err(|e| PlaybackError::Output(describe(&e)))?;
            }
            shared.node.0.play();
        }
        playhead.playing = true;
        Ok(())
    }

    fn pause(&self) {
        let shared = &self.shared;
        let mut playhead = lock(&shared.playhead);
        if !playhead.playing {
            return;
        }
        playhead.frozen = shared.frame_now(&playhead);
        playhead.playing = false;
        // SAFETY: the node is attached; pausing keeps its sample clock and queue.
        unsafe { shared.node.0.pause() };
    }

    fn seek(&self, to: Ms) -> Result<(), PlaybackError> {
        let shared = &self.shared;
        let frame = self.frames_of(to);
        let mut playhead = lock(&shared.playhead);
        let resume = playhead.playing && !shared.is_ended();
        shared.reposition(frame);
        playhead.start_frame = frame;
        playhead.frozen = frame;
        playhead.playing = false;
        if resume && !shared.is_ended() {
            // SAFETY: the engine runs, since the node was playing.
            unsafe { shared.node.0.play() };
            playhead.playing = true;
        }
        Ok(())
    }

    fn status(&self) -> PlayerStatus {
        let shared = &self.shared;
        let playhead = lock(&shared.playhead);
        let frame = shared.frame_now(&playhead);
        let state = match (shared.is_ended(), playhead.playing) {
            (true, _) => PlayState::Ended,
            (false, true) => PlayState::Playing,
            (false, false) => PlayState::Paused,
        };
        let rate = std::num::NonZeroU32::new(shared.rate_hz).unwrap_or(std::num::NonZeroU32::MIN);
        PlayerStatus {
            state,
            position: Ms::from_frames(frame.max(0) as u64, rate),
        }
    }
}

impl Drop for AvPlayer {
    fn drop(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        // SAFETY: stopping the node and engine releases the buffers and the handlers' blocks.
        unsafe {
            self.shared.node.0.stop();
            self.engine.0.stop();
        }
        let _ = self.shared.wake.send(QUIT);
        if let Some(feeder) = self.feeder.take() {
            let _ = feeder.join();
        }
    }
}
