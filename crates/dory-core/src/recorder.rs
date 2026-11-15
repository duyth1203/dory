//! The recorder's rules as a pure function. The actor that owns the device and the take only
//! executes the returned [`Effect`]s and feeds back [`Input`]s.
//!
//! Recorded time is media time: the take writer's running total of frames written, divided by
//! the rate. It is a total and not a sum of deltas, so odd chunk sizes lose nothing and the cap
//! fires at exactly `cap` of audio. Pauses and the undo grace write nothing, so they never count.
//!
//! Pause and the grace window stop the device, so the system's recording indicator is honest.
//! Undo and resume start it again. The take stays open across both, so an undo continues the same
//! file.
//!
//! Every `StartCapture` carries a [`Generation`], one higher than the last. The actor tags the
//! device handle with it, and a failure from that handle comes back carrying it. A failure whose
//! generation is not the current one belongs to a handle that was already replaced, so it is
//! ignored instead of pausing the new capture.
//!
//! ```text
//! Idle --Start--> Recording --Pause--> Paused --Resume--> Recording
//! Recording|Paused --Stop--> Grace --Undo--> Recording
//! Grace --GraceElapsed(matching)--> Idle (finalize)
//! any live state --Wrote reaching the cap--> Idle (finalize, no grace)
//! Recording --StartFailed, nothing recorded--> Idle (discard the take)
//! Recording --StartFailed, audio on disk--> Paused (keep the take)
//! ```

use std::num::NonZeroU32;

use crate::domain::Ms;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// A recording ends by itself at this much recorded time.
    pub cap: Ms,
    /// The warning fires once when this much remains.
    pub warn_before: Ms,
    /// How long a stop can be undone, in monotonic milliseconds.
    pub grace_ms: u64,
    /// The take's sample rate, which turns the frame total in [`Input::Wrote`] into media time.
    pub rate_hz: NonZeroU32,
}

impl Limits {
    pub const DEFAULT_RATE_HZ: NonZeroU32 = NonZeroU32::new(48_000).unwrap();
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cap: Ms(2 * 60 * 60 * 1000),
            warn_before: Ms(5 * 60 * 1000),
            grace_ms: 5_000,
            rate_hz: Self::DEFAULT_RATE_HZ,
        }
    }
}

/// Which start of the capture device a message belongs to. Each `StartCapture` bumps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u32);

