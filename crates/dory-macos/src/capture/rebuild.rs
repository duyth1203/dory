//! When to rebuild the device graph. Pure: time is a `Duration` since the supervisor started, so
//! every decision is unit-tested without hardware or sleeps.

use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Quiet time after the last trigger before a rebuild runs, so a burst of HAL notifications
    /// (output change, then its rate, then aliveness) costs one rebuild.
    pub debounce: Duration,
    /// How long failed rebuilds are retried, counted from the first failure.
    pub retry_for: Duration,
    pub first_backoff: Duration,
    pub max_backoff: Duration,
}

pub const POLICY: Policy = Policy {
    debounce: Duration::from_millis(250),
    retry_for: Duration::from_secs(5),
    first_backoff: Duration::from_millis(100),
    max_backoff: Duration::from_secs(1),
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The graph is fine and nothing asked for a rebuild.
    Steady,
    /// A trigger arrived. The rebuild runs at `due` unless another trigger pushes it out.
    Settling { due: Duration },
    /// The last attempt failed. The next runs at `due`.
    Retrying {
        since: Duration,
        failures: u32,
        due: Duration,
    },
    /// Retries ran out. Terminal: the capture is reported fatal.
    GaveUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Rebuilt,
    RetryAt(Duration),
    GiveUp,
}

#[derive(Clone, Copy, Debug)]
pub struct Rebuilder {
    policy: Policy,
    phase: Phase,
}

impl Rebuilder {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            phase: Phase::Steady,
        }
    }

    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// A device change, a stall, or an explicit request. A pending retry already rebuilds against
    /// the devices current at that time, so it is not postponed.
    pub fn trigger(&mut self, now: Duration) {
        if let Phase::Steady | Phase::Settling { .. } = self.phase {
            self.phase = Phase::Settling {
                due: now + self.policy.debounce,
            };
        }
    }

    /// When the next attempt is due, for the supervisor's wait.
    pub fn deadline(&self) -> Option<Duration> {
        match self.phase {
            Phase::Settling { due } | Phase::Retrying { due, .. } => Some(due),
            Phase::Steady | Phase::GaveUp => None,
        }
    }

    pub fn due(&self, now: Duration) -> bool {
        self.deadline().is_some_and(|due| now >= due)
    }

    /// Records the outcome of the attempt that just finished at `now`.
    pub fn record(&mut self, now: Duration, ok: bool) -> Verdict {
        if ok {
            self.phase = Phase::Steady;
            return Verdict::Rebuilt;
        }
        let (since, failures) = match self.phase {
            Phase::Retrying {
                since, failures, ..
            } => (since, failures + 1),
            _ => (now, 1),
        };
        let due = now + self.backoff(failures);
        if due > since + self.policy.retry_for {
            self.phase = Phase::GaveUp;
            return Verdict::GiveUp;
        }
        self.phase = Phase::Retrying {
            since,
            failures,
            due,
        };
        Verdict::RetryAt(due)
    }

    fn backoff(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        (self.policy.first_backoff * 2u32.pow(doublings)).min(self.policy.max_backoff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn a_burst_of_triggers_rebuilds_once_after_the_last_one_settles() {
        let mut r = Rebuilder::new(POLICY);
        assert_eq!(r.deadline(), None);
        for t in [0, 40, 90, 150] {
            r.trigger(ms(t));
            assert!(!r.due(ms(t)), "not due inside the burst");
        }
        assert!(!r.due(ms(399)), "250 ms after the last trigger is 400 ms");
        assert!(r.due(ms(400)));
        assert_eq!(r.record(ms(420), true), Verdict::Rebuilt);
        assert_eq!(r.phase(), Phase::Steady);
        assert!(!r.due(ms(10_000)), "one burst, one rebuild");
    }

    #[test]
    fn failures_back_off_by_doubling_up_to_one_second() {
        let mut r = Rebuilder::new(POLICY);
        r.trigger(ms(0));
        let mut now = ms(250);
        let mut waits = Vec::new();
        loop {
            match r.record(now, false) {
                Verdict::RetryAt(due) => {
                    waits.push((due - now).as_millis());
                    assert!(!r.due(due - ms(1)));
                    assert!(r.due(due));
                    now = due;
                }
                Verdict::GiveUp => break,
                Verdict::Rebuilt => unreachable!(),
            }
        }
        assert_eq!(waits, [100, 200, 400, 800, 1000, 1000, 1000]);
        assert_eq!(r.phase(), Phase::GaveUp);
        assert!(
            now <= ms(250) + POLICY.retry_for,
            "no attempt starts after the 5 s window, last at {now:?}"
        );
    }

    #[test]
    fn retries_stop_at_five_seconds_even_when_attempts_are_slow() {
        let mut r = Rebuilder::new(POLICY);
        r.trigger(ms(0));
        assert_eq!(r.record(ms(250), false), Verdict::RetryAt(ms(350)));
        assert_eq!(r.record(ms(4_000), false), Verdict::RetryAt(ms(4_200)));
        assert_eq!(
            r.record(ms(5_000), false),
            Verdict::GiveUp,
            "a 400 ms wait would start the next attempt past 5.25 s"
        );
    }

    #[test]
    fn a_retry_that_succeeds_returns_to_steady_and_a_later_failure_starts_a_fresh_window() {
        let mut r = Rebuilder::new(POLICY);
        r.trigger(ms(0));
        r.record(ms(250), false);
        assert_eq!(r.record(ms(350), true), Verdict::Rebuilt);
        r.trigger(ms(60_000));
        assert_eq!(
            r.record(ms(60_250), false),
            Verdict::RetryAt(ms(60_350)),
            "first failure of a new window waits the first backoff"
        );
    }

    #[test]
    fn triggers_during_a_retry_do_not_postpone_it() {
        let mut r = Rebuilder::new(POLICY);
        r.trigger(ms(0));
        r.record(ms(250), false);
        r.trigger(ms(300));
        assert!(r.due(ms(350)));
    }

    #[test]
    fn after_giving_up_nothing_is_ever_due_again() {
        let mut r = Rebuilder::new(Policy {
            retry_for: ms(0),
            ..POLICY
        });
        r.trigger(ms(0));
        assert_eq!(r.record(ms(250), false), Verdict::GiveUp);
        r.trigger(ms(300));
        assert_eq!(r.deadline(), None);
        assert!(!r.due(ms(100_000)));
    }
}
