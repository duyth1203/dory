//! The pipeline job and its state machine. [`Job::on`] is the only place a transition is decided.
//! The store applies it when it commits a `Change::Job`, the worker uses it to predict, and the
//! in-memory fake uses it too, so there is one rule set.

use thiserror::Error;

use super::{FailureCause, FailureClass, JobId, RecordingId, Timestamp};
use crate::models::{KindTag, Mismatch, ModelRef, Unavailable};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Transcribe,
    Summarize,
}

impl Stage {
    /// Stages run in this order.
    pub const ORDER: [Stage; 2] = [Stage::Transcribe, Stage::Summarize];
}

/// How many times a stage may start before an interruption fails the job instead of requeueing
/// it. A stage that kills the process would otherwise be restarted on every launch forever.
pub const MAX_ATTEMPTS: u8 = 3;

/// Which stages a job should produce. Never empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Goal {
    transcribe: bool,
    summarize: bool,
}

impl Goal {
    pub const ALL: Goal = Goal {
        transcribe: true,
        summarize: true,
    };
    pub const TRANSCRIBE: Goal = Goal {
        transcribe: true,
        summarize: false,
    };
    pub const SUMMARIZE: Goal = Goal {
        transcribe: false,
        summarize: true,
    };

    pub fn new(transcribe: bool, summarize: bool) -> Option<Goal> {
        (transcribe || summarize).then_some(Goal {
            transcribe,
            summarize,
        })
    }

