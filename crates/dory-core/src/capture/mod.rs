//! The frame ring between a platform capture adapter and the core's take writer.
//!
//! [`frame_channel`] returns a [`FrameSink`] for the adapter and a [`FrameSource`] for the core.
//! The sink is wait-free and allocation-free so adapters can push from a real-time audio thread.
//! Frames the ring cannot hold are counted, and the source pads them with silence, so elapsed
//! time always equals frames written divided by the rate.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError};

use crate::domain::ChannelLayout;
use crate::ports::capture::CaptureIssue;

const ISSUE_QUEUE: usize = 64;

/// Creates a ring that holds `capacity_frames` whole frames of `layout`.
pub fn frame_channel(layout: ChannelLayout, capacity_frames: usize) -> (FrameSink, FrameSource) {
    let channels = layout.channel_count();
    let (producer, consumer) = rtrb::RingBuffer::new(capacity_frames * channels);
    let shared = Arc::new(Shared {
        peaks: (0..channels).map(|_| AtomicU32::new(0)).collect(),
        dropped_frames: AtomicU64::new(0),
        dropped_issues: AtomicU64::new(0),
    });
    let (issue_tx, issue_rx) = crossbeam_channel::bounded(ISSUE_QUEUE);
    let sink = FrameSink {
        producer,
        channels,
        shared: Arc::clone(&shared),
        issues: IssueReporter {
            tx: issue_tx,
            shared: Arc::clone(&shared),
        },
    };
    let source = FrameSource {
        consumer,
        channels,
        shared,
        issues: issue_rx,
    };
    (sink, source)
}

struct Shared {
    /// Per-channel peak magnitude since the last read, stored as f32 bits. Non-negative f32 values
    /// order the same as their bit patterns, so `fetch_max` on the bits is a float max.
    peaks: Box<[AtomicU32]>,
    dropped_frames: AtomicU64,
    dropped_issues: AtomicU64,
}

/// The adapter's end. Fields are private: an adapter can push frames and report issues, nothing else.
pub struct FrameSink {
    producer: rtrb::Producer<f32>,
    channels: usize,
    shared: Arc<Shared>,
    issues: IssueReporter,
}

