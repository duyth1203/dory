//! The pipeline worker. The queue is the store: the worker takes `next_runnable_job`, commits
//! `Started`, runs the stage, and commits its output together with `StageDone` in one apply.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::Options;
use super::events::{Db, Event};
use super::models::{Engines, Registries};
use super::pace::{self, Budget, Pacer};
use super::recorder::RecorderWatch;
use super::stages::{self, StageCx, StageError};
use crate::domain::{
    FailureClass, Job, JobEvent, JobId, JobState, MAX_ATTEMPTS, Processing, RecordingId,
    RecordingQuery, Stage, Timestamp,
};
use crate::ports::{CancelToken, Change, Clock, Machine, Store, StoreError, Takes};

/// Wakes the worker. Wakes coalesce, so sending never blocks.
#[derive(Clone)]
pub(crate) struct Nudge(Sender<()>);

impl Nudge {
    pub fn wake(&self) {
        let _ = self.0.try_send(());
    }
}

pub(crate) fn nudge_channel() -> (Nudge, Receiver<()>) {
    let (tx, rx) = crossbeam_channel::bounded(1);
    (Nudge(tx), rx)
}

/// The job the worker is running, so the facade can cancel it.
#[derive(Default)]
pub(crate) struct Current(Mutex<Option<(JobId, RecordingId, CancelToken)>>);

impl Current {
    fn set(&self, value: Option<(JobId, RecordingId, CancelToken)>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = value;
    }

    /// Cancels the running job if `matches` says so.
    pub fn cancel_if(&self, matches: impl Fn(JobId, &RecordingId) -> bool) {
        if let Some((job, recording, token)) =
            &*self.0.lock().unwrap_or_else(PoisonError::into_inner)
            && matches(*job, recording)
        {
            token.cancel();
        }
    }
}

pub(crate) struct PipelineHandle {
    pub nudge: Nudge,
    pub current: Arc<Current>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl PipelineHandle {
    /// Cancels the running stage and waits for the worker to stop. A stage cut short goes back to
    /// the queue, not to Cancelled. A second caller also returns only once the worker has stopped.
    pub fn shutdown(&self) {
        let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(thread) = thread.take() else {
            return;
        };
        // Stop first, then cancel: a worker that sets `current` after this reads `stop` and
        // leaves its job queued.
        self.stop.store(true, Ordering::Release);
        self.current.cancel_if(|_, _| true);
        self.nudge.wake();
        let _ = thread.join();
    }
}

pub(crate) struct Deps {
    pub db: Db,
    pub takes: Arc<dyn Takes>,
    pub clock: Arc<dyn Clock>,
    pub models: Arc<Registries>,
    pub machine: Arc<dyn Machine>,
    pub recorder: RecorderWatch,
}

pub(crate) fn spawn(
    deps: Deps,
    options: &Options,
    nudge: Nudge,
    wake: Receiver<()>,
) -> std::io::Result<PipelineHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let current = Arc::new(Current::default());
    let worker = Worker {
        pacing: Pacing {
            pacer: Pacer::new(pace::fast_threads()),
            machine: Arc::clone(&deps.machine),
            recorder: deps.recorder.clone(),
            clock: Arc::clone(&deps.clock),
            store: Arc::clone(&deps.db.store),
            processing: options.processing,
        },
        engines: Engines::new(options.engine_idle_ms),
        retry_base_ms: options.retry_base_ms,
        clock_poll: options.clock_poll,
        stop: Arc::clone(&stop),
        current: Arc::clone(&current),
        wake,
        deps,
    };
    let thread = std::thread::Builder::new()
        .name("dory-pipeline".into())
        .spawn(move || worker.run())?;
    Ok(PipelineHandle {
        nudge,
        current,
        stop,
        thread: Mutex::new(Some(thread)),
    })
}

/// Decides the budget for each window.
struct Pacing {
    pacer: Pacer,
    machine: Arc<dyn Machine>,
    recorder: RecorderWatch,
    clock: Arc<dyn Clock>,
    store: Arc<dyn Store>,
    processing: Option<Processing>,
}

impl Pacing {
    /// Reads the setting and the job again each time, so a change of either applies from the
    /// next window. A job the user ran fast is Fast whatever the setting says.
    fn budget(&mut self, job: JobId) -> Budget {
        let run_fast = self
            .store
            .job(job)
            .is_ok_and(|j| j.is_some_and(|j| j.run_fast));
        let setting = if run_fast {
            Processing::Fast
        } else {
            self.processing.unwrap_or_else(|| {
                self.store
                    .settings()
                    .map_or(Processing::Auto, |s| s.processing)
            })
        };
        self.pacer.next(
            setting,
            self.machine.conditions(),
            self.recorder.busy(),
            self.clock.monotonic_ms(),
        )
    }
}

