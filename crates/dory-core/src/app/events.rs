//! Events are hints that something changed. Persisted-state events come only from a committed
//! change list, through [`Db::apply`], so no actor can write a fact without announcing it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_channel::{Receiver, Sender, TrySendError};

use super::pace::Budget;
use super::recorder::RecorderStatus;
use crate::domain::{AssistantChange, Fraction, JobId, Ms, RecordingId, Segment, Stage};
use crate::models::{KindTag, ModelRef};
use crate::ports::capture::CaptureIssue;
use crate::ports::{Change, Store, StoreError};
use crate::recorder::{FinalizeReason, Input};

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    RecordingAdded(RecordingId),
    RecordingChanged(RecordingId),
    RecordingDeleted(RecordingId),
    TranscriptChanged(RecordingId),
    SummaryChanged(RecordingId),
    SpeakerNamesChanged(RecordingId),
    JobEnqueued(RecordingId),
    JobChanged(JobId),
    SettingsChanged,

    /// The recorder moved to another phase or another take. Recorded time alone moving is carried
    /// by [`Event::Levels`].
    Recorder(RecorderStatus),
    /// Per-channel peaks since the last event, about ten times a second while capturing.
    Levels {
        peaks: Vec<f32>,
        recorded: Ms,
    },
    TimeWarning {
        remaining: Ms,
    },
    CaptureIssue(CaptureIssue),
    /// A recorder command that does not apply in the current state.
    Rejected(Input),
    /// The take was closed. Sealing and enqueueing follow on their own thread.
    Finished {
        id: RecordingId,
        reason: FinalizeReason,
    },

    /// Also sent when a window starts, so a change of budget shows within one window. A stage
    /// that runs unlimited, like the summary, has no budget.
    StageProgress {
        job: JobId,
        stage: Stage,
        fraction: Fraction,
        budget: Option<Budget>,
    },
    /// A segment as the engine produced it, before the transcript commits.
    DraftSegment {
        job: JobId,
        segment: Segment,
    },
    /// A piece of summary text as the model streams it.
    DraftSummary {
        job: JobId,
        text: String,
    },
    /// The summary starts over with another model, so drop the draft streamed so far.
    DraftReset {
        job: JobId,
    },

    ProvisionProgress {
        model: ModelRef,
        step: String,
        done: u64,
        total: Option<u64>,
    },
    ProvisionFinished {
        model: ModelRef,
        result: Result<(), String>,
    },
    /// The setting named a model no provider offers anymore, so it went back to `Auto`.
    ChoiceReset {
        kind: KindTag,
        removed: ModelRef,
    },
    /// Background work failed where no caller is waiting for the result.
    Error {
        context: &'static str,
        message: String,
    },

    /// Some events were dropped. Re-read whatever is on screen.
    Resync,
}

impl Event {
    /// Events a slow subscriber may lose, because the next one supersedes them.
    pub fn is_droppable(&self) -> bool {
        matches!(
            self,
            Event::Levels { .. }
                | Event::StageProgress { .. }
                | Event::DraftSegment { .. }
                | Event::DraftSummary { .. }
                | Event::ProvisionProgress { .. }
        )
    }

    pub(crate) fn error(context: &'static str, error: impl std::fmt::Display) -> Event {
        Event::Error {
            context,
            message: error.to_string(),
        }
    }
}

/// The events a committed change list announces.
/// Checkpoints and saved translation batches announce nothing: they are the pipeline's own bookkeeping, and stage progress
/// already tells a UI how far a run got.
pub fn change_events(changes: &[Change]) -> Vec<Event> {
    changes
        .iter()
        .filter_map(|change| match change {
            Change::RecordingCreated(r) => Some(Event::RecordingAdded(r.id.clone())),
            Change::RecordingEdited { id, .. } => Some(Event::RecordingChanged(id.clone())),
            Change::RecordingDeleted(id) => Some(Event::RecordingDeleted(id.clone())),
            Change::TranscriptReplaced { id, .. } => Some(Event::TranscriptChanged(id.clone())),
            Change::SpeakerNamesReplaced { id, .. } => Some(Event::SpeakerNamesChanged(id.clone())),
            Change::SummaryReplaced { id, .. }
            | Change::SummaryRestored(id)
            | Change::SummaryTranslationWanted { id, .. }
            | Change::SummaryTranslated { id, .. } => Some(Event::SummaryChanged(id.clone())),
            Change::SummaryInstructionSet { id, .. } => Some(Event::RecordingChanged(id.clone())),
            Change::JobEnqueued { id, .. } => Some(Event::JobEnqueued(id.clone())),
            Change::Job { job, .. } => Some(Event::JobChanged(*job)),
            Change::SettingsReplaced(_) => Some(Event::SettingsChanged),
            Change::TranslationReplaced { id, .. } => Some(Event::TranscriptChanged(id.clone())),
            Change::CheckpointSaved { .. }
            | Change::CheckpointsCleared(_)
            | Change::TranslationBatchSaved { .. }
            | Change::TranslationBatchesCleared(_) => None,
            Change::Assistant(AssistantChange::RecordingHidden { id, .. }) => {
                Some(Event::RecordingChanged(id.clone()))
            }
            Change::Assistant(_) => Some(Event::SettingsChanged),
        })
        .collect()
}

