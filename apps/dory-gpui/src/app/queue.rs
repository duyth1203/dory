//! The processing queue's reads and writes for the popover (UX-15). A child of `app`, so it
//! reaches the private core boundary like the rest of `AppState`.

use dory_core::domain::{Job, JobId, RecordingId};
use gpui_kit::{Context, Task};

use super::AppState;
use crate::library::Progress;

impl AppState {
    /// Running, queued, and blocked jobs, in the order the queue shows them.
    pub fn queue(&self, cx: &mut Context<Self>) -> Task<Result<Vec<Job>, String>> {
        self.core.run(cx, |dory| {
            dory.pipeline().queue().map_err(|e| e.to_string())
        })
    }

    pub fn progress_of(&self, id: &RecordingId) -> Option<Progress> {
        self.progress.get(id).copied()
    }

    /// Move to top. The core announces the change, so the popover refreshes by itself.
    pub fn promote_job(&mut self, job: JobId, cx: &mut Context<Self>) {
        let done = self.core.run(cx, move |dory| dory.pipeline().promote(job));
        self.report(done, cx);
    }

    pub fn cancel_job(&mut self, job: JobId, cx: &mut Context<Self>) {
        let done = self.core.run(cx, move |dory| dory.pipeline().cancel(job));
        self.report(done, cx);
    }
}