struct Worker {
    deps: Deps,
    pacing: Pacing,
    engines: Engines,
    retry_base_ms: u64,
    clock_poll: Duration,
    stop: Arc<AtomicBool>,
    current: Arc<Current>,
    wake: Receiver<()>,
}

impl Worker {
    fn run(mut self) {
        while !self.stopping() {
            self.engines.sweep(self.deps.clock.monotonic_ms());
            let now = self.deps.clock.now();
            match self.deps.db.store.next_runnable_job(now) {
                Ok(Some(job)) => self.run_job(job),
                Ok(None) => self.idle(now),
                Err(error) => {
                    self.deps
                        .db
                        .publish(Event::error("reading the queue", error));
                    self.sleep(Some(self.clock_poll));
                }
            }
        }
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Sleeps until a nudge, the next retry, or the next engine to unload.
    fn idle(&mut self, now: Timestamp) {
        let retry = match self.deps.db.store.next_wakeup(now) {
            Ok(at) => at.map(|at| ms(at.0.saturating_sub(now.0))),
            Err(_) => Some(self.clock_poll),
        };
        let mono = self.deps.clock.monotonic_ms();
        let unload = self
            .engines
            .next_expiry()
            .map(|at| Duration::from_millis(at.saturating_sub(mono)));
        self.sleep(retry.into_iter().chain(unload).min());
    }

    fn sleep(&self, timeout: Option<Duration>) {
        match timeout {
            // Retry times and idle deadlines are on the injected clock, which need not track
            // `Instant`, so re-read it at least every `clock_poll`.
            Some(timeout) => match self.wake.recv_timeout(timeout.min(self.clock_poll)) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.stop.store(true, Ordering::Release),
            },
            None => {
                if self.wake.recv().is_err() {
                    self.stop.store(true, Ordering::Release);
                }
            }
        }
    }

    fn run_job(&mut self, job: Job) {
        let Some(stage) = job.next_stage() else {
            return;
        };
        let started = JobEvent::Started { stage };
        let Ok(running) = job.on(&started) else {
            return;
        };
        // Cancellable before `Started` commits. The facade commits a cancel or a delete before it
        // cancels `current`, so one that commits after `Started` finds this token, and one that
        // commits before makes `Started` fail below. Shutdown sets `stop` before it cancels
        // `current`, so a shutdown that missed this token is seen here and the job stays queued.
        let cancel = CancelToken::new();
        self.current
            .set(Some((job.id, job.recording.clone(), cancel.clone())));
        if self.stopping() {
            self.current.set(None);
            return;
        }
        if let Err(error) = self.deps.db.apply(vec![Change::Job {
            job: job.id,
            event: started,
        }]) {
            self.current.set(None);
            // A job the user moved first is simply gone. A failing store gets a pause instead of
            // a hot loop.
            if !job_moved(&error) {
                self.report_commit(error);
                self.sleep(Some(self.clock_poll));
            }
            return;
        }

        let id = job.id;
        let outcome = match self.deps.db.store.settings() {
            Ok(settings) => stages::run(
                stage,
                StageCx {
                    job: &running,
                    db: &self.deps.db,
                    takes: self.deps.takes.as_ref(),
                    models: &self.deps.models,
                    engines: &mut self.engines,
                    cancel: &cancel,
                    settings: &settings,
                    now_mono: self.deps.clock.monotonic_ms(),
                    pace: &mut || self.pacing.budget(id),
                },
            ),
            Err(error) => Err(stages::failed(error)),
        };
        self.current.set(None);
        self.engines.release(self.deps.clock.monotonic_ms());
        self.settle(&running, stage, outcome, cancel.is_cancelled());
    }

    /// Commits a stage's result. Every event committed here is legal only from `Running`, which
    /// only this thread enters, so a result that lost a race with the user (cancel, then retry)
    /// is refused instead of overwriting the newer state.
    fn settle(
        &self,
        job: &Job,
        stage: Stage,
        outcome: Result<Vec<Change>, StageError>,
        cancelled: bool,
    ) {
        let error = match outcome {
            Ok(mut changes) => {
                changes.push(Change::Job {
                    job: job.id,
                    event: JobEvent::StageDone { stage },
                });
                match self.deps.db.apply(changes) {
                    Ok(()) => return,
                    Err(error) if job_moved(&error) => return,
                    Err(error) => stages::failed(error),
                }
            }
            Err(error) => error,
        };
        let Some(event) = self.failure_event(job, stage, error, cancelled) else {
            return;
        };
        if let Err(error) = self.deps.db.apply(vec![Change::Job { job: job.id, event }])
            && !job_moved(&error)
        {
            self.report_commit(error);
        }
    }

