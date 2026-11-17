//! What quitting would interrupt, and the core's shutdown. A child of `app`, so it reaches the
//! private core boundary like the rest of `AppState`.

use dory_core::domain::RecordingId;
use gpui_kit::{App, Context, Task};

use super::AppState;
use crate::in_progress::{Facts, InProgress};

impl AppState {
    pub fn in_progress(&self, cx: &App) -> InProgress {
        let mut downloads: Vec<_> = self
            .downloads
            .iter()
            .map(|(model, download)| (self.model_name(model), *download))
            .collect();
        downloads.sort_by(|a, b| a.0.cmp(&b.0));
        InProgress::of(Facts {
            recorder: &self.recorder,
            elapsed: self.live.read(cx).recorded,
            queue: &self.queue,
            title_of: &|id| self.title_or_new(id),
            progress_of: &|id| self.progress_of(id),
            downloads,
        })
    }

    /// The title the sidebar shows, or "New recording" for a take not listed yet.
    pub fn title_or_new(&self, id: &RecordingId) -> String {
        match self.row(id) {
            Some(row) => crate::library::title(row, &chrono::Local),
            None => "New recording".to_owned(),
        }
    }

    /// Saves the take, stops the pipeline after putting its job back in the queue, and stops
    /// downloads, blocking until they have. Safe to call more than once.
    pub fn shut_down(&mut self) {
        self.shut_down = true;
        self.core.dory().shutdown();
    }

    /// [`AppState::shut_down`] off the main thread. No recording can start meanwhile.
    pub fn shut_down_in_background(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.shut_down = true;
        self.core.run(cx, |dory| dory.shutdown())
    }

    pub fn is_shut_down(&self) -> bool {
        self.shut_down
    }
}
