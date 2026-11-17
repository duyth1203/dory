//! The processing queue's reads and writes for the Processing window (UX-15,
//! `docs/design/floating-surfaces.md` section 4). A child of `app`, so it reaches the private
//! core boundary like the rest of `AppState`.

use std::sync::Arc;
use std::time::Duration;

use dory_core::domain::{Job, JobId, JobState, Ms, RecordingId, Waveform};
use gpui_kit::{Context, SharedString, Task};

use super::AppState;
use crate::estimate::{Estimate, Left};
use crate::library::{self, Progress};
use crate::processing::{Phase, Problem};
use crate::processing_queue::{self, Blocked, Ended, Finished, Queue, Running, Waiting};

/// How long Undo stays offered after Cancel.
pub const UNDO_SECONDS: u8 = 5;

/// A cancel that Undo can still take back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cancelled {
    pub job: JobId,
    pub text: SharedString,
    pub seconds_left: u8,
    generation: u64,
}

impl AppState {
    /// Running, queued, and blocked jobs, in the order the queue shows them.
    pub fn queue(&self, cx: &mut Context<Self>) -> Task<Result<Vec<Job>, String>> {
        self.core.run(cx, |dory| {
            dory.pipeline().queue().map_err(|e| e.to_string())
        })
    }

    pub fn progress_of(&self, id: &RecordingId) -> Option<Progress> {
        self.progress.get(id).copied()
    }

    fn title_of(&self, id: &RecordingId) -> String {
        self.row(id).map_or_else(
            || "Recording".to_owned(),
            |row| library::title(row, &chrono::Local),
        )
    }

    /// The Processing window's four sections, from the queue read with the rows, this session's
    /// finished jobs, and the estimates. A pinned preview wins.
    pub fn processing_queue(&self) -> Queue {
        if let Some(preview) = &self.queue_preview {
            return preview.clone();
        }
        let mut queue = Queue {
            finished: self.finished.clone(),
            left: self.queue_left(),
            ..Queue::default()
        };
        for job in &self.queue {
            let id = &job.recording;
            match &job.state {
                JobState::Running { .. } => {
                    queue.now = Some(Running {
                        job: job.id,
                        recording: id.clone(),
                        title: self.title_of(id),
                        audio: self.audio_of(id),
                        phase: self
                            .phase_of(id)
                            .filter(|p| {
                                matches!(
                                    p,
                                    Phase::Preparing
                                        | Phase::Transcribing { .. }
                                        | Phase::Summarizing { .. }
                                )
                            })
                            .unwrap_or(Phase::Preparing),
                    })
                }
                JobState::Queued => queue.next.push(Waiting {
                    job: job.id,
                    recording: id.clone(),
                    title: self.title_of(id),
                    audio: self.audio_of(id),
                    starts_in: self.spot_of(id).and_then(|s| s.starts_in),
                }),
                JobState::Blocked { .. } if self.row(id).is_none() => {
                    queue.attention.push(Blocked {
                        recording: id.clone(),
                        title: self.title_of(id),
                        problem: Problem::of(&job.state, &self.models, &self.downloads),
                    })
                }
                _ => {}
            }
        }
        for row in &self.recordings {
            if let Phase::Failed(problem) = self.phase(row) {
                queue.attention.push(Blocked {
                    recording: row.recording.id.clone(),
                    title: library::title(row, &chrono::Local),
                    problem,
                });
            }
        }
        queue
    }

    /// Pins the Processing window to `queue`, for the visual pass and the layout checks.
    pub fn preview_queue(&mut self, queue: Option<Queue>, cx: &mut Context<Self>) {
        self.queue_preview = queue;
        cx.notify();
    }

    /// How long one active job still takes: the running one's smoothed estimate, a queued one's
    /// audio at this Mac's rate.
    pub(super) fn takes(&self, job: &Job) -> Option<Ms> {
        match job.state {
            JobState::Running { .. } => {
                match self.estimates.get(&job.recording).map(Estimate::left) {
                    Some(Left::About(ms)) => Some(ms),
                    _ => None,
                }
            }
            _ => self
                .audio_of(&job.recording)
                .zip(self.rates.any())
                .map(|(audio, rate)| rate.time_for(audio)),
        }
    }

    /// The time left for every running and queued job, once each has an estimate.
    fn queue_left(&self) -> Option<Ms> {
        let mut active = self
            .queue
            .iter()
            .filter(|j| matches!(j.state, JobState::Queued | JobState::Running { .. }))
            .peekable();
        active.peek()?;
        active.try_fold(Ms(0), |sum, job| self.takes(job).map(|t| Ms(sum.0 + t.0)))
    }

