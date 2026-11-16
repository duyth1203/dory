//! Settings and the model lists on `AppState` (P6): the reads and writes the Settings window,
//! the onboarding speech step, and the Set up summaries sheet need. A child of `app`, so it
//! reaches the private core boundary like the rest of `AppState`.

use dory_core::app::{AppKind, ModelRow};
use dory_core::domain::Settings;
use dory_core::models::{Choice, ModelId, ModelRef};
use dory_core::ports::ProvisionError;
use dory_core::ports::capture::InputDevice;
use gpui_kit::{App, Context, Task};

use super::{AppState, Download};

impl AppState {
    pub fn read_settings(&self, cx: &App) -> Task<Result<Settings, String>> {
        self.core
            .run(cx, |dory| dory.settings().map_err(|e| e.to_string()))
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