impl FrameSink {
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Wait-free, allocation-free, and free of panic paths, because adapters call it on the
    /// real-time audio thread. `interleaved` must hold whole frames; a trailing partial frame is
    /// ignored. Frames that do not fit are counted and later padded with silence by the
    /// [`FrameSource`].
    pub fn push(&mut self, interleaved: &[f32]) {
        let frames = interleaved.len().checked_div(self.channels).unwrap_or(0);
        if frames == 0 {
            return;
        }
        self.record_peaks(interleaved);

        let room = self
            .producer
            .slots()
            .checked_div(self.channels)
            .unwrap_or(0);
        let accepted = frames.min(room);
        if accepted > 0 {
            let samples = accepted * self.channels;
            if let (Ok(chunk), Some(head)) = (
                self.producer.write_chunk_uninit(samples),
                interleaved.get(..samples),
            ) {
                chunk.fill_from_iter(head.iter().copied());
            }
        }
        let dropped = frames - accepted;
        if dropped > 0 {
            self.shared
                .dropped_frames
                .fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }

    /// Reports an issue. Never blocks. Safe on the audio thread, though adapters usually report
    /// from listener threads through [`FrameSink::issue_reporter`].
    pub fn report(&self, issue: CaptureIssue) {
        self.issues.report(issue);
    }

    /// A handle for threads other than the audio thread, such as device-change listeners.
    pub fn issue_reporter(&self) -> IssueReporter {
        self.issues.clone()
    }

    fn record_peaks(&self, interleaved: &[f32]) {
        let mut peaks = [0.0f32; 8];
        for frame in interleaved.chunks_exact(self.channels.max(1)) {
            for (peak, sample) in peaks.iter_mut().zip(frame) {
                *peak = peak.max(sample.abs());
            }
        }
        for (slot, peak) in self.shared.peaks.iter().zip(&peaks) {
            slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
pub struct IssueReporter {
    tx: Sender<CaptureIssue>,
    shared: Arc<Shared>,
}

impl IssueReporter {
    pub fn report(&self, issue: CaptureIssue) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(issue) {
            self.shared.dropped_issues.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The core's end, owned by the take writer thread.
pub struct FrameSource {
    consumer: rtrb::Consumer<f32>,
    channels: usize,
    shared: Arc<Shared>,
    issues: Receiver<CaptureIssue>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// Frames read from the ring.
    pub frames: u64,
    /// Frames the ring had to drop, appended as silence after the read frames.
    pub padded_frames: u64,
}

impl FrameSource {
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Appends every whole frame in the ring to `out`, then silence for any dropped frames.
    pub fn drain(&mut self, out: &mut Vec<f32>) -> Drained {
        let readable = self.consumer.slots() / self.channels * self.channels;
        let mut drained = Drained::default();
        if readable > 0 {
            if let Ok(chunk) = self.consumer.read_chunk(readable) {
                let (a, b) = chunk.as_slices();
                out.extend_from_slice(a);
                out.extend_from_slice(b);
                chunk.commit_all();
            }
            drained.frames = (readable / self.channels) as u64;
        }
        let dropped = self.shared.dropped_frames.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            out.resize(out.len() + dropped as usize * self.channels, 0.0);
            drained.padded_frames = dropped;
        }
        drained
    }

    /// True once the sink is dropped and every frame has been drained.
    pub fn is_finished(&self) -> bool {
        self.consumer.is_abandoned() && self.consumer.slots() == 0
    }

    pub fn issues(&self) -> impl Iterator<Item = CaptureIssue> + '_ {
        self.issues.try_iter()
    }

    /// Issues that arrived while the issue queue was full.
    pub fn dropped_issue_count(&self) -> u64 {
        self.shared.dropped_issues.load(Ordering::Relaxed)
    }

    /// A cheap handle the UI can poll for levels.
    pub fn meter(&self) -> LevelMeter {
        LevelMeter {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// Per-channel peak levels since the last read.
#[derive(Clone)]
pub struct LevelMeter {
    shared: Arc<Shared>,
}

impl LevelMeter {
    /// Returns the peak magnitude (0.0 to 1.0 for normal audio) per channel and resets it.
    pub fn take_peaks(&self) -> Vec<f32> {
        self.shared
            .peaks
            .iter()
            .map(|p| f32::from_bits(p.swap(0, Ordering::Relaxed)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(frames: usize, mic: f32, system: f32) -> Vec<f32> {
        (0..frames).flat_map(|_| [mic, system]).collect()
    }

    #[test]
    fn frames_pushed_come_out_in_order() {
        let (mut sink, mut source) = frame_channel(ChannelLayout::MicSystem, 16);
        sink.push(&[0.1, 0.2, 0.3, 0.4]);
        let mut out = Vec::new();
        let d = source.drain(&mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(
            d,
            Drained {
                frames: 2,
                padded_frames: 0
            }
        );
    }

    #[test]
    fn overflow_is_padded_with_silence_so_elapsed_time_is_exact() {
        let (mut sink, mut source) = frame_channel(ChannelLayout::MicSystem, 4);
        sink.push(&stereo(6, 0.5, -0.5));
        let mut out = Vec::new();
        let d = source.drain(&mut out);
        assert_eq!(
            d,
            Drained {
                frames: 4,
                padded_frames: 2
            }
        );
        assert_eq!(out.len(), 6 * 2);
        assert_eq!(&out[8..], &[0.0; 4]);
    }

    #[test]
    fn peaks_are_per_channel_and_reset_on_read() {
        let (mut sink, source) = frame_channel(ChannelLayout::MicSystem, 16);
        let meter = source.meter();
        sink.push(&[0.1, -0.9, -0.4, 0.2]);
        assert_eq!(meter.take_peaks(), vec![0.4, 0.9]);
        assert_eq!(meter.take_peaks(), vec![0.0, 0.0]);
    }

    #[test]
    fn source_finishes_only_after_sink_dropped_and_ring_drained() {
        let (mut sink, mut source) = frame_channel(ChannelLayout::MicOnly, 8);
        sink.push(&[0.1, 0.2]);
        drop(sink);
        assert!(!source.is_finished());
        source.drain(&mut Vec::new());
        assert!(source.is_finished());
    }

    #[test]
    fn issues_reach_the_source_and_overflow_is_counted() {
        let (sink, source) = frame_channel(ChannelLayout::MicSystem, 8);
        let reporter = sink.issue_reporter();
        for _ in 0..ISSUE_QUEUE + 3 {
            reporter.report(CaptureIssue::SystemAudioSilent);
        }
        assert_eq!(source.issues().count(), ISSUE_QUEUE);
        assert_eq!(source.dropped_issue_count(), 3);
    }
}