    /// Notes the jobs that left the queue since the last read as finished this session, done or
    /// cancelled, and drops any that came back (Undo).
    pub(super) fn track_finished(&mut self, before: &[Job]) {
        let now = chrono::Local::now();
        let at = (chrono::Timelike::hour(&now), chrono::Timelike::minute(&now));
        for job in before {
            if self.queue.iter().any(|j| j.id == job.id) {
                continue;
            }
            let ended = match self.row(&job.recording).and_then(|r| r.job.clone()) {
                Some(JobState::Succeeded) => Ended::Done,
                Some(JobState::Cancelled) => Ended::Cancelled,
                _ => continue,
            };
            if self.finished.iter().any(|f| f.job == job.id) {
                continue;
            }
            self.finished.insert(
                0,
                Finished {
                    job: job.id,
                    recording: job.recording.clone(),
                    title: self.title_of(&job.recording),
                    ended,
                    at,
                },
            );
        }
        let queue = &self.queue;
        self.finished
            .retain(|f| !queue.iter().any(|j| j.id == f.job));
    }

    /// Clear: empties Finished this session.
    pub fn clear_finished(&mut self, cx: &mut Context<Self>) {
        self.finished.clear();
        if let Some(preview) = &mut self.queue_preview {
            preview.finished.clear();
        }
        cx.notify();
    }

    /// Moves a queued job to index `to` among the queued jobs. The running job never moves.
    pub fn move_job(&mut self, job: JobId, to: usize, cx: &mut Context<Self>) {
        let done = self
            .core
            .run(cx, move |dory| dory.pipeline().move_job(job, to));
        self.report(done, cx);
    }

    /// Move to Top (UX-15) from the card: the recording's queued job goes first in line.
    pub fn move_to_top(&mut self, id: RecordingId, cx: &mut Context<Self>) {
        if let Some(job) = self
            .queue
            .iter()
            .find(|j| j.recording == id && j.state == JobState::Queued)
        {
            self.move_job(job.id, 0, cx);
        }
    }

    /// Cancel, at once and with no question. The toast offers Undo for [`UNDO_SECONDS`].
    pub fn cancel_job(&mut self, job: JobId, cx: &mut Context<Self>) {
        let queue = self.processing_queue();
        let (title, restarts) = match &queue.now {
            Some(running) if running.job == job => {
                (running.title.clone(), running.restarts_when_undone())
            }
            _ => (
                queue
                    .next
                    .iter()
                    .find(|w| w.job == job)
                    .map_or_else(|| "the recording".to_owned(), |w| w.title.clone()),
                false,
            ),
        };
        let done = self.core.run(cx, move |dory| dory.pipeline().cancel(job));
        self.report(done, cx);
        let text: SharedString = processing_queue::cancelled(&title, restarts).into();
        crate::a11y::announce(&text);
        self.cancel_generation += 1;
        self.cancelled = Some(Cancelled {
            job,
            text,
            seconds_left: UNDO_SECONDS,
            generation: self.cancel_generation,
        });
        self.tick_cancel(self.cancel_generation, cx);
        cx.notify();
    }

    fn tick_cancel(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let more = this.update(cx, |state, cx| {
                    let Some(cancelled) = &mut state.cancelled else {
                        return false;
                    };
                    if cancelled.generation != generation {
                        return false;
                    }
                    cancelled.seconds_left = cancelled.seconds_left.saturating_sub(1);
                    if cancelled.seconds_left == 0 {
                        state.cancelled = None;
                    }
                    cx.notify();
                    state.cancelled.is_some()
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The cancel Undo can still take back.
    pub fn cancelled(&self) -> Option<&Cancelled> {
        self.cancelled.as_ref()
    }

    /// Undo: the cancelled job goes back where it was. Its place in line survives the cancel.
    pub fn undo_cancel(&mut self, cx: &mut Context<Self>) {
        let Some(cancelled) = self.cancelled.take() else {
            return;
        };
        let job = cancelled.job;
        let done = self.core.run(cx, move |dory| dory.pipeline().retry(job));
        self.report(done, cx);
        cx.notify();
    }

    /// The running job's levels, for the window's mini waveform. Read once per recording.
    pub fn waveform_of(&self, id: &RecordingId) -> Option<Arc<Waveform>> {
        self.waveforms.get(id).cloned().flatten()
    }

    /// Starts reading `id`'s levels unless they are read or being read.
    pub fn want_waveform(&mut self, id: &RecordingId, cx: &mut Context<Self>) {
        if self.waveforms.contains_key(id) {
            return;
        }
        self.waveforms.insert(id.clone(), None);
        let key = id.clone();
        let read = self
            .core
            .run(cx, move |dory| dory.library().waveform(&key).ok());
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let waveform = read.await;
            let _ = this.update(cx, |state, cx| {
                if let Some(waveform) = waveform {
                    state.waveforms.insert(id, Some(Arc::new(waveform)));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
