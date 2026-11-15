use std::time::{Instant, SystemTime, UNIX_EPOCH};

use dory_core::domain::Timestamp;
use dory_core::ports::Clock;

/// Wall time from the system clock and monotonic time counted from when this clock was made.
#[derive(Debug)]
pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let ms = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
        };
        Timestamp(ms)
    }

    fn monotonic_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_time_is_after_2024_and_monotonic_time_never_goes_back() {
        let clock = SystemClock::new();
        assert!(clock.now().0 > 1_704_067_200_000);
        let a = clock.monotonic_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(clock.monotonic_ms() >= a + 5);
    }
}
