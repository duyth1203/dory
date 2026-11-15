//! The recorder actor. It owns the recorder state, the device handle, the open take, and the
//! grace deadline, feeds inputs to the pure [`crate::recorder::step`], and executes the effects.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use thiserror::Error;

use super::Options;
use super::events::{Db, Event};
use super::pipeline::Nudge;
use super::writer::{self, Cadence, WriterJob};
use crate::capture::frame_channel;
use crate::domain::{
    ChannelLayout, Goal, LanguagePref, Ms, Priority, Recording, RecordingEdit, RecordingId,
    TakeInfo,
};
use crate::ports::capture::{CaptureBackend, CaptureHandle, CaptureSpec, MicSelection};
use crate::ports::{Change, Clock, StoreError, TakeError, TakeWriter, Takes};
use crate::recorder::{Effect, FinalizeReason, Generation, Input, Limits, State, step};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartOptions {
    pub title: Option<String>,
    /// `None` uses the language in settings.
    pub language: Option<LanguagePref>,
    pub layout: ChannelLayout,
    pub mic: MicSelection,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            title: None,
            language: None,
            layout: ChannelLayout::MicSystem,
            mic: MicSelection::SystemDefault,
        }
    }
}

/// What the recorder is doing, for display. `recorded` is media time, so pauses never count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderStatus {
    Idle,
    Recording {
        id: RecordingId,
        recorded: Ms,
    },
    Paused {
        id: RecordingId,
        recorded: Ms,
    },
    /// Stopped, and the stop can still be undone.
    Grace {
        id: RecordingId,
        recorded: Ms,
    },
}

impl RecorderStatus {
    pub fn id(&self) -> Option<&RecordingId> {
        match self {
            RecorderStatus::Idle => None,
            RecorderStatus::Recording { id, .. }
            | RecorderStatus::Paused { id, .. }
            | RecorderStatus::Grace { id, .. } => Some(id),
        }
    }

    fn same_phase(&self, other: &RecorderStatus) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other) && self.id() == other.id()
    }
}

pub(crate) enum Msg {
    Start(StartOptions),
    Command(Input),
    Wrote { total_frames: u64 },
    CaptureFailed { generation: Generation },
    Shutdown,
}

/// The facade's end of the actor.
pub(crate) struct RecorderHandle {
    tx: Sender<Msg>,
    status: Arc<Mutex<RecorderStatus>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Error)]
#[error("the recorder has shut down")]
pub struct RecorderGone;

impl RecorderHandle {
    pub fn send(&self, msg: Msg) -> Result<(), RecorderGone> {
        self.tx.send(msg).map_err(|_| RecorderGone)
    }

    pub fn status(&self) -> RecorderStatus {
        lock(&self.status).clone()
    }

    /// Finalizes whatever is recording and waits until it is committed.
    pub fn shutdown(&self) {
        let Some(thread) = lock(&self.thread).take() else {
            return;
        };
        let _ = self.tx.send(Msg::Shutdown);
        let _ = thread.join();
    }
}

pub(crate) struct Deps {
    pub db: Db,
    pub takes: Arc<dyn Takes>,
    pub capture: Arc<dyn CaptureBackend>,
    pub clock: Arc<dyn Clock>,
    pub nudge: Nudge,
}

