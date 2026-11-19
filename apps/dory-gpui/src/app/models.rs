//! Settings and the model lists on `AppState` (P6): the reads and writes the Settings window,
//! the onboarding speech step, and the Set up summaries sheet need. Also Settings > Storage's
//! **Move original WebM files to Trash** (P7). A child of `app`, so it reaches the private core
//! boundary like the rest of `AppState`.

use std::sync::Arc;

use dory_core::app::{AppKind, ModelRow, TrashedOriginals};
use dory_core::domain::{Settings, VocabularyTerm};
use dory_core::models::{Choice, ModelId, ModelRef};
use dory_core::ports::ProvisionError;
use dory_core::ports::capture::InputDevice;
use gpui_kit::{App, Context, SharedString, Task};

use super::{AppState, Download};

impl AppState {
    pub fn read_settings(&self, cx: &App) -> Task<Result<Settings, String>> {
        self.core
            .run(cx, |dory| dory.settings().map_err(|e| e.to_string()))
    }

    /// What the speech engine is asked to favor: the user's terms, then the speaker names from the
    /// library that are not hidden.
    pub fn read_speech_vocabulary(&self, cx: &App) -> Task<Result<Vec<VocabularyTerm>, String>> {
        self.core.run(cx, |dory| {
            dory.speech_vocabulary().map_err(|e| e.to_string())
        })
    }

    /// Validates and saves in one step. Blocked jobs are tried again, since the edit may fix them.
    pub fn edit_settings(
        &mut self,
        edit: impl FnOnce(&mut Settings) + Send + 'static,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.core.run(cx, move |dory| {
            dory.update_settings(edit).map_err(|e| e.to_string())
        })
    }

    /// Every model of kind `K`, probed now, with its fit against the settings. Probing can reach
    /// a server, so it runs off the main thread.
    pub fn model_rows<K: AppKind>(&self, cx: &App) -> Task<Result<Vec<ModelRow<K>>, String>> {
        let list = self.core.dory().models().list::<K>();
        cx.background_executor()
            .spawn(async move { list.await.map_err(|e| e.to_string()) })
    }

    pub fn choose_model<K: AppKind>(
        &mut self,
        choice: Choice<K>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.core.run(cx, move |dory| {
            dory.models().choose(choice).map_err(|e| e.to_string())
        })
    }

    /// Downloads a model of any kind. Progress and the result arrive as core events, which
    /// `download_of` and the error notice already follow.
    pub fn provision_model<K: AppKind>(&mut self, id: ModelId<K>, cx: &mut Context<Self>) {
        let model = id.erase();
        if self.downloads.contains_key(&model) {
            return;
        }
        let done = self.core.dory().models().provision(id);
        self.downloads.insert(
            model.clone(),
            Download {
                done: 0,
                total: None,
            },
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Only a model the catalog lacks fails before the provider runs, so only it has no
            // `ProvisionFinished` to clear the row.
            if let Err(error @ ProvisionError::NotProvisionable(_)) = done.await {
                let _ = this.update(cx, |state, cx| {
                    state.downloads.remove(&model);
                    state.say(format!("Could not download {model}: {error}"), cx);
                });
            }
        })
        .detach();
    }

    /// Stops a download. The provider answers with a cancelled `ProvisionFinished`.
    pub fn cancel_provision(&mut self, model: &ModelRef) {
        self.core.dory().models().cancel_provision(model);
    }

    /// The Mac's audio inputs, for Settings > Recording.
    pub fn input_devices(&self, cx: &App) -> Task<Result<Vec<InputDevice>, String>> {
        let devices = self.core.dory().recorder().input_devices();
        cx.background_executor()
            .spawn(async move { devices.await.map_err(|e| e.to_string()) })
    }
}

/// The old app's WebM files that recordings still read in place, and what Settings > Storage
/// offers for them. Nothing moves them unless the user asks, and Undo puts them back while the
/// app runs.
#[derive(Clone, Debug)]
pub enum Originals {
    Unread,
    Reading,
    /// `count` files, `bytes` in all. Zero hides the offer.
    Listed {
        count: usize,
        bytes: u64,
    },
    Moving,
    /// In the Trash, with what Undo needs.
    Moved(Arc<TrashedOriginals>),
    Restoring,
    Failed(SharedString),
}

impl AppState {
    pub fn originals(&self) -> &Originals {
        &self.originals
    }

    /// Lists the originals again, off the main thread. Settings > Storage calls it when it opens.
    pub fn read_originals(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.originals,
            Originals::Reading | Originals::Moving | Originals::Restoring | Originals::Moved(_)
        ) {
            return;
        }
        self.originals = Originals::Reading;
        cx.notify();
        let list = self.core.run(cx, |dory| dory.library().originals());
        cx.spawn(async move |this, cx| {
            let listed = list.await;
            let _ = this.update(cx, |state, cx| {
                state.originals = match listed {
                    Ok(found) => Originals::Listed {
                        count: found.len(),
                        bytes: found.iter().map(|o| o.bytes).sum(),
                    },
                    Err(error) => Originals::Failed(error.to_string().into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies each original in as Dory's own audio, then moves the originals to the Trash.
    pub fn trash_originals(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.originals, Originals::Listed { count, .. } if count > 0) {
            return;
        }
        self.originals = Originals::Moving;
        cx.notify();
        let work = self.core.run(cx, |dory| dory.library().trash_originals());
        cx.spawn(async move |this, cx| {
            let moved = work.await;
            let _ = this.update(cx, |state, cx| {
                match moved {
                    Ok(trashed) => {
                        let mut text = format!(
                            "Moved {} to the Trash.",
                            count(trashed.count(), "original WebM file")
                        );
                        if trashed.skipped() > 0 {
                            text.push_str(&format!(
                                " Left {} that are still processing.",
                                count(trashed.skipped(), "file")
                            ));
                        }
                        crate::a11y::announce(&text);
                        state.originals = Originals::Moved(Arc::new(trashed));
                    }
                    Err(error) => {
                        let text = format!("Could not move the WebM files: {error}");
                        crate::a11y::announce(&text);
                        state.originals = Originals::Failed(text.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts the originals back where they were. The recordings keep Dory's copies.
    pub fn undo_trash_originals(&mut self, cx: &mut Context<Self>) {
        let Originals::Moved(trashed) =
            std::mem::replace(&mut self.originals, Originals::Restoring)
        else {
            return;
        };
        let Some(trashed) = Arc::into_inner(trashed) else {
            return;
        };
        cx.notify();
        let work = self
            .core
            .run(cx, move |dory| dory.library().restore_originals(trashed));
        cx.spawn(async move |this, cx| {
            let restored = work.await;
            let _ = this.update(cx, |state, cx| {
                match restored {
                    Ok(()) => {
                        crate::a11y::announce("Put the WebM files back.");
                        state.originals = Originals::Unread;
                        state.read_originals(cx);
                    }
                    Err(error) => {
                        let text = format!("Could not put the WebM files back: {error}");
                        crate::a11y::announce(&text);
                        state.originals = Originals::Failed(text.into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// "1 file", "3 files".
pub fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}
