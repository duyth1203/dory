//! How hard the pipeline works (UX-30). The setting, the Mac's conditions, and the recorder decide
//! the budget before each speech window and each summary pass.

use crate::domain::Processing;
use crate::ports::{Conditions, Pace, Power, Thermal};

/// Automatic returns to fast only once no light condition has held for this long, so plugging
/// in and out does not make the work stutter, and an engine rebuilds its threads at most once
/// per hold.
pub const HOLD_MS: u64 = 2 * 60 * 1000;

/// Light's thread count: two efficiency cores.
pub const LIGHT_THREADS: u8 = 2;

/// Why a job runs light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LightReason {
    /// The user chose Light.
    Chosen,
    Battery,
    LowPower,
    /// Thermal state serious or critical.
    Hot,
    /// The recorder is not idle.
    Recording,
}

/// What one speech window or summary pass may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Budget {
    Fast { threads: u8 },
    Light { reason: LightReason },
}

impl Budget {
    pub fn pace(self) -> Pace {
        match self {
            Budget::Fast { .. } => Pace::Fast,
            Budget::Light { .. } => Pace::Light,
        }
    }

    pub fn threads(self) -> u8 {
        match self {
            Budget::Fast { threads } => threads,
            Budget::Light { .. } => LIGHT_THREADS,
        }
    }

    pub fn reason(self) -> Option<LightReason> {
        match self {
            Budget::Fast { .. } => None,
            Budget::Light { reason } => Some(reason),
        }
    }
}

/// The light condition that holds now, the first in UX-30's order.
pub fn light_condition(now: Conditions, recording: bool) -> Option<LightReason> {
    if now.power == Power::Battery {
        Some(LightReason::Battery)
    } else if now.low_power {
        Some(LightReason::LowPower)
    } else if now.thermal >= Thermal::Serious {
        Some(LightReason::Hot)
    } else if recording {
        Some(LightReason::Recording)
    } else {
        None
    }
}

/// The pace policy. It remembers only the last light condition Automatic saw and when, for the
/// hold, so each call is a pure step from that memory and its inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pacer {
    fast_threads: u8,
    /// The last light condition seen under Automatic, at that monotonic millisecond.
    held: Option<(LightReason, u64)>,
}

impl Pacer {
    /// `fast_threads` is Fast's count, `min(4, available cores)` on a real machine.
    pub fn new(fast_threads: u8) -> Self {
        Self {
            fast_threads: fast_threads.max(1),
            held: None,
        }
    }

    /// The budget at monotonic millisecond `at`. During the hold, the reason stays the condition
    /// that last held.
    pub fn next(
        &mut self,
        setting: Processing,
        now: Conditions,
        recording: bool,
        at: u64,
    ) -> Budget {
        let fast = Budget::Fast {
            threads: self.fast_threads,
        };
        match setting {
            Processing::Fast => fast,
            Processing::Light => Budget::Light {
                reason: LightReason::Chosen,
            },
            Processing::Auto => {
                if let Some(reason) = light_condition(now, recording) {
                    self.held = Some((reason, at));
                    return Budget::Light { reason };
                }
                match self.held {
                    Some((reason, since)) if at.saturating_sub(since) < HOLD_MS => {
                        Budget::Light { reason }
                    }
                    _ => {
                        self.held = None;
                        fast
                    }
                }
            }
        }
    }
}

/// Fast's thread count on this machine: today's count before P6b, so nothing gets slower.
pub(crate) fn fast_threads() -> u8 {
    std::thread::available_parallelism().map_or(2, |n| n.get().min(4) as u8)
}