    /// `None` when there is nothing to commit: the user's cancel already is.
    fn failure_event(
        &self,
        job: &Job,
        stage: Stage,
        error: StageError,
        cancelled: bool,
    ) -> Option<JobEvent> {
        let now = self.deps.clock.now();
        Some(match error {
            StageError::Blocked(blocker) => JobEvent::Blocked { stage, blocker },
            StageError::Failed(error) => match error.class {
                FailureClass::Cancelled if self.stopping() => JobEvent::Retry { not_before: now },
                FailureClass::Cancelled if cancelled => return None,
                FailureClass::Transient if job.attempts < MAX_ATTEMPTS => JobEvent::Retry {
                    not_before: Timestamp(
                        now.0
                            .saturating_add(backoff_ms(self.retry_base_ms, job.attempts) as i64),
                    ),
                },
                // Includes an engine that reports a cancel nobody asked for.
                FailureClass::Cancelled
                | FailureClass::Transient
                | FailureClass::NeedsUser
                | FailureClass::Permanent => JobEvent::Failed { stage, error },
            },
        })
    }

    fn report_commit(&self, error: StoreError) {
        if !job_moved(&error) {
            self.deps
                .db
                .publish(Event::error("saving pipeline progress", error));
        }
    }
}

fn job_moved(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::IllegalTransition(_) | StoreError::NotFound(_)
    )
}

fn ms(millis: i64) -> Duration {
    Duration::from_millis(u64::try_from(millis).unwrap_or(0))
}

/// `base`, doubling per attempt, at most an hour.
pub(crate) fn backoff_ms(base: u64, attempts: u8) -> u64 {
    let doublings = u32::from(attempts.saturating_sub(1)).min(16);
    base.saturating_mul(1 << doublings).min(60 * 60 * 1000)
}

/// Requeues every blocked job so the worker resolves its models again. One that is still stuck
/// blocks again, so calling this after any change the user makes is safe.
pub(crate) fn unblock_all(db: &Db) {
    let blocked = match jobs_where(db, |state| matches!(state, JobState::Blocked { .. })) {
        Ok(jobs) => jobs,
        Err(error) => {
            db.publish(Event::error("unblocking jobs", error));
            return;
        }
    };
    for job in blocked {
        let change = Change::Job {
            job: job.id,
            event: JobEvent::Unblock,
        };
        if let Err(error) = db.apply(vec![change])
            && !job_moved(&error)
        {
            db.publish(Event::error("unblocking jobs", error));
        }
    }
}

/// What the queue popover lists: the running job, then the queued ones in the order the worker
/// takes them (Front first, then oldest), then the blocked ones.
pub(crate) fn queue_order(jobs: &mut [Job]) {
    let rank = |state: &JobState| match state {
        JobState::Running { .. } => 0,
        JobState::Queued => 1,
        _ => 2,
    };
    jobs.sort_by_key(|j| {
        (
            rank(&j.state),
            std::cmp::Reverse(j.priority),
            j.created_at,
            j.id,
        )
    });
}

/// Every job in a state `wanted` accepts. A list row shows only the newest job of a recording,
/// so this reads each recording's jobs.
pub(crate) fn jobs_where(
    db: &Db,
    wanted: impl Fn(&JobState) -> bool,
) -> Result<Vec<Job>, StoreError> {
    let mut jobs = Vec::new();
    for row in db.store.recordings(&RecordingQuery::default())? {
        jobs.extend(
            db.store
                .jobs_for(&row.recording.id)?
                .into_iter()
                .filter(|j| wanted(&j.state)),
        );
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Goal, Priority};

    fn job(id: u64, state: JobState, priority: Priority, at: i64) -> Job {
        let recording = RecordingId::parse(&format!("r{id}")).unwrap();
        let mut job = Job::new(JobId(id), recording, Goal::ALL, priority, Timestamp(at));
        job.state = state;
        job
    }

    #[test]
    fn the_queue_lists_running_then_front_then_oldest_then_blocked() {
        let running = JobState::Running {
            stage: Stage::Transcribe,
        };
        let mut jobs = vec![
            job(1, JobState::Queued, Priority::Normal, 10),
            job(2, JobState::Queued, Priority::Normal, 5),
            job(3, JobState::Queued, Priority::Front, 20),
            job(4, running, Priority::Normal, 30),
        ];
        queue_order(&mut jobs);
        let ids: Vec<u64> = jobs.iter().map(|j| j.id.0).collect();
        assert_eq!(ids, [4, 3, 2, 1]);
    }

    #[test]
    fn backoff_doubles_from_the_base_and_is_capped() {
        let cases = [
            (0, 10_000),
            (1, 10_000),
            (2, 20_000),
            (3, 40_000),
            (200, 3_600_000),
        ];
        for (attempts, want) in cases {
            assert_eq!(backoff_ms(10_000, attempts), want, "{attempts}");
        }
    }
}