/// Fans events out to subscribers. Each subscriber has its own bounded queue, and publishing
/// never waits on any of them.
#[derive(Clone)]
pub(crate) struct EventHub {
    inner: Arc<HubInner>,
}

struct HubInner {
    capacity: usize,
    subscribers: Mutex<Vec<Subscriber>>,
}

struct Subscriber {
    tx: Sender<Event>,
    lost: Arc<AtomicBool>,
    /// Wake-up `Resync`s in the queue.
    wakeups: Arc<AtomicUsize>,
}

impl Subscriber {
    /// Events queued, not counting wake-ups.
    fn queued(&self) -> usize {
        self.tx
            .len()
            .saturating_sub(self.wakeups.load(Ordering::Acquire))
    }

    /// Flags the loss, and on the first loss also queues a wake-up `Resync`. The flag alone would
    /// not wake a receiver that emptied the queue and parked just before it was set.
    fn lose(&self) {
        if !self.lost.swap(true, Ordering::AcqRel) {
            self.wakeups.fetch_add(1, Ordering::AcqRel);
            if self.tx.try_send(Event::Resync).is_err() {
                self.wakeups.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

impl EventHub {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(HubInner {
                capacity: capacity.max(4),
                subscribers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn subscribe(&self) -> Subscription {
        // Room past capacity for the wake-up `Resync`s that `lose` queues, which never count
        // against the events a subscriber may fall behind by.
        let (tx, rx) = async_channel::bounded(self.inner.capacity + 2);
        let lost = Arc::new(AtomicBool::new(false));
        let wakeups = Arc::new(AtomicUsize::new(0));
        self.subscribers().push(Subscriber {
            tx,
            lost: Arc::clone(&lost),
            wakeups: Arc::clone(&wakeups),
        });
        Subscription { rx, lost, wakeups }
    }

    pub fn publish(&self, event: Event) {
        // Droppable events stop short of the last quarter of the queue, so a burst of levels
        // cannot crowd out a state change.
        let capacity = self.inner.capacity;
        let limit = match event.is_droppable() {
            true => capacity - capacity / 4,
            false => capacity,
        };
        let mut subscribers = self.subscribers();
        subscribers.retain(|s| !s.tx.is_closed());
        for subscriber in subscribers.iter() {
            // Publishers hold the lock, so only the subscriber changes the length meanwhile, and
            // only downward.
            if subscriber.queued() >= limit {
                subscriber.lose();
            } else if let Err(TrySendError::Full(_)) = subscriber.tx.try_send(event.clone()) {
                subscriber.lose();
            }
        }
    }

    fn subscribers(&self) -> std::sync::MutexGuard<'_, Vec<Subscriber>> {
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// One subscriber's queue. After a loss the next receive returns [`Event::Resync`] before the
/// events still queued, once per loss. Receives return `None` once `Dory` has shut down and the
/// queue is empty.
pub struct Subscription {
    rx: Receiver<Event>,
    lost: Arc<AtomicBool>,
    wakeups: Arc<AtomicUsize>,
}

impl Subscription {
    pub fn try_recv(&self) -> Option<Event> {
        loop {
            if let Some(resync) = self.take_resync() {
                return Some(resync);
            }
            if let Some(event) = self.unless_wakeup(self.rx.try_recv().ok()?) {
                return Some(event);
            }
        }
    }

    pub fn recv_blocking(&self) -> Option<Event> {
        loop {
            if let Some(resync) = self.take_resync() {
                return Some(resync);
            }
            if let Some(event) = self.unless_wakeup(self.rx.recv_blocking().ok()?) {
                return Some(event);
            }
        }
    }

    pub async fn recv(&self) -> Option<Event> {
        loop {
            if let Some(resync) = self.take_resync() {
                return Some(resync);
            }
            if let Some(event) = self.unless_wakeup(self.rx.recv().await.ok()?) {
                return Some(event);
            }
        }
    }

    /// A queued `Resync` is only ever a wake-up: no publisher sends one otherwise.
    fn unless_wakeup(&self, event: Event) -> Option<Event> {
        match event {
            Event::Resync => {
                self.wakeups.fetch_sub(1, Ordering::AcqRel);
                None
            }
            event => Some(event),
        }
    }

    /// The queued `Resync` only wakes the receiver. The flag is what reports the loss, so a
    /// queued one whose loss was already reported is skipped above.
    fn take_resync(&self) -> Option<Event> {
        self.lost
            .swap(false, Ordering::AcqRel)
            .then_some(Event::Resync)
    }
}

/// The one write path: commits a change list and announces it.
#[derive(Clone)]
pub(crate) struct Db {
    pub store: Arc<dyn Store>,
    pub hub: EventHub,
}

impl Db {
    pub fn apply(&self, changes: Vec<Change>) -> Result<(), StoreError> {
        let events = change_events(&changes);
        self.store.apply(changes)?;
        for event in events {
            self.hub.publish(event);
        }
        Ok(())
    }

    pub fn publish(&self, event: Event) {
        self.hub.publish(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(n: u64) -> Event {
        Event::Levels {
            peaks: vec![0.5],
            recorded: Ms(n),
        }
    }

    fn id() -> RecordingId {
        RecordingId::parse("r").unwrap()
    }

    fn drain(sub: &Subscription) -> Vec<Event> {
        std::iter::from_fn(|| sub.try_recv()).collect()
    }

    #[test]
    fn a_slow_subscriber_loses_levels_keeps_state_changes_and_gets_resync() {
        let hub = EventHub::new(8);
        let slow = hub.subscribe();
        let fast = hub.subscribe();
        let mut fast_seen = Vec::new();
        for n in 0..100 {
            hub.publish(level(n));
            fast_seen.extend(drain(&fast));
        }
        hub.publish(Event::TranscriptChanged(id()));
        hub.publish(Event::SettingsChanged);
        fast_seen.extend(drain(&fast));

        let got = drain(&slow);
        assert_eq!(got.first(), Some(&Event::Resync));
        assert!(got.contains(&Event::TranscriptChanged(id())));
        assert!(got.contains(&Event::SettingsChanged));
        assert!(
            got.len() <= 9,
            "bounded queue plus one resync, got {}",
            got.len()
        );

        assert_eq!(
            fast_seen.len(),
            102,
            "a subscriber that keeps up loses nothing"
        );
        assert!(!fast_seen.contains(&Event::Resync));
    }

    #[test]
    fn a_full_queue_drops_even_state_changes_and_reports_resync_once() {
        let hub = EventHub::new(4);
        let sub = hub.subscribe();
        for _ in 0..10 {
            hub.publish(Event::SettingsChanged);
        }
        let got = drain(&sub);
        assert_eq!(got[0], Event::Resync);
        assert_eq!(got.iter().filter(|e| **e == Event::Resync).count(), 1);
        assert_eq!(got.len(), 5);
        hub.publish(Event::SettingsChanged);
        assert_eq!(drain(&sub), vec![Event::SettingsChanged]);
    }

    #[test]
    fn a_loss_reaches_a_receiver_that_checked_the_flag_before_it_was_set() {
        let hub = EventHub::new(4);
        let sub = hub.subscribe();
        // A receive checks the flag, finds nothing, and is about to wait on the queue. Meanwhile
        // the queue fills and an event is lost.
        assert_eq!(sub.take_resync(), None);
        for _ in 0..5 {
            hub.publish(Event::SettingsChanged);
        }
        // The wait on the queue alone must surface the loss, or the receiver parks past it.
        let queued: Vec<_> = std::iter::from_fn(|| sub.rx.try_recv().ok()).collect();
        assert_eq!(queued.len(), 5, "four events and a wake-up: {queued:?}");
        assert_eq!(queued.last(), Some(&Event::Resync));
    }

    #[test]
    fn dropped_subscriptions_are_forgotten_and_closing_ends_receives() {
        let hub = EventHub::new(4);
        drop(hub.subscribe());
        let sub = hub.subscribe();
        hub.publish(Event::SettingsChanged);
        assert_eq!(hub.subscribers().len(), 1);
        drop(hub);
        assert_eq!(sub.recv_blocking(), Some(Event::SettingsChanged));
        assert_eq!(sub.recv_blocking(), None);
    }

    #[test]
    fn every_change_kind_announces_its_event() {
        use crate::domain::{Goal, JobEvent, Priority, Settings, Timestamp};
        let changes = vec![
            Change::RecordingDeleted(id()),
            Change::JobEnqueued {
                id: id(),
                goal: Goal::ALL,
                priority: Priority::Normal,
                picks: Default::default(),
                at: Timestamp(0),
            },
            Change::Job {
                job: JobId(4),
                event: JobEvent::Cancelled,
            },
            Change::SettingsReplaced(Settings::default()),
        ];
        assert_eq!(
            change_events(&changes),
            vec![
                Event::RecordingDeleted(id()),
                Event::JobEnqueued(id()),
                Event::JobChanged(JobId(4)),
                Event::SettingsChanged,
            ]
        );
    }
}
