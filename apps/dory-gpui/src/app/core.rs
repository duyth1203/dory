use std::sync::Arc;

use dory_core::app::Dory;
use gpui_kit::{App, Task};

/// The one holder of the `Dory` facade. It is private to `app`, so a view cannot reach the store:
/// a `use crate::app::core` from any other module fails to build.
#[derive(Clone)]
pub(super) struct Core(Arc<Dory>);

impl Core {
    pub(super) fn new(dory: Dory) -> Self {
        Self(Arc::new(dory))
    }

    pub(super) fn dory(&self) -> &Dory {
        &self.0
    }

    /// Runs `work` against the facade on GPUI's background executor, so a store read or write
    /// never blocks a frame.
    pub(super) fn run<T: Send + 'static>(
        &self,
        cx: &App,
        work: impl FnOnce(&Dory) -> T + Send + 'static,
    ) -> Task<T> {
        let dory = Arc::clone(&self.0);
        cx.background_executor().spawn(async move { work(&dory) })
    }
}
