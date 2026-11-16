use std::sync::Arc;

use dory_core::ports::{Conditions, Machine};

use crate::{Config, platform};

/// The platform's machine, or one that pretends `DORY_MACHINE`'s conditions.
pub fn for_config(config: &Config) -> Arc<dyn Machine> {
    match config.machine {
        Some(conditions) => Arc::new(Pretend { conditions }),
        None => platform::machine(),
    }
}

/// Reports fixed conditions.
struct Pretend {
    conditions: Conditions,
}

impl Machine for Pretend {
    fn conditions(&self) -> Conditions {
        self.conditions
    }
}
