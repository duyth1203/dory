use crate::domain::Timestamp;

pub trait Clock: Send + Sync {
    /// Wall clock, for records and display.
    fn now(&self) -> Timestamp;
    /// Milliseconds on a clock that never goes backwards, for deadlines and timers.
    fn monotonic_ms(&self) -> u64;
}