impl Generation {
    fn next(self) -> Generation {
        Generation(self.0.wrapping_add(1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// `generation` is the last capture start, kept so a late message from a previous take can
    /// never match the next one.
    Idle { generation: Generation },
    Recording {
        recorded: Ms,
        warned: bool,
        generation: Generation,
    },
    Paused {
        recorded: Ms,
        warned: bool,
        generation: Generation,
    },
    /// Stopped but not finalized. `deadline` is on the monotonic clock.
    Grace {
        recorded: Ms,
        warned: bool,
        deadline: u64,
        generation: Generation,
    },
}

impl State {
    pub fn idle() -> State {
        State::Idle {
            generation: Generation::default(),
        }
    }

    pub fn recorded(&self) -> Option<Ms> {
        self.progress().map(|p| p.recorded)
    }

    /// Whether the capture device should be running.
    pub fn is_capturing(&self) -> bool {
        matches!(self, State::Recording { .. })
    }

    pub fn generation(&self) -> Generation {
        match *self {
            State::Idle { generation }
            | State::Recording { generation, .. }
            | State::Paused { generation, .. }
            | State::Grace { generation, .. } => generation,
        }
    }

    fn progress(&self) -> Option<Progress> {
        match *self {
            State::Idle { .. } => None,
            State::Recording {
                recorded,
                warned,
                generation,
            }
            | State::Paused {
                recorded,
                warned,
                generation,
            }
            | State::Grace {
                recorded,
                warned,
                generation,
                ..
            } => Some(Progress {
                recorded,
                warned,
                generation,
            }),
        }
    }

    fn with_progress(self, recorded: Ms, warned: bool) -> State {
        match self {
            State::Idle { .. } => self,
            State::Recording { generation, .. } => State::Recording {
                recorded,
                warned,
                generation,
            },
            State::Paused { generation, .. } => State::Paused {
                recorded,
                warned,
                generation,
            },
            State::Grace {
                deadline,
                generation,
                ..
            } => State::Grace {
                recorded,
                warned,
                deadline,
                generation,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Progress {
    recorded: Ms,
    warned: bool,
    generation: Generation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Start,
    Pause,
    Resume,
    Stop,
    Undo,
    /// The take writer has written `total_frames` in all, counted from the start of the take and
    /// across resumes. Frames still in flight when the device stops arrive after the stop, so
    /// this is accepted in every live state. Being a running total, a late report from an older
    /// capture is harmless and carries no generation.
    Wrote {
        total_frames: u64,
    },
    /// The grace timer fired. It carries the deadline it was armed for, so a timer that outlived
    /// an undo is recognized as stale.
    GraceElapsed {
        armed_for: u64,
    },
    /// The device died while recording. The take is kept and the user decides what to do. Stale
    /// unless `generation` is the capture that is running now.
    CaptureFailed {
        generation: Generation,
    },
    /// The device would not start. With nothing recorded the take is discarded, otherwise the
    /// recording stays paused so the user can try again. Stale unless `generation` is the start
    /// that was just requested.
    StartFailed {
        generation: Generation,
    },
    /// The app is quitting. Whatever was recorded is saved, never lost.
    Shutdown,
}

impl Input {
    fn is_command(self) -> bool {
        matches!(
            self,
            Input::Start | Input::Pause | Input::Resume | Input::Stop | Input::Undo
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalizeReason {
    Stopped,
    CapReached,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Create the take. Only on a fresh start.
    OpenTake,
    /// Start the device with a fresh sink that feeds the same open take. Tag the handle with
    /// `generation` and send it back with `CaptureFailed` or `StartFailed`.
    StartCapture {
        generation: Generation,
    },
    StopCapture,
    /// Send `GraceElapsed { armed_for: deadline }` when the monotonic clock reaches `deadline`.
    ArmGrace {
        deadline: u64,
    },
    /// Five minutes (by default) remain.
    Warn {
        remaining: Ms,
    },
    /// Close the take, seal it, and hand it to the pipeline.
    Finalize {
        recorded: Ms,
        reason: FinalizeReason,
    },
    /// Delete the open take. Nothing was recorded, so there is nothing to keep or list.
    DiscardTake,
    /// A command that does not apply in this state. The state did not change.
    Rejected,
}

pub fn step(state: State, input: Input, limits: &Limits, now_mono: u64) -> (State, Vec<Effect>) {
    use Effect::{ArmGrace, DiscardTake, Finalize, OpenTake, Rejected, StartCapture, StopCapture};
    use State::{Grace, Idle, Paused, Recording};

    match (state, input) {
        (Idle { generation }, Input::Start) => {
            let generation = generation.next();
            (
                Recording {
                    recorded: Ms(0),
                    warned: false,
                    generation,
                },
                vec![OpenTake, StartCapture { generation }],
            )
        }

        (
            Recording {
                recorded,
                warned,
                generation,
            },
            Input::Pause,
        ) => (
            Paused {
                recorded,
                warned,
                generation,
            },
            vec![StopCapture],
        ),
        (
            Paused {
                recorded,
                warned,
                generation,
            },
            Input::Resume,
        ) => {
            let generation = generation.next();
            (
                Recording {
                    recorded,
                    warned,
                    generation,
                },
                vec![StartCapture { generation }],
            )
        }

        (
            Recording {
                recorded,
                warned,
                generation,
            },
            Input::Stop,
        ) => {
            let deadline = now_mono.saturating_add(limits.grace_ms);
            (
                Grace {
                    recorded,
                    warned,
                    deadline,
                    generation,
                },
                vec![StopCapture, ArmGrace { deadline }],
            )
        }
        (
            Paused {
                recorded,
                warned,
                generation,
            },
            Input::Stop,
        ) => {
            let deadline = now_mono.saturating_add(limits.grace_ms);
            (
                Grace {
                    recorded,
                    warned,
                    deadline,
                    generation,
                },
                vec![ArmGrace { deadline }],
            )
        }

        (
            Grace {
                recorded,
                warned,
                deadline,
                generation,
            },
            Input::Undo,
        ) if now_mono < deadline => {
            let generation = generation.next();
            (
                Recording {
                    recorded,
                    warned,
                    generation,
                },
                vec![StartCapture { generation }],
            )
        }
        (
            Grace {
                recorded,
                deadline,
                generation,
                ..
            },
            Input::GraceElapsed { armed_for },
        ) if armed_for == deadline => (
            Idle { generation },
            vec![Finalize {
                recorded,
                reason: FinalizeReason::Stopped,
            }],
        ),

        (state, Input::Wrote { total_frames }) => wrote(state, total_frames, limits),

        (
            Recording {
                recorded,
                warned,
                generation,
            },
            Input::CaptureFailed { generation: from },
        ) if from == generation => (
            Paused {
                recorded,
                warned,
                generation,
            },
            vec![StopCapture],
        ),

        (
            Recording {
                recorded,
                warned,
                generation,
            },
            Input::StartFailed { generation: from },
        ) if from == generation => {
            if recorded == Ms(0) {
                (Idle { generation }, vec![DiscardTake])
            } else {
                (
                    Paused {
                        recorded,
                        warned,
                        generation,
                    },
                    vec![],
                )
            }
        }

        (
            Recording {
                recorded,
                generation,
                ..
            },
            Input::Shutdown,
        ) => (
            Idle { generation },
            vec![
                StopCapture,
                Finalize {
                    recorded,
                    reason: FinalizeReason::Shutdown,
                },
            ],
        ),
        (
            Paused {
                recorded,
                generation,
                ..
            }
            | Grace {
                recorded,
                generation,
                ..
            },
            Input::Shutdown,
        ) => (
            Idle { generation },
            vec![Finalize {
                recorded,
                reason: FinalizeReason::Shutdown,
            }],
        ),

        (state, input) if input.is_command() => (state, vec![Rejected]),
        (state, _) => (state, vec![]),
    }
}

fn wrote(state: State, total_frames: u64, limits: &Limits) -> (State, Vec<Effect>) {
    let Some(Progress {
        warned, generation, ..
    }) = state.progress()
    else {
        return (state, vec![]);
    };
    let recorded = Ms::from_frames(total_frames, limits.rate_hz);

    if recorded >= limits.cap {
        let mut effects = Vec::new();
        if state.is_capturing() {
            effects.push(Effect::StopCapture);
        }
        effects.push(Effect::Finalize {
            recorded,
            reason: FinalizeReason::CapReached,
        });
        return (State::Idle { generation }, effects);
    }

    let warn_at = limits.cap.0.saturating_sub(limits.warn_before.0);
    if !warned && recorded.0 >= warn_at {
        let remaining = Ms(limits.cap.0 - recorded.0);
        return (
            state.with_progress(recorded, true),
            vec![Effect::Warn { remaining }],
        );
    }
    (state.with_progress(recorded, warned), vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use Effect::*;
    use Input::*;
    use State::{Grace, Idle, Paused, Recording};

    const GRACE: u64 = 5_000;

    /// One frame per millisecond, so a frame total reads as milliseconds.
    fn limits() -> Limits {
        Limits {
            cap: Ms(10_000),
            warn_before: Ms(3_000),
            grace_ms: GRACE,
            rate_hz: NonZeroU32::new(1_000).unwrap(),
        }
    }

    fn g(n: u32) -> Generation {
        Generation(n)
    }

    fn idle() -> State {
        State::idle()
    }

    fn idle_g(n: u32) -> State {
        Idle { generation: g(n) }
    }

    fn rec_g(ms: u64, n: u32) -> State {
        Recording {
            recorded: Ms(ms),
            warned: false,
            generation: g(n),
        }
    }

    fn rec(ms: u64) -> State {
        rec_g(ms, 1)
    }

    fn paused(ms: u64) -> State {
        Paused {
            recorded: Ms(ms),
            warned: false,
            generation: g(1),
        }
    }

    fn grace(ms: u64, deadline: u64) -> State {
        Grace {
            recorded: Ms(ms),
            warned: false,
            deadline,
            generation: g(1),
        }
    }

    fn fin(ms: u64, reason: FinalizeReason) -> Effect {
        Finalize {
            recorded: Ms(ms),
            reason,
        }
    }

    fn start(n: u32) -> Effect {
        StartCapture { generation: g(n) }
    }

    fn wrote(total_frames: u64) -> Input {
        Wrote { total_frames }
    }

    fn failed(n: u32) -> Input {
        CaptureFailed { generation: g(n) }
    }

    fn start_failed(n: u32) -> Input {
        StartFailed { generation: g(n) }
    }

    #[test]
    fn transition_table() {
        type Case = (&'static str, State, Input, u64, State, Vec<Effect>);
        let cases: Vec<Case> = vec![
            (
                "start from idle",
                idle(),
                Start,
                0,
                rec(0),
                vec![OpenTake, start(1)],
            ),
            (
                "pause stops the device",
                rec(700),
                Pause,
                0,
                paused(700),
                vec![StopCapture],
            ),
            (
                "resume restarts it without a new take, under a new generation",
                paused(700),
                Resume,
                0,
                rec_g(700, 2),
                vec![start(2)],
            ),
            (
                "stop while recording stops the device and arms the grace timer",
                rec(900),
                Stop,
                1_000,
                grace(900, 6_000),
                vec![StopCapture, ArmGrace { deadline: 6_000 }],
            ),
            (
                "stop while paused only arms the timer, the device is already off",
                paused(900),
                Stop,
                1_000,
                grace(900, 6_000),
                vec![ArmGrace { deadline: 6_000 }],
            ),
            (
                "undo inside the grace resumes the same take under a new generation",
                grace(900, 6_000),
                Undo,
                5_999,
                rec_g(900, 2),
                vec![start(2)],
            ),
            (
                "undo at the deadline is too late",
                grace(900, 6_000),
                Undo,
                6_000,
                grace(900, 6_000),
                vec![Rejected],
            ),
            (
                "the matching timer finalizes",
                grace(900, 6_000),
                GraceElapsed { armed_for: 6_000 },
                6_000,
                idle_g(1),
                vec![fin(900, FinalizeReason::Stopped)],
            ),
            (
                "a stale timer does nothing",
                grace(900, 9_000),
                GraceElapsed { armed_for: 6_000 },
                9_000,
                grace(900, 9_000),
                vec![],
            ),
            (
                "a timer after undo does nothing",
                rec(900),
                GraceElapsed { armed_for: 6_000 },
                6_000,
                rec(900),
                vec![],
            ),
            (
                "a timer when idle does nothing",
                idle(),
                GraceElapsed { armed_for: 6_000 },
                6_000,
                idle(),
                vec![],
            ),
            (
                "capture failure pauses and keeps the take",
                rec(400),
                failed(1),
                0,
                paused(400),
                vec![StopCapture],
            ),
            (
                "capture failure outside recording is stale",
                paused(400),
                failed(1),
                0,
                paused(400),
                vec![],
            ),
            (
                "capture failure from an older capture is stale",
                rec_g(400, 2),
                failed(1),
                0,
                rec_g(400, 2),
                vec![],
            ),
            (
                "capture failure from a capture that has not started yet is stale",
                rec_g(400, 2),
                failed(3),
                0,
                rec_g(400, 2),
                vec![],
            ),
            (
                "first start failing discards the take",
                rec_g(0, 1),
                start_failed(1),
                0,
                idle_g(1),
                vec![DiscardTake],
            ),
            (
                "a resume failing with audio on disk keeps the take paused",
                rec_g(400, 2),
                start_failed(2),
                0,
                Paused {
                    recorded: Ms(400),
                    warned: false,
                    generation: g(2),
                },
                vec![],
            ),
            (
                "a resume failing with nothing ever recorded discards the take",
                rec_g(0, 2),
                start_failed(2),
                0,
                idle_g(2),
                vec![DiscardTake],
            ),
            (
                "a start failure from an older capture is stale",
                rec_g(0, 2),
                start_failed(1),
                0,
                rec_g(0, 2),
                vec![],
            ),
            (
                "a start failure when not recording is stale",
                paused(0),
                start_failed(1),
                0,
                paused(0),
                vec![],
            ),
            (
                "shutdown while recording saves",
                rec(400),
                Shutdown,
                0,
                idle_g(1),
                vec![StopCapture, fin(400, FinalizeReason::Shutdown)],
            ),
            (
                "shutdown while paused saves",
                paused(400),
                Shutdown,
                0,
                idle_g(1),
                vec![fin(400, FinalizeReason::Shutdown)],
            ),
            (
                "shutdown in grace saves now",
                grace(400, 6_000),
                Shutdown,
                0,
                idle_g(1),
                vec![fin(400, FinalizeReason::Shutdown)],
            ),
            (
                "shutdown when idle does nothing",
                idle(),
                Shutdown,
                0,
                idle(),
                vec![],
            ),
        ];
        for (name, state, input, now, want_state, want_effects) in cases {
            assert_eq!(
                step(state, input, &limits(), now),
                (want_state, want_effects),
                "{name}"
            );
        }
    }

    #[test]
    fn commands_that_do_not_apply_are_rejected_and_change_nothing() {
        let states = [idle(), rec(1), paused(1), grace(1, 100)];
        let commands = [Start, Pause, Resume, Stop, Undo];
        let legal = |s: State, c: Input| {
            matches!(
                (s, c),
                (Idle { .. }, Start)
                    | (Recording { .. }, Pause | Stop)
                    | (Paused { .. }, Resume | Stop)
                    | (Grace { .. }, Undo)
            )
        };
        for state in states {
            for command in commands {
                let (next, effects) = step(state, command, &limits(), 0);
                if legal(state, command) {
                    assert!(!effects.contains(&Rejected), "{state:?} {command:?}");
                } else {
                    assert_eq!(
                        (next, effects),
                        (state, vec![Rejected]),
                        "{state:?} {command:?}"
                    );
                }
            }
        }
    }

    fn run(inputs: &[(Input, u64)]) -> (State, Vec<Effect>) {
        run_with(&limits(), inputs)
    }

    fn run_with(limits: &Limits, inputs: &[(Input, u64)]) -> (State, Vec<Effect>) {
        let mut state = idle();
        let mut all = Vec::new();
        for (input, now) in inputs {
            let (next, effects) = step(state, *input, limits, *now);
            state = next;
            all.extend(effects);
        }
        (state, all)
    }

    #[test]
    fn recorded_time_follows_frames_written_not_the_clock() {
        let (state, _) = run(&[
            (Start, 0),
            (wrote(2_000), 0),
            (Pause, 2_000),
            (wrote(2_010), 2_001),
            (Resume, 60_000),
            (wrote(3_010), 61_000),
            (Stop, 61_000),
            (wrote(3_030), 61_020),
            (Undo, 62_000),
            (wrote(3_530), 62_500),
        ]);
        assert_eq!(
            state,
            rec_g(3_530, 3),
            "the minute spent paused adds nothing, frames still in flight after a stop do"
        );
    }

    #[test]
    fn stop_undo_stop_ignores_the_first_timer_and_honors_the_second() {
        let (state, effects) = run(&[
            (Start, 0),
            (wrote(1_000), 1_000),
            (Stop, 1_000),
            (Undo, 2_000),
            (Stop, 3_000),
            (
                GraceElapsed {
                    armed_for: 1_000 + GRACE,
                },
                6_000,
            ),
        ]);
        assert_eq!(
            state,
            Grace {
                recorded: Ms(1_000),
                warned: false,
                deadline: 3_000 + GRACE,
                generation: g(2),
            },
            "the stale timer must not finalize"
        );
        assert!(!effects.iter().any(|e| matches!(e, Finalize { .. })));

        let (state, effects) = step(
            state,
            GraceElapsed {
                armed_for: 3_000 + GRACE,
            },
            &limits(),
            8_000,
        );
        assert_eq!(
            (state, effects),
            (idle_g(2), vec![fin(1_000, FinalizeReason::Stopped)])
        );
    }

    #[test]
    fn the_device_runs_only_while_recording() {
        let mut state = idle();
        let script = [Start, Pause, Resume, Stop, Undo, Stop];
        for (i, input) in script.into_iter().enumerate() {
            let (next, effects) = step(state, input, &limits(), i as u64);
            let started = effects
                .iter()
                .filter(|e| matches!(e, StartCapture { .. }))
                .count() as i32;
            let stopped = effects.iter().filter(|e| **e == StopCapture).count() as i32;
            assert_eq!(
                i32::from(next.is_capturing()) - i32::from(state.is_capturing()),
                started - stopped,
                "{input:?}"
            );
            state = next;
        }
        assert!(!state.is_capturing());
    }

    #[test]
    fn every_capture_start_gets_a_fresh_generation_even_across_takes() {
        let (_, effects) = run(&[
            (Start, 0),
            (Pause, 1),
            (Resume, 2),
            (Stop, 3),
            (Undo, 4),
            (Stop, 5),
            (
                GraceElapsed {
                    armed_for: 5 + GRACE,
                },
                5 + GRACE,
            ),
            (Start, 6_000),
        ]);
        let generations: Vec<_> = effects
            .iter()
            .filter_map(|e| match e {
                StartCapture { generation } => Some(generation.0),
                _ => None,
            })
            .collect();
        assert_eq!(generations, vec![1, 2, 3, 4]);
    }

    #[test]
    fn reaching_the_cap_finalizes_with_no_grace() {
        let cases = [
            (
                "while recording",
                rec(9_000),
                wrote(10_000),
                vec![StopCapture, fin(10_000, FinalizeReason::CapReached)],
            ),
            (
                "overshoot is reported as written",
                rec(9_990),
                wrote(10_010),
                vec![StopCapture, fin(10_010, FinalizeReason::CapReached)],
            ),
            (
                "trailing frames after a pause",
                paused(9_990),
                wrote(10_000),
                vec![fin(10_000, FinalizeReason::CapReached)],
            ),
            (
                "trailing frames in the grace",
                grace(9_990, 6_000),
                wrote(10_000),
                vec![fin(10_000, FinalizeReason::CapReached)],
            ),
        ];
        for (name, state, input, want) in cases {
            assert_eq!(
                step(state, input, &limits(), 0),
                (idle_g(1), want),
                "{name}"
            );
        }
    }

    #[test]
    fn odd_sized_drains_reach_the_two_hour_cap_at_exactly_two_hours_of_frames() {
        let limits = Limits::default();
        let cap_frames = 2 * 60 * 60 * 48_000_u64;
        let drain = 959;

        let mut state = idle();
        let mut finalizes = Vec::new();
        let mut total = 0;
        let (next, _) = step(state, Start, &limits, 0);
        state = next;
        let mut drains = 0_u64;
        while state != idle_g(1) {
            total += drain;
            drains += 1;
            let (next, effects) = step(state, wrote(total), &limits, 0);
            state = next;
            finalizes.extend(effects.into_iter().filter(|e| matches!(e, Finalize { .. })));
            assert!(drains < 1_000_000, "the cap never fired");
        }

        assert_eq!(
            total,
            cap_frames.div_ceil(drain) * drain,
            "fires on the first drain that reaches the cap, not a drain later"
        );
        assert!(
            total - drain < cap_frames,
            "the drain before was under the cap"
        );
        assert_eq!(
            finalizes,
            vec![fin(
                Ms::from_frames(total, limits.rate_hz).0,
                FinalizeReason::CapReached
            )]
        );

        let (_, effects) = step(rec(0), wrote(cap_frames - 1), &limits, 0);
        assert!(
            !effects.iter().any(|e| matches!(e, Finalize { .. })),
            "one frame short is not the cap"
        );
        let (state, effects) = step(rec(0), wrote(cap_frames), &limits, 0);
        assert_eq!(
            (state, effects),
            (
                idle_g(1),
                vec![StopCapture, fin(7_200_000, FinalizeReason::CapReached)]
            ),
            "exactly two hours of frames is exactly the cap"
        );
    }

    #[test]
    fn a_timer_armed_before_the_cap_is_stale_afterwards() {
        let (state, effects) = run(&[
            (Start, 0),
            (wrote(9_000), 0),
            (Stop, 0),
            (wrote(10_000), 10),
            (GraceElapsed { armed_for: GRACE }, GRACE),
        ]);
        assert_eq!(state, idle_g(1));
        assert_eq!(
            effects
                .iter()
                .filter(|e| matches!(e, Finalize { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn the_warning_fires_once_when_the_threshold_is_crossed() {
        let (state, effects) = step(rec(6_900), wrote(7_000), &limits(), 0);
        assert_eq!(
            (state, effects),
            (
                Recording {
                    recorded: Ms(7_000),
                    warned: true,
                    generation: g(1),
                },
                vec![Warn {
                    remaining: Ms(3_000)
                }]
            )
        );

        let (state, effects) = step(state, wrote(7_500), &limits(), 0);
        assert_eq!(
            (state, effects),
            (
                Recording {
                    recorded: Ms(7_500),
                    warned: true,
                    generation: g(1),
                },
                vec![]
            )
        );
    }

    #[test]
    fn the_warning_survives_pause_and_undo() {
        let (state, effects) = run(&[
            (Start, 0),
            (wrote(7_000), 0),
            (Pause, 0),
            (Resume, 1),
            (Stop, 2),
            (Undo, 3),
            (wrote(8_000), 4),
        ]);
        assert_eq!(
            state,
            Recording {
                recorded: Ms(8_000),
                warned: true,
                generation: g(3),
            }
        );
        assert_eq!(
            effects.iter().filter(|e| matches!(e, Warn { .. })).count(),
            1
        );
    }

    #[test]
    fn a_jump_over_the_warning_and_the_cap_only_finalizes() {
        let (state, effects) = step(rec(0), wrote(10_000), &limits(), 0);
        assert_eq!(state, idle_g(1));
        assert_eq!(
            effects,
            vec![StopCapture, fin(10_000, FinalizeReason::CapReached)]
        );
    }

    #[test]
    fn limits_are_configurable_and_default_to_two_hours_at_48_khz() {
        let d = Limits::default();
        assert_eq!(
            (d.cap, d.warn_before, d.grace_ms, d.rate_hz.get()),
            (Ms(7_200_000), Ms(300_000), 5_000, 48_000)
        );

        let tight = Limits {
            cap: Ms(100),
            warn_before: Ms(10),
            grace_ms: 1,
            rate_hz: NonZeroU32::new(1_000).unwrap(),
        };
        let (state, effects) = step(rec_g(89, 1), wrote(90), &tight, 0);
        assert_eq!(
            (state.recorded(), effects),
            (Some(Ms(90)), vec![Warn { remaining: Ms(10) }])
        );
        let (state, effects) = step(state, Stop, &tight, 50);
        assert_eq!(
            (state, effects),
            (
                Grace {
                    recorded: Ms(90),
                    warned: true,
                    deadline: 51,
                    generation: g(1),
                },
                vec![StopCapture, ArmGrace { deadline: 51 }]
            )
        );
        let (_, effects) = step(rec(0), wrote(100), &tight, 0);
        assert_eq!(
            effects,
            vec![StopCapture, fin(100, FinalizeReason::CapReached)]
        );
    }

    #[test]
    fn wrote_while_idle_is_ignored() {
        assert_eq!(step(idle(), wrote(5), &limits(), 0), (idle(), vec![]));
    }

    #[test]
    fn a_start_that_fails_leaves_nothing_behind_and_the_next_start_works() {
        let (state, effects) = run(&[(Start, 0), (start_failed(1), 1)]);
        assert_eq!(state, idle_g(1));
        assert_eq!(effects, vec![OpenTake, start(1), DiscardTake]);

        let (state, effects) = run(&[(Start, 0), (start_failed(1), 1), (Stop, 2)]);
        assert_eq!(
            (state, effects.last().copied()),
            (idle_g(1), Some(Rejected)),
            "there is no take left to stop, so a Stop cannot finalize an empty one"
        );
        assert!(!effects.iter().any(|e| matches!(e, Finalize { .. })));

        let (state, effects) = step(state, Start, &limits(), 3);
        assert_eq!((state, effects), (rec_g(0, 2), vec![OpenTake, start(2)]));
    }

    #[test]
    fn a_late_failure_from_the_replaced_capture_does_not_pause_the_new_one() {
        let (state, _) = run(&[
            (Start, 0),
            (wrote(500), 1),
            (Pause, 2),
            (Resume, 3),
            (failed(1), 4),
        ]);
        assert_eq!(state, rec_g(500, 2), "still recording");

        let (state, effects) = step(state, failed(2), &limits(), 5);
        assert_eq!((state, effects), (paused_g(500, 2), vec![StopCapture]));
    }

    #[test]
    fn a_failure_from_a_previous_take_cannot_touch_the_next_take() {
        let (state, _) = run(&[
            (Start, 0),
            (Stop, 1),
            (
                GraceElapsed {
                    armed_for: GRACE + 1,
                },
                GRACE + 1,
            ),
            (Start, 10_000),
        ]);
        assert_eq!(state, rec_g(0, 2));
        let (state, effects) = step(state, failed(1), &limits(), 10_001);
        assert_eq!((state, effects), (rec_g(0, 2), vec![]));
    }

    fn paused_g(ms: u64, n: u32) -> State {
        Paused {
            recorded: Ms(ms),
            warned: false,
            generation: g(n),
        }
    }
}