    pub fn wants(self, stage: Stage) -> bool {
        match stage {
            Stage::Transcribe => self.transcribe,
            Stage::Summarize => self.summarize,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Normal,
    /// The user moved it to the front of the queue.
    Front,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: JobId,
    pub recording: RecordingId,
    pub goal: Goal,
    /// Stages finished, in the order they finished. No duplicates.
    pub done: Vec<Stage>,
    pub state: JobState,
    pub priority: Priority,
    /// The user chose Use Fast This Time (UX-30): this job transcribes at Fast whatever the
    /// setting and the Mac's conditions. Other jobs keep following the setting.
    pub run_fast: bool,
    /// Starts of the current stage. Reset when a stage finishes or the user retries.
    pub attempts: u8,
    /// Retry backoff. The worker skips the job until then.
    pub not_before: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running {
        stage: Stage,
    },
    /// Needs something only the user can supply. Leaves through `Unblock`, `Retake`, or `Cancelled`.
    Blocked {
        stage: Stage,
        blocker: Blocker,
    },
    Succeeded,
    Failed {
        stage: Stage,
        error: JobError,
    },
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobError {
    pub class: FailureClass,
    pub message: String,
    pub cause: FailureCause,
}

/// Why a stage cannot proceed without the user. Persisted, so the UI can offer the fix after a
/// restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// The model is not on this machine. Provisioning is a user action, never automatic.
    NeedsProvision {
        model: ModelRef,
        bytes: Option<u64>,
    },
    Unavailable {
        model: ModelRef,
        why: Unavailable,
    },
    /// The model cannot do what this recording needs, and the user can change the choice or the
    /// recording's language.
    Incompatible {
        model: ModelRef,
        why: Mismatch,
    },
    /// The catalog holds nothing for this kind, or the choice is Off where a model is required.
    NoModel {
        kind: KindTag,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEvent {
    Started {
        stage: Stage,
    },
    StageDone {
        stage: Stage,
    },
    Blocked {
        stage: Stage,
        blocker: Blocker,
    },
    Failed {
        stage: Stage,
        error: JobError,
    },
    /// Back to the queue after a transient failure, not before `not_before`.
    Retry {
        not_before: Timestamp,
    },
    Cancelled,
    /// A restart found the job Running, so the process died mid-stage. Requeues the job until
    /// [`MAX_ATTEMPTS`] starts have been cut short, then fails it.
    Interrupted,
    Unblock,
    /// The user tries a failed, cancelled, or blocked job again.
    Retake,
    /// Move to the front of the queue. Accepted while the job runs or waits so a click that races
    /// the worker is not an error, and the wish holds when the job is queued again. Illegal once
    /// the job is terminal.
    Promote,
    /// Use Fast This Time. Accepted while the job runs or waits, like `Promote`, and kept when
    /// the job is queued again.
    RunFast,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("illegal job transition: {event:?} while {from:?}")]
pub struct IllegalTransition {
    pub from: Box<JobState>,
    pub event: Box<JobEvent>,
}

impl Job {
    pub fn new(
        id: JobId,
        recording: RecordingId,
        goal: Goal,
        priority: Priority,
        created_at: Timestamp,
    ) -> Job {
        Job {
            id,
            recording,
            goal,
            done: Vec::new(),
            state: JobState::Queued,
            priority,
            run_fast: false,
            attempts: 0,
            not_before: None,
            created_at,
        }
    }

    /// The first stage the goal wants that is not done, or `None` when the goal is met.
    pub fn next_stage(&self) -> Option<Stage> {
        Stage::ORDER
            .into_iter()
            .find(|s| self.goal.wants(*s) && !self.done.contains(s))
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            JobState::Succeeded | JobState::Failed { .. } | JobState::Cancelled
        )
    }

    /// The only transition function. The outer match is exhaustive over states, so a new state
    /// cannot compile until it says what it accepts.
    pub fn on(&self, event: &JobEvent) -> Result<Job, IllegalTransition> {
        use JobEvent as E;
        use JobState as S;
        let mut next = self.clone();
        let legal = match &self.state {
            S::Queued => match event {
                E::Started { stage } if self.next_stage() == Some(*stage) => {
                    next.state = S::Running { stage: *stage };
                    next.attempts = self.attempts.saturating_add(1);
                    next.not_before = None;
                    true
                }
                E::Promote => {
                    next.priority = Priority::Front;
                    true
                }
                E::RunFast => {
                    next.run_fast = true;
                    true
                }
                E::Cancelled => {
                    next.state = S::Cancelled;
                    true
                }
                _ => false,
            },
            S::Running { stage } => match event {
                E::StageDone { stage: done } if done == stage => {
                    next.done.push(*done);
                    next.attempts = 0;
                    next.state = if next.next_stage().is_some() {
                        S::Queued
                    } else {
                        S::Succeeded
                    };
                    true
                }
                E::Blocked { stage: at, blocker } if at == stage => {
                    next.state = S::Blocked {
                        stage: *at,
                        blocker: blocker.clone(),
                    };
                    true
                }
                E::Failed { stage: at, error } if at == stage => {
                    next.state = S::Failed {
                        stage: *at,
                        error: error.clone(),
                    };
                    true
                }
                E::Retry { not_before } => {
                    next.state = S::Queued;
                    next.not_before = Some(*not_before);
                    true
                }
                E::Interrupted => {
                    // `attempts` already counts the start that was cut short.
                    next.state = if self.attempts >= MAX_ATTEMPTS {
                        S::Failed {
                            stage: *stage,
                            error: JobError {
                                class: FailureClass::Permanent,
                                message: format!(
                                    "interrupted {} times while running, the process stopped before the stage finished",
                                    self.attempts
                                ),
                                cause: FailureCause::Crashed,
                            },
                        }
                    } else {
                        S::Queued
                    };
                    true
                }
                E::Promote => {
                    next.priority = Priority::Front;
                    true
                }
                E::RunFast => {
                    next.run_fast = true;
                    true
                }
                E::Cancelled => {
                    next.state = S::Cancelled;
                    true
                }
                _ => false,
            },
            S::Blocked { .. } => match event {
                E::Promote => {
                    next.priority = Priority::Front;
                    true
                }
                E::RunFast => {
                    next.run_fast = true;
                    true
                }
                E::Unblock => {
                    // A blocked stage never got to do its work, so waiting is not a failed attempt.
                    next.state = S::Queued;
                    next.attempts = 0;
                    true
                }
                E::Retake => {
                    next.retake();
                    true
                }
                E::Cancelled => {
                    next.state = S::Cancelled;
                    true
                }
                _ => false,
            },
            S::Failed { .. } | S::Cancelled => match event {
                E::Retake => {
                    next.retake();
                    true
                }
                _ => false,
            },
            S::Succeeded => false,
        };
        if legal {
            Ok(next)
        } else {
            Err(IllegalTransition {
                from: Box::new(self.state.clone()),
                event: Box::new(event.clone()),
            })
        }
    }

    fn retake(&mut self) {
        self.state = JobState::Queued;
        self.attempts = 0;
        self.not_before = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Stage = Stage::Transcribe;
    const S: Stage = Stage::Summarize;

    fn job(goal: Goal, done: &[Stage], state: JobState) -> Job {
        Job {
            id: JobId(1),
            recording: RecordingId::parse("rec").unwrap(),
            goal,
            done: done.to_vec(),
            state,
            priority: Priority::Normal,
            run_fast: false,
            attempts: 0,
            not_before: None,
            created_at: Timestamp(0),
        }
    }

    fn blocker() -> Blocker {
        Blocker::NoModel { kind: KindTag::Stt }
    }

    fn error() -> JobError {
        JobError {
            class: FailureClass::Permanent,
            message: "boom".into(),
            cause: FailureCause::Other,
        }
    }

    fn running(stage: Stage) -> JobState {
        JobState::Running { stage }
    }

    /// The job every table row starts from. Nonzero attempts, a pending backoff, and a stage
    /// already in `done` would all show if a transition resets or keeps them by mistake.
    fn base(state: JobState) -> Job {
        Job {
            attempts: 1,
            not_before: Some(Timestamp(77)),
            ..job(Goal::ALL, &[], state)
        }
    }

    /// The whole job a transition must produce, or `None` when it must be refused. Written out
    /// per row so the table is the spec and not a copy of `Job::on`.
    fn expected(before: &Job, event: &JobEvent) -> Option<Job> {
        use JobEvent as E;
        use JobState as St;
        let with = |f: &dyn Fn(&mut Job)| {
            let mut j = before.clone();
            f(&mut j);
            Some(j)
        };
        match (&before.state, event) {
            (St::Queued, E::Started { stage: T }) => with(&|j| {
                j.state = running(T);
                j.attempts = 2;
                j.not_before = None;
            }),
            (St::Queued | St::Running { .. } | St::Blocked { .. }, E::Promote) => {
                with(&|j| j.priority = Priority::Front)
            }
            (St::Queued | St::Running { .. } | St::Blocked { .. }, E::RunFast) => {
                with(&|j| j.run_fast = true)
            }
            (St::Queued, E::Cancelled) => with(&|j| j.state = St::Cancelled),

            (St::Running { stage }, E::StageDone { stage: done }) if stage == done => with(&|j| {
                j.state = St::Queued;
                j.done = vec![*done];
                j.attempts = 0;
            }),
            (
                St::Running { stage },
                E::Blocked {
                    stage: at,
                    blocker: b,
                },
            ) if stage == at => with(&|j| {
                j.state = St::Blocked {
                    stage: *at,
                    blocker: b.clone(),
                }
            }),
            (
                St::Running { stage },
                E::Failed {
                    stage: at,
                    error: e,
                },
            ) if stage == at => with(&|j| {
                j.state = St::Failed {
                    stage: *at,
                    error: e.clone(),
                }
            }),
            (St::Running { .. }, E::Retry { not_before }) => with(&|j| {
                j.state = St::Queued;
                j.not_before = Some(*not_before);
            }),
            (St::Running { .. }, E::Interrupted) => with(&|j| j.state = St::Queued),
            (St::Running { .. }, E::Cancelled) => with(&|j| j.state = St::Cancelled),

            (St::Blocked { .. }, E::Unblock) => with(&|j| {
                j.state = St::Queued;
                j.attempts = 0;
            }),
            (St::Blocked { .. } | St::Failed { .. } | St::Cancelled, E::Retake) => with(&|j| {
                j.state = St::Queued;
                j.attempts = 0;
                j.not_before = None;
            }),
            (St::Blocked { .. }, E::Cancelled) => with(&|j| j.state = St::Cancelled),
            _ => None,
        }
    }

    /// One index per variant. The matches have no wildcard, so adding a variant stops compiling
    /// here, and `every_variant_is_in_the_table` then fails until the lists below include it.
    fn state_index(s: &JobState) -> usize {
        match s {
            JobState::Queued => 0,
            JobState::Running { .. } => 1,
            JobState::Blocked { .. } => 2,
            JobState::Succeeded => 3,
            JobState::Failed { .. } => 4,
            JobState::Cancelled => 5,
        }
    }
    const STATE_VARIANTS: usize = 6;

    fn event_index(e: &JobEvent) -> usize {
        match e {
            JobEvent::Started { .. } => 0,
            JobEvent::StageDone { .. } => 1,
            JobEvent::Blocked { .. } => 2,
            JobEvent::Failed { .. } => 3,
            JobEvent::Retry { .. } => 4,
            JobEvent::Cancelled => 5,
            JobEvent::Interrupted => 6,
            JobEvent::Unblock => 7,
            JobEvent::Retake => 8,
            JobEvent::Promote => 9,
            JobEvent::RunFast => 10,
        }
    }
    const EVENT_VARIANTS: usize = 11;

    fn all_states() -> Vec<JobState> {
        vec![
            JobState::Queued,
            running(T),
            JobState::Blocked {
                stage: T,
                blocker: blocker(),
            },
            JobState::Succeeded,
            JobState::Failed {
                stage: T,
                error: error(),
            },
            JobState::Cancelled,
        ]
    }

    fn all_events() -> Vec<JobEvent> {
        vec![
            JobEvent::Started { stage: T },
            JobEvent::Started { stage: S },
            JobEvent::StageDone { stage: T },
            JobEvent::StageDone { stage: S },
            JobEvent::Blocked {
                stage: T,
                blocker: blocker(),
            },
            JobEvent::Blocked {
                stage: S,
                blocker: blocker(),
            },
            JobEvent::Failed {
                stage: T,
                error: error(),
            },
            JobEvent::Failed {
                stage: S,
                error: error(),
            },
            JobEvent::Retry {
                not_before: Timestamp(500),
            },
            JobEvent::Cancelled,
            JobEvent::Interrupted,
            JobEvent::Unblock,
            JobEvent::Retake,
            JobEvent::Promote,
            JobEvent::RunFast,
        ]
    }

    #[test]
    fn every_variant_is_in_the_table() {
        let states: std::collections::BTreeSet<_> = all_states().iter().map(state_index).collect();
        let events: std::collections::BTreeSet<_> = all_events().iter().map(event_index).collect();
        assert_eq!(
            states,
            (0..STATE_VARIANTS).collect(),
            "a JobState is missing from all_states"
        );
        assert_eq!(
            events,
            (0..EVENT_VARIANTS).collect(),
            "a JobEvent is missing from all_events"
        );
    }

    #[test]
    fn full_state_event_table() {
        for state in all_states() {
            for event in all_events() {
                let before = base(state.clone());
                let got = before.on(&event);
                match expected(&before, &event) {
                    Some(want) => {
                        let after = got.unwrap_or_else(|e| {
                            panic!("{state:?} + {event:?} should be legal: {e}")
                        });
                        assert_eq!(after, want, "{state:?} + {event:?}");
                    }
                    None => {
                        let err =
                            got.expect_err(&format!("{state:?} + {event:?} should be illegal"));
                        assert_eq!(
                            err,
                            IllegalTransition {
                                from: Box::new(state.clone()),
                                event: Box::new(event.clone())
                            }
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_attempt_cap_is_the_only_thing_that_turns_interrupted_into_failed() {
        let at_cap = Job {
            attempts: MAX_ATTEMPTS,
            ..base(running(T))
        };
        let failed = at_cap.on(&JobEvent::Interrupted).unwrap();
        assert!(matches!(failed.state, JobState::Failed { stage: T, .. }));
        assert_eq!(failed.attempts, MAX_ATTEMPTS, "the count stays for the UI");

        let below = Job {
            attempts: MAX_ATTEMPTS - 1,
            ..base(running(T))
        };
        assert_eq!(
            below.on(&JobEvent::Interrupted).unwrap().state,
            JobState::Queued
        );
    }

    #[test]
    fn illegal_transition_leaves_the_job_untouched() {
        let before = job(Goal::ALL, &[], JobState::Succeeded);
        let snapshot = before.clone();
        assert!(before.on(&JobEvent::Retake).is_err());
        assert_eq!(before, snapshot);
    }

    #[test]
    fn next_stage_table() {
        let cases = [
            (Goal::ALL, vec![], Some(T)),
            (Goal::ALL, vec![T], Some(S)),
            (Goal::ALL, vec![T, S], None),
            (Goal::ALL, vec![S], Some(T)),
            (Goal::TRANSCRIBE, vec![], Some(T)),
            (Goal::TRANSCRIBE, vec![T], None),
            (Goal::SUMMARIZE, vec![], Some(S)),
            (Goal::SUMMARIZE, vec![S], None),
        ];
        for (goal, done, want) in cases {
            assert_eq!(
                job(goal, &done, JobState::Queued).next_stage(),
                want,
                "{goal:?} done {done:?}"
            );
        }
    }

    #[test]
    fn goal_is_never_empty() {
        assert_eq!(Goal::new(false, false), None);
        assert_eq!(Goal::new(true, true), Some(Goal::ALL));
        assert_eq!(Goal::new(false, true), Some(Goal::SUMMARIZE));
    }

    #[test]
    fn a_job_walks_both_stages_to_success() {
        let mut j = job(Goal::ALL, &[], JobState::Queued);
        for stage in [T, S] {
            j = j.on(&JobEvent::Started { stage }).unwrap();
            assert_eq!(j.state, running(stage));
            j = j.on(&JobEvent::StageDone { stage }).unwrap();
        }
        assert_eq!(j.state, JobState::Succeeded);
        assert_eq!(j.done, vec![T, S]);
        assert!(j.is_terminal());
    }

    #[test]
    fn started_must_be_the_next_stage() {
        let j = job(Goal::ALL, &[], JobState::Queued);
        assert!(j.on(&JobEvent::Started { stage: S }).is_err());
        let j = job(Goal::TRANSCRIBE, &[T], JobState::Queued);
        assert!(j.on(&JobEvent::Started { stage: T }).is_err());
    }

    #[test]
    fn attempts_count_starts_and_reset_on_stage_done_and_retake() {
        let mut j = job(Goal::ALL, &[], JobState::Queued);
        j = j.on(&JobEvent::Started { stage: T }).unwrap();
        j = j
            .on(&JobEvent::Retry {
                not_before: Timestamp(900),
            })
            .unwrap();
        assert_eq!((j.attempts, j.not_before), (1, Some(Timestamp(900))));
        j = j.on(&JobEvent::Started { stage: T }).unwrap();
        assert_eq!((j.attempts, j.not_before), (2, None));
        j = j
            .on(&JobEvent::Failed {
                stage: T,
                error: error(),
            })
            .unwrap();
        j = j.on(&JobEvent::Retake).unwrap();
        assert_eq!(j.attempts, 0);
        j = j.on(&JobEvent::Started { stage: T }).unwrap();
        j = j.on(&JobEvent::StageDone { stage: T }).unwrap();
        assert_eq!((j.attempts, j.state.clone()), (0, JobState::Queued));
    }

    #[test]
    fn blocked_keeps_the_blocker_for_the_ui() {
        let j = job(Goal::ALL, &[], running(T));
        let b = Blocker::NeedsProvision {
            model: ModelRef::parse("sona/whisper-small").unwrap(),
            bytes: Some(10),
        };
        let blocked = j
            .on(&JobEvent::Blocked {
                stage: T,
                blocker: b.clone(),
            })
            .unwrap();
        assert_eq!(
            blocked.state,
            JobState::Blocked {
                stage: T,
                blocker: b
            }
        );
    }

    #[test]
    fn promote_is_idempotent_and_retake_keeps_finished_stages() {
        let j = job(Goal::ALL, &[], JobState::Queued);
        let once = j.on(&JobEvent::Promote).unwrap();
        assert_eq!(once.on(&JobEvent::Promote).unwrap(), once);
        assert_eq!(once.priority, Priority::Front);

        let failed = job(
            Goal::ALL,
            &[T],
            JobState::Failed {
                stage: S,
                error: error(),
            },
        );
        let again = failed.on(&JobEvent::Retake).unwrap();
        assert_eq!((again.done.clone(), again.next_stage()), (vec![T], Some(S)));
    }

    #[test]
    fn a_stage_that_keeps_killing_the_process_fails_after_the_attempt_cap() {
        let mut j = job(Goal::ALL, &[], JobState::Queued);
        for attempt in 1..=MAX_ATTEMPTS {
            j = j.on(&JobEvent::Started { stage: T }).unwrap();
            assert_eq!(j.attempts, attempt);
            j = j.on(&JobEvent::Interrupted).unwrap();
            if attempt < MAX_ATTEMPTS {
                assert_eq!(j.state, JobState::Queued, "attempt {attempt} may run again");
            }
        }
        let JobState::Failed { stage, error } = &j.state else {
            panic!(
                "expected Failed after {MAX_ATTEMPTS} interrupted starts, got {:?}",
                j.state
            );
        };
        assert_eq!(*stage, T);
        assert_eq!(error.class, FailureClass::Permanent);
        assert_eq!(error.cause, FailureCause::Crashed);
        assert!(error.message.contains("interrupted"), "{}", error.message);
        assert!(j.is_terminal());

        let again = j.on(&JobEvent::Retake).unwrap();
        assert_eq!((again.state, again.attempts), (JobState::Queued, 0));
    }

    #[test]
    fn a_finished_stage_gives_the_next_one_a_fresh_attempt_budget() {
        let mut j = job(Goal::ALL, &[], JobState::Queued);
        for _ in 1..MAX_ATTEMPTS {
            j = j.on(&JobEvent::Started { stage: T }).unwrap();
            j = j.on(&JobEvent::Interrupted).unwrap();
        }
        j = j.on(&JobEvent::Started { stage: T }).unwrap();
        j = j.on(&JobEvent::StageDone { stage: T }).unwrap();
        j = j.on(&JobEvent::Started { stage: S }).unwrap();
        j = j.on(&JobEvent::Interrupted).unwrap();
        assert_eq!((j.state, j.attempts), (JobState::Queued, 1));
    }

    #[test]
    fn promote_is_accepted_while_the_job_runs_or_waits_and_is_remembered() {
        for state in [
            running(T),
            JobState::Blocked {
                stage: T,
                blocker: blocker(),
            },
        ] {
            let promoted = job(Goal::ALL, &[], state.clone())
                .on(&JobEvent::Promote)
                .unwrap_or_else(|e| panic!("{state:?}: {e}"));
            assert_eq!(
                (&promoted.state, promoted.priority),
                (&state, Priority::Front)
            );
        }
        let queued_again = job(
            Goal::ALL,
            &[],
            JobState::Blocked {
                stage: T,
                blocker: blocker(),
            },
        )
        .on(&JobEvent::Promote)
        .and_then(|j| j.on(&JobEvent::Unblock))
        .unwrap();
        assert_eq!(
            (queued_again.state, queued_again.priority),
            (JobState::Queued, Priority::Front),
            "the user's wish to go first survives the wait"
        );
        for state in [JobState::Succeeded, JobState::Cancelled] {
            assert!(job(Goal::ALL, &[], state).on(&JobEvent::Promote).is_err());
        }
    }

    #[test]
    fn run_fast_survives_a_retry_and_a_wait() {
        let fast = job(Goal::ALL, &[], running(T))
            .on(&JobEvent::RunFast)
            .and_then(|j| {
                j.on(&JobEvent::Retry {
                    not_before: Timestamp(5),
                })
            })
            .and_then(|j| j.on(&JobEvent::RunFast))
            .unwrap();
        assert_eq!((fast.state, fast.run_fast), (JobState::Queued, true));
    }

    #[test]
    fn waiting_on_a_blocker_never_uses_up_attempts() {
        let mut j = job(Goal::ALL, &[], JobState::Queued);
        for _ in 0..(MAX_ATTEMPTS * 3) {
            j = j
                .on(&JobEvent::Started { stage: T })
                .and_then(|j| {
                    j.on(&JobEvent::Blocked {
                        stage: T,
                        blocker: blocker(),
                    })
                })
                .and_then(|j| j.on(&JobEvent::Unblock))
                .unwrap();
        }
        let interrupted = j
            .on(&JobEvent::Started { stage: T })
            .and_then(|j| j.on(&JobEvent::Interrupted))
            .unwrap();
        assert_eq!(interrupted.state, JobState::Queued);
    }
}
