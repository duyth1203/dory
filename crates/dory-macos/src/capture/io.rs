//! The real-time half of the adapter: what the IOProc does each cycle, and the cell that hands its
//! state between the HAL's IO thread and the thread that tears the capture down.

use std::cell::UnsafeCell;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use block2::RcBlock;
use dory_core::capture::FrameSink;
use dory_core::domain::{Channel, ChannelLayout};
use dory_core::ports::capture::CaptureIssue;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};

use super::SYSTEM_AUDIO_HEARD;
use super::dsp::{self, BufferShape, InputMap, LinearResampler, MAX_BUFFERS, Presence, SilenceRun};
use super::hal::IoBlockFn;

/// Frames rendered per inner step. Fixes the scratch size independent of the device buffer size.
pub const CHUNK_FRAMES: usize = 1024;
/// Longest gap a rebuild fills. The supervisor gives up long before this (stall limit plus retry
/// window is under 10 s), so a longer reading means a bad host time, and filling it would stall
/// the IO thread.
const MAX_GAP_SECONDS: f64 = 30.0;

/// Counters the IO thread publishes for the monitor thread.
#[derive(Default)]
pub struct Shared {
    /// Consecutive digitally silent system frames, at the device rate.
    pub silent_frames: AtomicU64,
    /// IO cycles run so far. The watchdog reports a stall when it stops moving.
    pub cycles: AtomicU64,
}

/// Host-time span of every frame pushed into one sink, across graphs. Zero means not yet.
#[derive(Debug, Default)]
pub struct Span {
    pub first: AtomicU64,
    pub end: AtomicU64,
    /// Silent frames pushed for input the HAL skipped inside one graph.
    pub skipped: AtomicU64,
}

/// When a cycle's first input frame was captured, as far as the HAL says.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stamp {
    pub host: Option<u64>,
    /// The device's sample time. It only means something within one graph.
    pub sample: Option<f64>,
}

impl Stamp {
    fn read(time: &AudioTimeStamp) -> Self {
        let has = |flag| time.mFlags.contains(flag);
        Self {
            host: has(AudioTimeStampFlags::HostTimeValid).then_some(time.mHostTime),
            sample: has(AudioTimeStampFlags::SampleTimeValid).then_some(time.mSampleTime),
        }
    }
}

/// What outlives one device graph: the sink and where the timeline stands. It has exactly one
/// owner at a time, the running graph's `IoState` or the supervisor between graphs.
pub struct Timeline {
    sink: FrameSink,
    /// Host time just past the last frame pushed. `None` until a cycle with a valid host time.
    end: Option<u64>,
    /// Set when the timeline moves into a rebuilt graph. That graph's first cycle fills the gap
    /// since `end` with silence and reports `Rebuilt`.
    resumed: bool,
    mic_present: bool,
    system_present: bool,
    span: Arc<Span>,
}

impl Timeline {
    pub fn new(sink: FrameSink, span: Arc<Span>) -> Self {
        Self {
            sink,
            end: None,
            resumed: false,
            mic_present: true,
            system_present: true,
            span,
        }
    }

    /// Marks the next graph this timeline moves into as a rebuild.
    pub fn resume(&mut self) {
        self.resumed = true;
    }

    pub fn report(&mut self, issue: CaptureIssue) {
        self.sink.report(issue);
    }
}

/// The clocks one graph runs on.
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    pub device_hz: f64,
    pub out_hz: f64,
    /// Host time ticks per second.
    pub host_hz: f64,
}

/// Output frames that fit between host times `end` and `start`. Zero when they overlap.
pub fn gap_frames(end: u64, start: u64, clocks: Clocks) -> u64 {
    let ticks = start.saturating_sub(end) as f64;
    let seconds = (ticks / clocks.host_hz).min(MAX_GAP_SECONDS);
    (seconds * clocks.out_hz).round() as u64
}

/// Output frames for the device frames between the expected and the actual start of a cycle.
/// Zero when the sample time did not move ahead, as after the HAL resets it.
pub fn skipped_frames(expected: f64, sample: f64, clocks: Clocks) -> u64 {
    let device_frames = (sample - expected).min(MAX_GAP_SECONDS * clocks.device_hz);
    if device_frames < 1.0 {
        return 0;
    }
    (device_frames * clocks.out_hz / clocks.device_hz).round() as u64
}

