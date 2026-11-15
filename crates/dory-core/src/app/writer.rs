//! The take writer thread. One runs per capture interval. It owns the `TakeWriter` while the
//! device runs and hands it back by returning it, so the writer has exactly one owner at a time:
//! this thread while capturing, the recorder actor otherwise.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use super::events::{Event, EventHub};
use super::recorder::Msg;
use crate::capture::FrameSource;
use crate::domain::Ms;
use crate::ports::TakeWriter;
use crate::ports::capture::CaptureIssue;
use crate::recorder::Generation;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cadence {
    pub drain: Duration,
    pub levels: Duration,
    pub checkpoint: Duration,
}

pub(crate) struct WriterJob {
    pub writer: Box<dyn TakeWriter>,
    pub source: FrameSource,
    /// Set by the actor once the device has stopped. The adapter may still hold the sink, so the
    /// writer cannot wait for the sink to drop.
    pub stopped: Arc<AtomicBool>,
    pub generation: Generation,
    pub rate_hz: NonZeroU32,
    pub actor: Sender<Msg>,
    pub hub: EventHub,
    pub cadence: Cadence,
}

/// Runs until the device is stopped and the ring is drained, then returns the writer.
pub(crate) fn spawn(job: WriterJob) -> std::io::Result<JoinHandle<Box<dyn TakeWriter>>> {
    std::thread::Builder::new()
        .name("dory-take-writer".into())
        .spawn(move || job.run())
}

impl WriterJob {
    fn run(mut self) -> Box<dyn TakeWriter> {
        let meter = self.source.meter();
        let mut buf = Vec::new();
        let mut last_levels = Instant::now();
        let mut last_checkpoint = Instant::now();
        let mut failed = false;
        loop {
            // Read before draining: once the sink is gone or the device has stopped nothing more
            // can arrive, so the drain below is the last one and catches every frame and every
            // padded overrun.
            let finished = self.source.is_finished() || self.stopped.load(Ordering::Acquire);
            buf.clear();
            self.source.drain(&mut buf);
            if !buf.is_empty() && !failed {
                match self.writer.write(&buf) {
                    Ok(()) => {
                        let _ = self.actor.send(Msg::Wrote {
                            total_frames: self.writer.frames_written(),
                        });
                    }
                    Err(error) => {
                        failed = true;
                        self.hub.publish(Event::error("writing the take", error));
                        self.fail();
                    }
                }
            }
            let issues: Vec<CaptureIssue> = self.source.issues().collect();
            for issue in issues {
                let fatal = matches!(issue, CaptureIssue::Fatal(_));
                self.hub.publish(Event::CaptureIssue(issue));
                if fatal {
                    self.fail();
                }
            }
            if last_levels.elapsed() >= self.cadence.levels {
                last_levels = Instant::now();
                self.hub.publish(Event::Levels {
                    peaks: meter.take_peaks(),
                    recorded: Ms::from_frames(self.writer.frames_written(), self.rate_hz),
                });
            }
            if !failed && last_checkpoint.elapsed() >= self.cadence.checkpoint {
                last_checkpoint = Instant::now();
                self.checkpoint();
            }
            if finished {
                break;
            }
            std::thread::sleep(self.cadence.drain);
        }
        if !failed {
            self.checkpoint();
        }
        self.writer
    }

    fn fail(&self) {
        let _ = self.actor.send(Msg::CaptureFailed {
            generation: self.generation,
        });
    }

    fn checkpoint(&mut self) {
        if let Err(error) = self.writer.checkpoint() {
            self.hub
                .publish(Event::error("checkpointing the take", error));
        }
    }
}
