use std::sync::Arc;

use dory_core::ports::{Conditions, Machine, Pace};

use crate::{Config, platform};

/// The platform's machine, or one that pretends `DORY_MACHINE`'s conditions.
pub fn for_config(config: &Config) -> Arc<dyn Machine> {
    let real = platform::machine();
    match config.machine {
        Some(conditions) => Arc::new(Pretend { conditions, real }),
        None => real,
    }
}

/// Reports fixed conditions. The pace still reaches the real thread, so a lane measures it.
struct Pretend {
    conditions: Conditions,
    real: Arc<dyn Machine>,
}

impl Machine for Pretend {
    fn conditions(&self) -> Conditions {
        self.conditions
    }

    fn set_pace(&self, pace: Pace) {
        self.real.set_pace(pace);
    }
}
