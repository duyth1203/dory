//! First run, permissions, settings, and per-recording language on `AppState` (P6). Views call
//! these methods; only this module tree reaches the facade.

use dory_core::app::DoryError;
use dory_core::domain::{Goal, LanguagePref, RecordingId, Settings};
use dory_core::ports::capture::{Access, CaptureCaps, Source};
use gpui_kit::Context;

use super::{AppState, Sources};
use crate::detail::language_name;
use crate::onboarding::{self, OnboardingStep, Permission};

impl AppState {
    /// `Done` once first run is over or skipped.
    pub fn onboarding(&self) -> OnboardingStep {
        self.prefs.onboarding.unwrap_or(OnboardingStep::Done)
    }

    pub fn continue_onboarding(&mut self, cx: &mut Context<Self>) {
        let next = self.onboarding().next();
        if next == OnboardingStep::Models
            && let Some(language) = onboarding::system_language()
        {
            self.update_settings(
                move |settings| {
                    if settings.meeting_languages.is_empty() {
                        settings.meeting_languages = vec![language];
                    }
                },
                cx,
            );
        }
        self.set_onboarding(next, cx);
    }

    /// Recording works without models (UX-17), so the Models step can be left at any time.
    pub fn skip_onboarding(&mut self, cx: &mut Context<Self>) {
        self.set_onboarding(OnboardingStep::Done, cx);
    }

    fn set_onboarding(&mut self, step: OnboardingStep, cx: &mut Context<Self>) {
        if self.prefs.onboarding == Some(step) {
            return;
        }
        eprintln!("onboarding: {}", step.as_str());
        self.prefs.onboarding = Some(step);
        cx.notify();
    }

    pub fn permission(&self, source: Source) -> Permission {
        if self.asking.contains(&source) {
            return Permission::Asking;
        }
        match (&self.sources, source) {
            (None, _) => Permission::Checking,
            (Some(s), Source::Microphone) => Permission::Known(s.microphone),
            (Some(s), Source::SystemAudio) => Permission::Known(s.system_audio),
        }
    }

    /// Reads both permissions and the default microphone again, with no prompt: at launch, and
    /// when the window comes back from System Settings.
    pub fn reprobe(&mut self, cx: &mut Context<Self>) {
        let probe = self.core.dory().recorder().probe();
        let devices = self.core.dory().recorder().input_devices();
        cx.spawn(async move |this, cx| {
            let caps = probe.await.unwrap_or(CaptureCaps {
                microphone: Access::Unsupported,
                system_audio: Access::Unsupported,
                unsupported: None,
            });
            let microphone_name = devices
                .await
                .ok()
                .and_then(|d| d.into_iter().find(|d| d.is_default))
                .map(|d| d.name);
            let _ = this.update(cx, |state, cx| {
                let fresh = Sources {
                    microphone: caps.microphone,
                    microphone_name,
                    system_audio: caps.system_audio,
                };
                if state.sources.as_ref() != Some(&fresh) {
                    state.sources = Some(fresh);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Grant: shows the macOS prompt when macOS still can, and opens System Settings for System
    /// Audio Recording, which has no prompt of its own.
    pub fn request_access(&mut self, source: Source, cx: &mut Context<Self>) {
        if !self.asking.insert(source) {
            return;
        }
        cx.notify();
        let ask = self.core.dory().recorder().request_access(source);
        cx.spawn(async move |this, cx| {
            let answer = ask.await;
            let _ = this.update(cx, |state, cx| {
                state.asking.remove(&source);
                match answer {
                    Ok(access) => {
                        if let Some(sources) = &mut state.sources {
                            match source {
                                Source::Microphone => sources.microphone = access,
                                Source::SystemAudio => sources.system_audio = access,
                            }
                        }
                        if onboarding::opens_settings_after_grant(source, access) {
                            cx.open_url(onboarding::settings_url(source));
                        }
                    }
                    Err(error) => state.fail(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Edits the settings in the core. `SettingsChanged` then refreshes [`AppState::settings`].
    pub fn update_settings(
        &mut self,
        edit: impl FnOnce(&mut Settings) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let done = self.core.run(cx, move |dory| dory.update_settings(edit));
        self.report(done, cx);
    }

    /// **Re-transcribe in…** (UX-26): pins the language, or returns it to Automatic, and runs the
    /// whole pipeline again. A job still in flight is cancelled first, so the transcript that
    /// lands is in the language just picked.
    pub fn retranscribe(
        &mut self,
        id: RecordingId,
        language: LanguagePref,
        cx: &mut Context<Self>,
    ) {
        let notice = match &language {
            LanguagePref::Auto => "Transcribing again in the detected language.".to_owned(),
            LanguagePref::Pinned(tag) => format!("Transcribing again in {}.", language_name(tag)),
        };
        let done = self.core.run(cx, move |dory| {
            dory.library().set_language(&id, language)?;
            for job in dory.library().jobs(&id)? {
                if !job.is_terminal() {
                    dory.pipeline().cancel(job.id)?;
                }
            }
            dory.pipeline().enqueue(&id, Goal::ALL)?;
            Ok::<_, DoryError>(())
        });
        self.report(done, cx);
        self.say(notice, cx);
    }

    pub fn retranscribe_selection(&mut self, language: LanguagePref, cx: &mut Context<Self>) {
        if let Some(id) = self.selection.id().cloned() {
            self.retranscribe(id, language, cx);
        }
    }
}