/// Everything the IOProc touches. Preallocated at start, so a cycle never allocates.
pub struct IoState {
    timeline: Timeline,
    layout: ChannelLayout,
    map: InputMap,
    shapes: Vec<BufferShape>,
    scratch: Box<[f32]>,
    resampled: Box<[f32]>,
    resampler: Option<LinearResampler>,
    clocks: Clocks,
    /// Frames in the last cycle, used when no buffer in a cycle is valid.
    cycle_frames: usize,
    /// The sample time the next cycle should start at, if the input is contiguous.
    next_sample: Option<f64>,
    silence: SilenceRun,
    shared: Arc<Shared>,
}

impl IoState {
    /// `buffer_frames` is the device's IO buffer size, the cycle length until a cycle shows one.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timeline: Timeline,
        layout: ChannelLayout,
        map: InputMap,
        shapes: Vec<BufferShape>,
        resampler: Option<LinearResampler>,
        clocks: Clocks,
        buffer_frames: usize,
        shared: Arc<Shared>,
    ) -> Self {
        let channels = layout.channel_count();
        let out_frames = resampler.as_ref().map_or(0, |r| r.max_output(CHUNK_FRAMES));
        Self {
            timeline,
            layout,
            map,
            shapes,
            scratch: vec![0.0; CHUNK_FRAMES * channels].into_boxed_slice(),
            resampled: vec![0.0; out_frames * channels].into_boxed_slice(),
            resampler,
            clocks,
            cycle_frames: buffer_frames,
            next_sample: None,
            silence: SilenceRun::default(),
            shared,
        }
    }

    pub fn into_timeline(self) -> Timeline {
        self.timeline
    }

    /// Runs one IO cycle. `stamp` is the input time stamp of the cycle's first frame.
    ///
    /// # Safety
    /// `list` must point to the input AudioBufferList the HAL passed to this cycle: a header
    /// followed by `mNumberBuffers` buffers, each with `mDataByteSize` readable bytes at `mData`
    /// when `mData` is not null.
    pub unsafe fn run_cycle(&mut self, list: NonNull<AudioBufferList>, stamp: Stamp) {
        self.shared.cycles.fetch_add(1, Ordering::Relaxed);
        let mut views: [Option<&[f32]>; MAX_BUFFERS] = [None; MAX_BUFFERS];
        // SAFETY: forwarded from this function's contract.
        unsafe { view_inputs(list, &self.shapes, &mut views) };
        self.process(&views, stamp);
    }

    fn process(&mut self, views: &[Option<&[f32]>], stamp: Stamp) {
        self.fill_gaps(stamp);
        let frames = dsp::cycle_frames(views, &self.shapes).unwrap_or(self.cycle_frames);
        self.cycle_frames = frames;
        self.next_sample = stamp.sample.map(|s| s + frames as f64);
        let presence = self.map.presence(views, frames);
        let channels = self.layout.channel_count();
        let mut system_zero = true;
        let mut start = 0;
        while start < frames {
            let len = (frames - start).min(CHUNK_FRAMES);
            let Some(scratch) = self.scratch.get_mut(..len * channels) else {
                return;
            };
            system_zero &= dsp::render(&self.map, self.layout, views, presence, start, scratch);
            match &mut self.resampler {
                None => self.timeline.sink.push(scratch),
                Some(r) => {
                    let n = r.process(scratch, &mut self.resampled);
                    if let Some(out) = self.resampled.get(..n * channels) {
                        self.timeline.sink.push(out);
                    }
                }
            }
            start += len;
        }
        self.advance(stamp.host, frames);
        self.track(presence, system_zero, frames);
    }

    /// Keeps output time equal to elapsed time across a rebuild, measured in host time because
    /// the new device's sample time starts over, and across input the HAL skipped inside this
    /// graph, measured in sample time. A new graph's second cycle can start a few hundred frames
    /// past its first one's end (observed on macOS 26.5).
    fn fill_gaps(&mut self, stamp: Stamp) {
        if self.timeline.resumed {
            self.timeline.resumed = false;
            let gap = match (self.timeline.end, stamp.host) {
                (Some(end), Some(start)) => gap_frames(end, start, self.clocks),
                _ => 0,
            };
            self.push_silence(gap);
            self.timeline
                .sink
                .report(CaptureIssue::Rebuilt { gap_frames: gap });
        } else if let (Some(expected), Some(sample)) = (self.next_sample, stamp.sample) {
            let skipped = skipped_frames(expected, sample, self.clocks);
            if skipped > 0 {
                self.push_silence(skipped);
                self.timeline
                    .span
                    .skipped
                    .fetch_add(skipped, Ordering::Relaxed);
            }
        }
    }

    /// Pushes `frames` of silence in scratch-sized chunks.
    fn push_silence(&mut self, mut frames: u64) {
        self.scratch.fill(0.0);
        let channels = self.layout.channel_count().max(1);
        while frames > 0 {
            let len = frames.min(CHUNK_FRAMES as u64) as usize;
            let Some(zeros) = self.scratch.get(..len * channels) else {
                return;
            };
            self.timeline.sink.push(zeros);
            frames -= len as u64;
        }
    }

    /// Moves the timeline's end past this cycle's frames.
    fn advance(&mut self, host: Option<u64>, frames: usize) {
        let ticks = (frames as f64 * self.clocks.host_hz / self.clocks.device_hz).round() as u64;
        let Some(end) = host.or(self.timeline.end).map(|t| t.saturating_add(ticks)) else {
            return;
        };
        if let Some(start) = host {
            let _ = self.timeline.span.first.compare_exchange(
                0,
                start,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        self.timeline.end = Some(end);
        self.timeline.span.end.store(end, Ordering::Relaxed);
    }

    fn track(&mut self, presence: Presence, system_zero: bool, frames: usize) {
        let t = &mut self.timeline;
        if self.layout.index_of(Channel::Mic).is_some() && presence.mic != t.mic_present {
            t.mic_present = presence.mic;
            t.sink.report(if presence.mic {
                CaptureIssue::SourceRestored(Channel::Mic)
            } else {
                CaptureIssue::SourceLost(Channel::Mic)
            });
        }
        if self.layout.index_of(Channel::System).is_some() {
            if presence.system != t.system_present {
                t.system_present = presence.system;
                t.sink.report(if presence.system {
                    CaptureIssue::SourceRestored(Channel::System)
                } else {
                    CaptureIssue::SourceLost(Channel::System)
                });
            }
            let run = self.silence.observe(frames, system_zero);
            self.shared.silent_frames.store(run, Ordering::Relaxed);
            if !system_zero {
                SYSTEM_AUDIO_HEARD.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Fills `views` with the samples of each buffer that matches its expected shape: not null,
/// aligned, the expected channel count, and whole frames. Every other buffer is `None`. A list
/// with a different buffer count means the aggregate changed under us, so every buffer is `None`.
///
/// # Safety
/// As for [`IoState::run_cycle`]. The views must not outlive the cycle.
unsafe fn view_inputs(
    list: NonNull<AudioBufferList>,
    shapes: &[BufferShape],
    views: &mut [Option<&[f32]>; MAX_BUFFERS],
) {
    let list = list.as_ptr();
    // SAFETY: `list` points to a valid header. Reading through the raw pointer keeps the
    // provenance of the whole list, which the trailing buffers need.
    let count = unsafe { std::ptr::addr_of!((*list).mNumberBuffers).read() } as usize;
    if count != shapes.len() {
        return;
    }
    // SAFETY: as above, the buffer array starts at `mBuffers`.
    let first = unsafe { std::ptr::addr_of!((*list).mBuffers) }.cast::<AudioBuffer>();
    for (i, (shape, view)) in shapes.iter().zip(views.iter_mut()).enumerate() {
        // SAFETY: `i < count`, so buffer `i` is inside the list.
        let buffer = unsafe { first.add(i).read() };
        // SAFETY: the HAL hands us `mDataByteSize` readable bytes at `mData`.
        *view = unsafe { view_buffer(&buffer, *shape) };
    }
}

/// # Safety
/// When `buffer.mData` is not null it must point to `mDataByteSize` readable bytes.
unsafe fn view_buffer<'a>(buffer: &AudioBuffer, shape: BufferShape) -> Option<&'a [f32]> {
    let data = buffer.mData.cast::<f32>();
    let frame_bytes = shape.channels * size_of::<f32>();
    let bytes = buffer.mDataByteSize as usize;
    let fits = !data.is_null()
        && data.is_aligned()
        && buffer.mNumberChannels as usize == shape.channels
        && bytes.checked_rem(frame_bytes) == Some(0);
    // SAFETY: checked non-null and aligned; length is within the bytes the caller vouches for.
    fits.then(|| unsafe { std::slice::from_raw_parts(data, bytes / size_of::<f32>()) })
}

const IDLE: u8 = 0;
const BUSY: u8 = 1;
const CLOSED: u8 = 2;

/// The IOProc state, shared by the HAL's IO thread and the handle. The IO thread uses it only
/// while it holds the `BUSY` flag. `close` waits out a running cycle and leaves the cell `CLOSED`
/// for good, so a callback that arrives after teardown, even one the HAL should never make, finds
/// nothing to touch.
pub struct IoCell {
    flag: AtomicU8,
    state: UnsafeCell<Option<IoState>>,
}

// SAFETY: `state` is only reached by the holder of the `BUSY` flag or by the one `close` call that
// moved the flag to `CLOSED`, and never after that.
unsafe impl Sync for IoCell {}

impl IoCell {
    pub fn new(state: IoState) -> Self {
        Self {
            flag: AtomicU8::new(IDLE),
            state: UnsafeCell::new(Some(state)),
        }
    }

    /// Runs `f` on the state unless the cell is closed. Never blocks.
    fn with(&self, f: impl FnOnce(&mut IoState)) {
        if self
            .flag
            .compare_exchange(IDLE, BUSY, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        // SAFETY: this thread holds `BUSY`, so it has the only access.
        if let Some(state) = unsafe { (*self.state.get()).as_mut() } {
            f(state);
        }
        self.flag.store(IDLE, Ordering::Release);
    }

    /// Closes the cell for good and returns the state, waiting for a running cycle to finish.
    pub fn close(&self) -> Option<IoState> {
        loop {
            match self
                .flag
                .compare_exchange(IDLE, CLOSED, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(CLOSED) => return None,
                Err(_) => std::thread::yield_now(),
            }
        }
        // SAFETY: this call moved the flag to `CLOSED`, so no other access can follow.
        unsafe { (*self.state.get()).take() }
    }
}

/// The IOProc body. It holds only the cell.
pub fn io_block(cell: Arc<IoCell>) -> RcBlock<IoBlockFn> {
    RcBlock::new(
        move |_now: NonNull<AudioTimeStamp>,
              input: NonNull<AudioBufferList>,
              input_time: NonNull<AudioTimeStamp>,
              _output: NonNull<AudioBufferList>,
              _output_time: NonNull<AudioTimeStamp>| {
            // SAFETY: the HAL passes this cycle's input time stamp.
            let stamp = Stamp::read(unsafe { input_time.as_ref() });
            // SAFETY: the HAL passes this cycle's input list.
            cell.with(|state| unsafe { state.run_cycle(input, stamp) });
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dory_core::capture::{FrameSource, frame_channel};
    use std::mem::offset_of;

    use crate::capture::dsp::{Role, Segment};

    /// One buffer of a hand-built input list.
    struct Buf {
        channels: u32,
        data: Option<Vec<f32>>,
        bytes: Option<u32>,
    }

    fn interleaved(channels: u32, frames: usize, value: f32) -> Buf {
        Buf {
            channels,
            data: Some(vec![value; frames * channels as usize]),
            bytes: None,
        }
    }

    fn null(channels: u32, bytes: u32) -> Buf {
        Buf {
            channels,
            data: None,
            bytes: Some(bytes),
        }
    }

    /// An AudioBufferList with `bufs.len()` trailing buffers, laid out as the HAL passes it.
    struct List {
        words: Vec<u64>,
        _data: Vec<Vec<f32>>,
    }

    impl List {
        fn new(bufs: Vec<Buf>) -> Self {
            let n = bufs.len().max(1);
            let size = offset_of!(AudioBufferList, mBuffers) + n * size_of::<AudioBuffer>();
            let mut words = vec![0u64; size.div_ceil(8)];
            let list = words.as_mut_ptr().cast::<AudioBufferList>();
            let mut keep = Vec::new();
            // SAFETY: `words` is 8-aligned and large enough for the header and `n` buffers.
            unsafe {
                std::ptr::addr_of_mut!((*list).mNumberBuffers).write(bufs.len() as u32);
                let first = std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>();
                for (i, b) in bufs.into_iter().enumerate() {
                    let (ptr, len) = match b.data {
                        Some(mut d) => {
                            let p = d.as_mut_ptr().cast();
                            let len = d.len() as u32 * 4;
                            keep.push(d);
                            (p, len)
                        }
                        None => (std::ptr::null_mut(), 0),
                    };
                    first.add(i).write(AudioBuffer {
                        mNumberChannels: b.channels,
                        mDataByteSize: b.bytes.unwrap_or(len),
                        mData: ptr,
                    });
                }
            }
            Self { words, _data: keep }
        }

        fn ptr(&mut self) -> NonNull<AudioBufferList> {
            NonNull::new(self.words.as_mut_ptr().cast()).unwrap()
        }
    }

    /// 48 kHz device and output, and a host clock of 24 MHz, as on Apple silicon.
    const CLOCKS: Clocks = Clocks {
        device_hz: 48_000.0,
        out_hz: 48_000.0,
        host_hz: 24_000_000.0,
    };
    /// Host ticks per 48 kHz frame.
    const TICKS: u64 = 500;

    fn mic_system(mic_channels: usize) -> (IoState, FrameSource) {
        let (sink, source) = frame_channel(ChannelLayout::MicSystem, 48_000);
        let timeline = Timeline::new(sink, Arc::default());
        (graph(timeline, Some(mic_channels)), source)
    }

    /// A `[mic, system]` graph state over `timeline`. `mic_channels: None` is a mic-less graph:
    /// the tap only, as after a pinned mic disappears.
    fn graph(timeline: Timeline, mic_channels: Option<usize>) -> IoState {
        let mut shapes = Vec::new();
        let mut parts = Vec::new();
        if let Some(channels) = mic_channels {
            shapes.push(BufferShape { channels });
            parts.push(Segment {
                role: Role::Mic,
                buffers: vec![BufferShape { channels }],
            });
        }
        shapes.push(BufferShape { channels: 2 });
        parts.push(Segment {
            role: Role::System,
            buffers: vec![BufferShape { channels: 2 }],
        });
        let map = dsp::resolve_inputs(&shapes, &parts).unwrap();
        IoState::new(
            timeline,
            ChannelLayout::MicSystem,
            map,
            shapes,
            None,
            CLOCKS,
            512,
            Arc::default(),
        )
    }

    fn cycle(state: &mut IoState, list: &mut List) {
        cycle_at(state, list, None);
    }

    fn cycle_at(state: &mut IoState, list: &mut List, host: Option<u64>) {
        stamped(state, list, Stamp { host, sample: None });
    }

    fn stamped(state: &mut IoState, list: &mut List, stamp: Stamp) {
        // SAFETY: `list` is a well-formed input list that outlives the call.
        unsafe { state.run_cycle(list.ptr(), stamp) }
    }

    /// Frames pushed so far as `(mic, system)` pairs, and the issues reported.
    fn drain(source: &mut FrameSource) -> (Vec<(f32, f32)>, Vec<CaptureIssue>) {
        let mut out = Vec::new();
        source.drain(&mut out);
        let frames = out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[m, s]| (m, s))
            .collect();
        (frames, source.issues().collect())
    }

    #[test]
    fn a_mic_buffer_with_no_bytes_keeps_the_timeline_and_reports_the_mic_lost() {
        let (mut state, mut source) = mic_system(1);
        let mut list = List::new(vec![interleaved(1, 0, 0.0), interleaved(2, 512, 0.25)]);
        cycle(&mut state, &mut list);
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames, vec![(0.0, 0.25); 512]);
        assert_eq!(issues, vec![CaptureIssue::SourceLost(Channel::Mic)]);
    }

    #[test]
    fn a_null_mic_buffer_keeps_the_timeline_and_reports_the_mic_lost() {
        let (mut state, mut source) = mic_system(1);
        let mut list = List::new(vec![null(1, 512 * 4), interleaved(2, 512, 0.25)]);
        cycle(&mut state, &mut list);
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames, vec![(0.0, 0.25); 512]);
        assert_eq!(issues, vec![CaptureIssue::SourceLost(Channel::Mic)]);
    }

    #[test]
    fn a_short_mic_buffer_is_silence_for_the_whole_cycle() {
        let (mut state, mut source) = mic_system(1);
        let mut list = List::new(vec![interleaved(1, 256, 0.5), interleaved(2, 512, 0.25)]);
        cycle(&mut state, &mut list);
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames, vec![(0.0, 0.25); 512]);
        assert_eq!(issues, vec![CaptureIssue::SourceLost(Channel::Mic)]);
    }

    #[test]
    fn a_tap_buffer_with_another_channel_count_is_lost_not_misread() {
        let (mut state, mut source) = mic_system(1);
        let mut list = List::new(vec![interleaved(1, 512, 0.5), interleaved(1, 1024, 0.25)]);
        cycle(&mut state, &mut list);
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames, vec![(0.5, 0.0); 512]);
        assert_eq!(issues, vec![CaptureIssue::SourceLost(Channel::System)]);
    }

    #[test]
    fn a_partial_frame_marks_the_buffer_lost() {
        let (mut state, mut source) = mic_system(1);
        let mut tap = interleaved(2, 512, 0.25);
        tap.bytes = Some(512 * 8 - 4);
        let mut list = List::new(vec![interleaved(1, 512, 0.5), tap]);
        cycle(&mut state, &mut list);
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames, vec![(0.5, 0.0); 512]);
        assert_eq!(issues, vec![CaptureIssue::SourceLost(Channel::System)]);
    }

    #[test]
    fn a_cycle_larger_than_one_chunk_is_pushed_whole() {
        let (mut state, mut source) = mic_system(1);
        let frames = CHUNK_FRAMES * 2 + 100;
        let mut list = List::new(vec![
            interleaved(1, frames, 0.5),
            interleaved(2, frames, 0.25),
        ]);
        cycle(&mut state, &mut list);
        let (out, issues) = drain(&mut source);
        assert_eq!(out, vec![(0.5, 0.25); frames]);
        assert!(issues.is_empty());
    }

    #[test]
    fn a_source_that_returns_is_reported_restored() {
        let (mut state, mut source) = mic_system(1);
        cycle(
            &mut state,
            &mut List::new(vec![null(1, 0), interleaved(2, 64, 0.25)]),
        );
        cycle(
            &mut state,
            &mut List::new(vec![interleaved(1, 64, 0.5), interleaved(2, 64, 0.25)]),
        );
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames.len(), 128);
        assert_eq!(frames[64], (0.5, 0.25));
        assert_eq!(
            issues,
            vec![
                CaptureIssue::SourceLost(Channel::Mic),
                CaptureIssue::SourceRestored(Channel::Mic)
            ]
        );
    }

    #[test]
    fn a_list_with_another_buffer_count_keeps_the_timeline_with_silence() {
        let (mut state, mut source) = mic_system(1);
        cycle(
            &mut state,
            &mut List::new(vec![interleaved(1, 64, 0.5), interleaved(2, 64, 0.25)]),
        );
        cycle(&mut state, &mut List::new(vec![interleaved(2, 64, 0.25)]));
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames.len(), 128);
        assert_eq!(&frames[64..], &[(0.0, 0.0); 64]);
        assert_eq!(
            issues,
            vec![
                CaptureIssue::SourceLost(Channel::Mic),
                CaptureIssue::SourceLost(Channel::System)
            ]
        );
    }

    #[test]
    fn a_cycle_with_no_valid_buffer_uses_the_device_buffer_size() {
        let (mut state, mut source) = mic_system(1);
        cycle(&mut state, &mut List::new(vec![null(1, 0), null(2, 0)]));
        let (frames, _) = drain(&mut source);
        assert_eq!(frames, vec![(0.0, 0.0); 512]);
    }

    #[test]
    fn a_closed_cell_never_runs_the_state_again() {
        let (state, mut source) = mic_system(1);
        let cell = Arc::new(IoCell::new(state));
        let block = io_block(Arc::clone(&cell));
        let mut list = List::new(vec![interleaved(1, 64, 0.5), interleaved(2, 64, 0.25)]);
        let call = |list: &mut List| {
            // SAFETY: AudioTimeStamp is plain numbers and flags, for which all zeros is valid.
            let mut stamp: AudioTimeStamp = unsafe { std::mem::zeroed() };
            let ts = NonNull::from(&mut stamp);
            let out = NonNull::<AudioBufferList>::dangling();
            block.call((ts, list.ptr(), ts, out, ts));
        };
        call(&mut list);
        assert!(cell.close().is_some(), "close hands back the state once");
        assert!(cell.close().is_none());
        call(&mut list);
        let (frames, _) = drain(&mut source);
        assert_eq!(
            frames.len(),
            64,
            "only the cycle before close pushed frames"
        );
        assert!(source.is_finished(), "closing dropped the sink");
    }

    #[test]
    fn gap_frames_converts_host_ticks_to_output_frames() {
        assert_eq!(gap_frames(1_000, 1_000 + 48_000 * TICKS, CLOCKS), 48_000);
        assert_eq!(
            gap_frames(1_000, 1_000 + 250, CLOCKS),
            1,
            "half a frame rounds up"
        );
        assert_eq!(gap_frames(5_000, 1_000, CLOCKS), 0, "overlap is no gap");
        let to_16k = Clocks {
            out_hz: 16_000.0,
            ..CLOCKS
        };
        assert_eq!(gap_frames(0, 24_000_000 / 10, to_16k), 1_600);
        assert_eq!(
            gap_frames(0, u64::MAX, CLOCKS),
            30 * 48_000,
            "a nonsense host time cannot stall the IO thread"
        );
    }

    #[test]
    fn a_rebuilt_graph_fills_the_gap_with_silence_before_its_first_frame() {
        let (mut first, mut source) = mic_system(1);
        let t0 = 1_000_000;
        cycle_at(
            &mut first,
            &mut List::new(vec![interleaved(1, 512, 0.5), interleaved(2, 512, 0.25)]),
            Some(t0),
        );
        let mut timeline = first.into_timeline();
        timeline.resume();
        let gap = 3_000;
        let resumed_at = t0 + (512 + gap) * TICKS;
        let mut second = graph(timeline, Some(1));
        for k in 0..2 {
            cycle_at(
                &mut second,
                &mut List::new(vec![interleaved(1, 256, 0.75), interleaved(2, 256, 0.125)]),
                Some(resumed_at + k * 256 * TICKS),
            );
        }
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames.len(), 512 + gap as usize + 512);
        assert_eq!(&frames[..512], &[(0.5, 0.25); 512][..]);
        assert!(
            frames[512..512 + gap as usize]
                .iter()
                .all(|f| *f == (0.0, 0.0))
        );
        assert_eq!(&frames[512 + gap as usize..], &[(0.75, 0.125); 512][..]);
        assert_eq!(
            issues,
            vec![CaptureIssue::Rebuilt { gap_frames: gap }],
            "reported once, by the first cycle"
        );
        let span = &second.timeline.span;
        let host_frames =
            (span.end.load(Ordering::Relaxed) - span.first.load(Ordering::Relaxed)) / TICKS;
        assert_eq!(
            host_frames,
            frames.len() as u64,
            "frames out equal host time elapsed"
        );
    }

    #[test]
    fn a_gap_longer_than_the_ring_is_counted_not_lost() {
        let (sink, mut source) = frame_channel(ChannelLayout::MicSystem, 4_096);
        let mut first = graph(Timeline::new(sink, Arc::default()), Some(1));
        let list = || List::new(vec![interleaved(1, 64, 0.5), interleaved(2, 64, 0.25)]);
        cycle_at(&mut first, &mut list(), Some(0));
        let mut timeline = first.into_timeline();
        timeline.resume();
        let mut second = graph(timeline, Some(1));
        cycle_at(&mut second, &mut list(), Some((64 + 10_000) * TICKS));
        let mut out = Vec::new();
        let drained = source.drain(&mut out);
        assert_eq!(drained.frames + drained.padded_frames, 64 + 10_000 + 64);
    }

    #[test]
    fn mic_presence_carries_across_graphs() {
        let (mut first, mut source) = mic_system(1);
        let both = || List::new(vec![interleaved(1, 64, 0.5), interleaved(2, 64, 0.25)]);
        cycle_at(&mut first, &mut both(), Some(0));
        let mut timeline = first.into_timeline();
        timeline.resume();
        let mut micless = graph(timeline, None);
        cycle_at(
            &mut micless,
            &mut List::new(vec![interleaved(2, 64, 0.25)]),
            Some(64 * TICKS),
        );
        let mut timeline = micless.into_timeline();
        timeline.resume();
        let mut back = graph(timeline, Some(1));
        cycle_at(&mut back, &mut both(), Some(128 * TICKS));
        let (frames, issues) = drain(&mut source);
        assert_eq!(
            &frames[64..128],
            &[(0.0, 0.25); 64][..],
            "mic channel is silence"
        );
        assert_eq!(
            issues,
            vec![
                CaptureIssue::Rebuilt { gap_frames: 0 },
                CaptureIssue::SourceLost(Channel::Mic),
                CaptureIssue::Rebuilt { gap_frames: 0 },
                CaptureIssue::SourceRestored(Channel::Mic),
            ]
        );
    }

    #[test]
    fn a_rebuild_before_any_valid_host_time_reports_no_gap() {
        let (first, mut source) = mic_system(1);
        let mut timeline = first.into_timeline();
        timeline.resume();
        let mut second = graph(timeline, Some(1));
        cycle_at(
            &mut second,
            &mut List::new(vec![interleaved(1, 64, 0.5), interleaved(2, 64, 0.25)]),
            Some(10_000 * TICKS),
        );
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames.len(), 64);
        assert_eq!(issues, vec![CaptureIssue::Rebuilt { gap_frames: 0 }]);
    }

    #[test]
    fn skipped_frames_scale_to_the_output_rate_and_ignore_resets() {
        assert_eq!(skipped_frames(512.0, 512.0, CLOCKS), 0);
        assert_eq!(skipped_frames(512.0, 512.4, CLOCKS), 0, "sub-frame jitter");
        assert_eq!(skipped_frames(512.0, 1361.0, CLOCKS), 849);
        assert_eq!(
            skipped_frames(25_812.0, 751.0, CLOCKS),
            0,
            "sample time reset"
        );
        let from_96k = Clocks {
            device_hz: 96_000.0,
            ..CLOCKS
        };
        assert_eq!(skipped_frames(0.0, 1_000.0, from_96k), 500);
    }

    #[test]
    fn input_the_hal_skipped_inside_a_graph_is_filled_with_silence() {
        let (mut state, mut source) = mic_system(1);
        let list = || List::new(vec![interleaved(1, 512, 0.5), interleaved(2, 512, 0.25)]);
        let at = |sample: u64| Stamp {
            host: Some(sample * TICKS),
            sample: Some(sample as f64),
        };
        stamped(&mut state, &mut list(), at(751));
        stamped(&mut state, &mut list(), at(751 + 1361));
        stamped(&mut state, &mut list(), at(751 + 1361 + 512));
        let (frames, issues) = drain(&mut source);
        assert_eq!(frames.len(), 512 + 849 + 512 + 512);
        assert!(frames[512..512 + 849].iter().all(|f| *f == (0.0, 0.0)));
        assert_eq!(frames[512 + 849], (0.5, 0.25));
        assert!(issues.is_empty(), "{issues:?}");
        let span = &state.timeline.span;
        assert_eq!(span.skipped.load(Ordering::Relaxed), 849);
        let host_frames =
            (span.end.load(Ordering::Relaxed) - span.first.load(Ordering::Relaxed)) / TICKS;
        assert_eq!(host_frames, frames.len() as u64);
    }
}