pub(crate) fn spawn(deps: Deps, options: &Options) -> std::io::Result<RecorderHandle> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let status = Arc::new(Mutex::new(RecorderStatus::Idle));
    let actor = Actor {
        state: State::idle(),
        limits: options.limits,
        cadence: Cadence {
            drain: options.drain_every,
            levels: options.level_every,
            checkpoint: options.checkpoint_every,
        },
        ring_frames: options.ring_frames(),
        clock_poll: options.clock_poll,
        take: None,
        pending: None,
        armed: None,
        suffix: 0,
        sealing: Vec::new(),
        status: Arc::clone(&status),
        published: RecorderStatus::Idle,
        tx: tx.clone(),
        deps,
    };
    let thread = std::thread::Builder::new()
        .name("dory-recorder".into())
        .spawn(move || actor.run(rx))?;
    Ok(RecorderHandle {
        tx,
        status,
        thread: Mutex::new(Some(thread)),
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The open take. Exists from `OpenTake` until `Finalize` or `DiscardTake`.
struct Take {
    id: RecordingId,
    layout: ChannelLayout,
    mic: MicSelection,
    writer: Writer,
}

/// Who holds the take writer. The variants make a second owner unrepresentable.
enum Writer {
    Parked(Box<dyn TakeWriter>),
    Running {
        capture: Box<dyn CaptureHandle>,
        thread: JoinHandle<Box<dyn TakeWriter>>,
    },
    /// The writer thread panicked. The file keeps what was last checkpointed.
    Lost,
}

#[derive(Debug, Error)]
enum ActorError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Take(#[from] TakeError),
}

struct Actor {
    state: State,
    limits: Limits,
    cadence: Cadence,
    ring_frames: usize,
    clock_poll: Duration,
    take: Option<Take>,
    /// Options for the `OpenTake` that the pending `Start` will produce.
    pending: Option<StartOptions>,
    /// The grace deadline on the injected monotonic clock.
    armed: Option<u64>,
    suffix: u16,
    sealing: Vec<JoinHandle<()>>,
    status: Arc<Mutex<RecorderStatus>>,
    published: RecorderStatus,
    tx: Sender<Msg>,
    deps: Deps,
}

impl Actor {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.armed {
                // The deadline is on the injected clock, which need not track `Instant`, so wake
                // at least every `clock_poll` to re-read it.
                Some(deadline) => {
                    let left = deadline.saturating_sub(self.deps.clock.monotonic_ms());
                    match rx.recv_timeout(Duration::from_millis(left).min(self.clock_poll)) {
                        Ok(msg) => Some(msg),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => Some(Msg::Shutdown),
                    }
                }
                None => Some(rx.recv().unwrap_or(Msg::Shutdown)),
            };
            if let Some(deadline) = self.armed
                && self.deps.clock.monotonic_ms() >= deadline
            {
                self.armed = None;
                self.feed(Input::GraceElapsed {
                    armed_for: deadline,
                });
            }
            match msg {
                None => {}
                Some(Msg::Start(options)) => {
                    self.pending = Some(options);
                    self.feed(Input::Start);
                }
                Some(Msg::Command(input)) => self.feed(input),
                Some(Msg::Wrote { total_frames }) => self.feed(Input::Wrote { total_frames }),
                Some(Msg::CaptureFailed { generation }) => {
                    self.feed(Input::CaptureFailed { generation });
                }
                Some(Msg::Shutdown) => {
                    self.feed(Input::Shutdown);
                    break;
                }
            }
            self.sealing.retain(|t| !t.is_finished());
        }
        for thread in self.sealing.drain(..) {
            let _ = thread.join();
        }
    }

    fn feed(&mut self, input: Input) {
        let mut inputs = VecDeque::from([input]);
        while let Some(input) = inputs.pop_front() {
            let (next, effects) = step(
                self.state,
                input,
                &self.limits,
                self.deps.clock.monotonic_ms(),
            );
            self.state = next;
            for effect in effects {
                if let Some(follow) = self.execute(effect, input) {
                    inputs.push_back(follow);
                }
            }
            self.sync_status();
        }
    }

    fn execute(&mut self, effect: Effect, input: Input) -> Option<Input> {
        match effect {
            Effect::OpenTake => {
                if let Err(error) = self.open_take() {
                    self.publish(Event::error("creating the recording", error));
                }
            }
            Effect::StartCapture { generation } => {
                if !self.start_capture(generation) {
                    return Some(Input::StartFailed { generation });
                }
            }
            Effect::StopCapture => self.stop_capture(),
            Effect::ArmGrace { deadline } => self.armed = Some(deadline),
            Effect::Warn { remaining } => self.publish(Event::TimeWarning { remaining }),
            Effect::Finalize { reason, .. } => self.finalize(reason),
            Effect::DiscardTake => self.discard(),
            Effect::Rejected => {
                if input == Input::Start {
                    self.pending = None;
                }
                self.publish(Event::Rejected(input));
            }
        }
        None
    }

    fn open_take(&mut self) -> Result<(), ActorError> {
        let options = self.pending.take().unwrap_or_default();
        let store = &self.deps.db.store;
        let settings = store.settings()?;
        let now = self.deps.clock.now();
        let id = loop {
            self.suffix = self.suffix.wrapping_add(1);
            let id = RecordingId::generate(now, self.suffix);
            if store.recording(&id)?.is_none() {
                break id;
            }
        };
        // The row first: recovery reads the layout from it, and a row without audio is harmless.
        self.deps
            .db
            .apply(vec![Change::RecordingCreated(Recording {
                id: id.clone(),
                title: options.title,
                created_at: now,
                language: options.language.unwrap_or(settings.language),
                layout: options.layout,
                take: None,
            })])?;
        match self
            .deps
            .takes
            .begin(&id, options.layout, self.limits.rate_hz.get())
        {
            Ok(writer) => {
                self.take = Some(Take {
                    id,
                    layout: options.layout,
                    mic: options.mic,
                    writer: Writer::Parked(writer),
                });
                Ok(())
            }
            Err(error) => {
                let _ = self
                    .deps
                    .db
                    .apply(vec![Change::RecordingDeleted(id.clone())]);
                Err(error.into())
            }
        }
    }

    /// Starts the device with a fresh ring that feeds the same open take. False when it could not.
    fn start_capture(&mut self, generation: Generation) -> bool {
        let Some(take) = self.take.as_mut() else {
            return false;
        };
        let writer = match std::mem::replace(&mut take.writer, Writer::Lost) {
            Writer::Parked(writer) => writer,
            other => {
                take.writer = other;
                return false;
            }
        };
        let (sink, source) = frame_channel(take.layout, self.ring_frames);
        let spec = CaptureSpec {
            layout: take.layout,
            rate_hz: self.limits.rate_hz.get(),
            mic: take.mic.clone(),
        };
        let capture = match self.deps.capture.start(&spec, sink) {
            Ok(capture) => capture,
            Err(error) => {
                take.writer = Writer::Parked(writer);
                self.deps
                    .db
                    .publish(Event::error("starting capture", error));
                return false;
            }
        };
        let job = WriterJob {
            writer,
            source,
            generation,
            rate_hz: self.limits.rate_hz,
            actor: self.tx.clone(),
            hub: self.deps.db.hub.clone(),
            cadence: self.cadence,
        };
        match writer::spawn(job) {
            Ok(thread) => {
                take.writer = Writer::Running { capture, thread };
                true
            }
            Err(error) => {
                // The job, and the writer in it, went down with the failed spawn.
                capture.stop();
                self.deps
                    .db
                    .publish(Event::error("starting the take writer", error));
                false
            }
        }
    }

    /// Releases the device, then waits for the writer thread to drain the ring and hand the
    /// writer back.
    fn stop_capture(&mut self) {
        let Some(take) = self.take.as_mut() else {
            return;
        };
        take.writer = match std::mem::replace(&mut take.writer, Writer::Lost) {
            Writer::Running { capture, thread } => {
                capture.stop();
                match thread.join() {
                    Ok(writer) => Writer::Parked(writer),
                    Err(_) => {
                        self.deps.db.publish(Event::error(
                            "writing the take",
                            "the take writer thread panicked",
                        ));
                        Writer::Lost
                    }
                }
            }
            other => other,
        };
    }

    /// Closes the take here, then seals and commits it on a short-lived thread so the next
    /// recording can start at once.
    fn finalize(&mut self, reason: FinalizeReason) {
        self.stop_capture();
        let Some(take) = self.take.take() else {
            return;
        };
        self.armed = None;
        self.publish(Event::Finished {
            id: take.id.clone(),
            reason,
        });
        let closed = match take.writer {
            Writer::Parked(writer) => Some(writer.finish()),
            Writer::Running { .. } | Writer::Lost => None,
        };
        let seal = Seal {
            id: take.id,
            closed,
            db: self.deps.db.clone(),
            takes: Arc::clone(&self.deps.takes),
            clock: Arc::clone(&self.deps.clock),
            nudge: self.deps.nudge.clone(),
        };
        match std::thread::Builder::new()
            .name("dory-seal".into())
            .spawn(move || seal.run())
        {
            Ok(thread) => self.sealing.push(thread),
            Err(error) => self.publish(Event::error("sealing the take", error)),
        }
    }

    fn discard(&mut self) {
        self.stop_capture();
        let Some(take) = self.take.take() else {
            return;
        };
        if let Writer::Parked(writer) = take.writer {
            let _ = writer.finish();
        }
        // Audio goes after the row commits, so a crash in between leaves an orphan that startup
        // deletes rather than a row pointing at nothing.
        if let Err(error) = self
            .deps
            .db
            .apply(vec![Change::RecordingDeleted(take.id.clone())])
        {
            self.publish(Event::error("discarding the take", error));
            return;
        }
        if let Err(error) = self.deps.takes.delete(&take.id) {
            self.publish(Event::error("discarding the take", error));
        }
    }

    fn sync_status(&mut self) {
        let id = self.take.as_ref().map(|t| t.id.clone());
        let status = match (self.state, id) {
            (State::Recording { recorded, .. }, Some(id)) => {
                RecorderStatus::Recording { id, recorded }
            }
            (State::Paused { recorded, .. }, Some(id)) => RecorderStatus::Paused { id, recorded },
            (State::Grace { recorded, .. }, Some(id)) => RecorderStatus::Grace { id, recorded },
            _ => RecorderStatus::Idle,
        };
        *lock(&self.status) = status.clone();
        if !status.same_phase(&self.published) {
            self.published = status.clone();
            self.publish(Event::Recorder(status));
        }
    }

    fn publish(&self, event: Event) {
        self.deps.db.publish(event);
    }
}

struct Seal {
    id: RecordingId,
    /// The result of closing the WAV, or `None` when the writer was lost.
    closed: Option<Result<TakeInfo, TakeError>>,
    db: Db,
    takes: Arc<dyn Takes>,
    clock: Arc<dyn Clock>,
    nudge: Nudge,
}

impl Seal {
    fn run(self) {
        if let Some(Err(error)) = &self.closed {
            self.db.publish(Event::error("closing the take", error));
        }
        match self.takes.seal(&self.id) {
            Ok(info) => {
                if let Err(error) = commit_sealed(&self.db, &self.id, info, self.clock.as_ref()) {
                    self.db.publish(Event::error("saving the recording", error));
                }
                self.nudge.wake();
            }
            Err(error) => {
                self.db.publish(Event::error("sealing the take", error));
                // List it with the WAV's length. Startup seals it, because it is still an orphan.
                if let Some(Ok(wav)) = self.closed {
                    let _ = self.db.apply(vec![Change::RecordingEdited {
                        id: self.id,
                        edit: RecordingEdit::Take(wav),
                    }]);
                }
            }
        }
    }
}

/// Records the sealed take and, when settings ask for it, queues processing in the same commit,
/// so the pipeline never sees a job for audio that is not sealed yet.
pub(crate) fn commit_sealed(
    db: &Db,
    id: &RecordingId,
    info: TakeInfo,
    clock: &dyn Clock,
) -> Result<(), StoreError> {
    let mut changes = vec![Change::RecordingEdited {
        id: id.clone(),
        edit: RecordingEdit::Take(info),
    }];
    if db.store.settings()?.auto_process && db.store.jobs_for(id)?.is_empty() {
        changes.push(Change::JobEnqueued {
            id: id.clone(),
            goal: Goal::ALL,
            priority: Priority::Normal,
            at: clock.now(),
        });
    }
    db.apply(changes)
}
