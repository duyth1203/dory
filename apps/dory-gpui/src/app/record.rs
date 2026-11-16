//! The record flow and processing on `AppState` (P5): recorder commands, the recorder's events,
//! progress and the time left, and the finish and fix actions. Views call these methods; only
//! this module tree reaches the facade.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use dory_core::app::{AppKind, Dory, Event, RecorderStatus, StartOptions};
use dory_core::domain::{Blocker, Fraction, Goal, JobState, Ms, RecordingId, RecordingRow, Stage};
use dory_core::models::{Diar, KindTag, ModelId, ModelRef, Stt};
use dory_core::ports::ProvisionError;
use dory_core::ports::capture::CaptureIssue;
use dory_core::recorder::{FinalizeReason, Limits};
use gpui_kit::Context;

use super::core::Core;
use super::{AppState, Saved};
use crate::library::{self, Percent, Progress, RowStatus};
use crate::processing::{self, Outcome, Rate, Run};
use crate::record_control::{self, Live};
use crate::{a11y, notify};

/// A model download in flight, from `ProvisionProgress`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Download {
    pub done: u64,
    pub total: Option<u64>,
}

impl Download {
    pub fn percent(self) -> Option<Percent> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| Percent::of(Fraction::new(self.done as f32 / t as f32)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub kind: KindTag,
    pub name: String,
}

/// Fills the model names the Download button shows.
pub(super) fn load_models(core: &Core, cx: &mut Context<AppState>) {
    let stt = core.dory().models().list::<Stt>();
    let diar = core.dory().models().list::<Diar>();
    cx.spawn(async move |this, cx| {
        let mut models = HashMap::new();
        for row in stt.await.into_iter().flatten() {
            let entry = row.status.entry;
            models.insert(
                entry.id.erase(),
                ModelInfo {
                    kind: KindTag::Stt,
                    name: entry.display_name,
                },
            );
        }
        for row in diar.await.into_iter().flatten() {
            let entry = row.status.entry;
            models.insert(
                entry.id.erase(),
                ModelInfo {
                    kind: KindTag::Diar,
                    name: entry.display_name,
                },
            );
        }
        let _ = this.update(cx, |state, cx| {
            state.models = models;
            cx.notify();
        });
    })
    .detach();
}

pub(super) fn load_settings(core: &Core, cx: &mut Context<AppState>) {
    let settings = core.run(cx, |dory| dory.settings());
    cx.spawn(async move |this, cx| {
        if let Ok(settings) = settings.await {
            let _ = this.update(cx, |state, cx| {
                state.settings = settings;
                cx.notify();
            });
        }
    })
    .detach();
}

type Provision = Pin<Box<dyn Future<Output = Result<(), String>>>>;

fn provision<K: AppKind>(dory: &Dory, model: &ModelRef) -> Provision {
    match model.as_str().parse::<ModelId<K>>() {
        Ok(id) => {
            let done = dory.models().provision(id);
            Box::pin(async move { done.await.map_err(|e| e.to_string()) })
        }
        Err(error) => {
            let error = error.to_string();
            Box::pin(async move { Err(error) })
        }
    }
}

impl AppState {
    pub fn live(&self) -> &gpui_kit::Entity<Live> {
        &self.live
    }

    /// Seconds left on the Undo toast. `Some` only while the recorder is in grace.
    pub fn grace_left(&self) -> Option<u8> {
        matches!(self.recorder, RecorderStatus::Grace { .. }).then_some(self.grace_left.max(1))
    }

    /// Stopped and not yet listed with its audio: in grace, or sealing.
    pub fn is_saving(&self, id: &RecordingId) -> bool {
        matches!(&self.recorder, RecorderStatus::Grace { id: g, .. } if g == id)
            || self.sealing.contains(id)
    }

    /// Settings > Transcription > Process recordings automatically (UX-11).
    pub fn auto_process(&self) -> bool {
        self.settings.auto_process
    }

    /// The toolbar button, File > New Recording, the start screen, and the menu-bar extra.
    pub fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.recorder != RecorderStatus::Idle {
            return;
        }
        eprintln!("record-requested: {} ms", self.epoch.elapsed().as_millis());
        self.command(cx, |dory| dory.recorder().start(StartOptions::default()));
    }

    pub fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        match self.recorder {
            RecorderStatus::Recording { .. } => self.command(cx, |dory| dory.recorder().pause()),
            RecorderStatus::Paused { .. } => self.command(cx, |dory| dory.recorder().resume()),
            _ => {}
        }
    }

    pub fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.recorder,
            RecorderStatus::Recording { .. } | RecorderStatus::Paused { .. }
        ) {
            self.command(cx, |dory| dory.recorder().stop());
        }
    }

    /// Resumes into the same take while the grace period lasts.
    pub fn undo_stop(&mut self, cx: &mut Context<Self>) {
        if matches!(self.recorder, RecorderStatus::Grace { .. }) {
            self.command(cx, |dory| dory.recorder().undo_stop());
        }
    }

    /// Recorder commands only post to the actor, so they run here without blocking a frame.
    fn command(
        &mut self,
        cx: &mut Context<Self>,
        send: impl FnOnce(&Dory) -> Result<(), dory_core::app::RecorderGone>,
    ) {
        if let Err(error) = send(self.core.dory()) {
            self.fail(error.to_string(), cx);
        }
    }

    /// The finish card's buttons (UX-11). `always` turns automatic processing back on.
    pub fn finish(&mut self, id: RecordingId, goal: Goal, always: bool, cx: &mut Context<Self>) {
        let done = self.core.run(cx, move |dory| {
            dory.pipeline().enqueue(&id, goal)?;
            if always {
                dory.update_settings(|settings| settings.auto_process = true)?;
            }
            Ok::<_, dory_core::app::DoryError>(())
        });
        self.report(done, cx);
    }

    /// **Try again** (UX-13): unblocks a blocked job, retries a failed or cancelled one, and
    /// queues a recording that has none.
    pub fn try_again(&mut self, id: RecordingId, cx: &mut Context<Self>) {
        let done = self.core.run(cx, move |dory| {
            let pipeline = dory.pipeline();
            match dory.library().jobs(&id)?.pop() {
                Some(job) if matches!(job.state, JobState::Blocked { .. }) => {
                    pipeline.unblock(job.id)
                }
                Some(job) if matches!(job.state, JobState::Failed { .. } | JobState::Cancelled) => {
                    pipeline.retry(job.id)
                }
                Some(_) => Ok(()),
                None => pipeline.enqueue(&id, Goal::ALL),
            }
        });
        self.report(done, cx);
    }

    pub(super) fn report<E: std::fmt::Display + 'static>(
        &mut self,
        done: gpui_kit::Task<Result<(), E>>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            if let Err(error) = done.await {
                let _ = this.update(cx, |state, cx| state.fail(error.to_string(), cx));
            }
        })
        .detach();
    }

    /// **Download <model>** (UX-13). Progress shows in the row; blocked jobs rerun once it lands.
    pub fn download(&mut self, model: ModelRef, cx: &mut Context<Self>) {
        let kind = self.models.get(&model).map_or(KindTag::Stt, |m| m.kind);
        let done = match kind {
            KindTag::Diar => provision::<Diar>(self.core.dory(), &model),
            _ => provision::<Stt>(self.core.dory(), &model),
        };
        self.downloads.insert(
            model.clone(),
            Download {
                done: 0,
                total: None,
            },
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            if let Err(error) = done.await {
                let _ = this.update(cx, |state, cx| {
                    state.downloads.remove(&model);
                    state.say(format!("Could not download {model}: {error}"), cx);
                });
            }
        })
        .detach();
    }

    pub fn model_name(&self, model: &ModelRef) -> String {
        self.models
            .get(model)
            .map_or_else(|| model.to_string(), |m| m.name.clone())
    }

    pub fn download_of(&self, model: &ModelRef) -> Option<Download> {
        self.downloads.get(model).copied()
    }

    /// The finish card's sentence, at the rate seen on this Mac so far.
    pub fn finish_estimate(&self, audio: Ms) -> String {
        processing::finish_estimate(audio, self.rate)
    }

    /// The time the running transcription still takes, once a minute of audio is done.
    pub fn time_left(&self, id: &RecordingId) -> Option<Ms> {
        let progress = self.progress.get(id)?;
        if progress.stage != Stage::Transcribe {
            return None;
        }
        processing::time_left(self.audio_of(id)?, progress.fraction, self.rate?)
    }

    /// The status words a row and the detail show: "Transcribing, 40%, about 3 min left", with
    /// "· Light, on battery" after it while the job runs light, or "Downloading, 12%" while the
    /// model a blocked job needs downloads.
    pub fn status_text(&self, row: &RecordingRow) -> String {
        let status = self.status(row);
        if let Some(JobState::Blocked {
            blocker: Blocker::NeedsProvision { model, .. },
            ..
        }) = &row.job
            && let Some(download) = self.downloads.get(model)
        {
            return match download.percent() {
                Some(p) => format!("Downloading, {}%", p.get()),
                None => "Downloading".into(),
            };
        }
        let words = match (status, self.time_left(&row.recording.id)) {
            (RowStatus::Transcribing(Some(_)), Some(left)) => {
                format!("{}, {}", status.label(), processing::left_label(left))
            }
            _ => status.label(),
        };
        let light = self
            .progress
            .get(&row.recording.id)
            .and_then(|p| p.budget.reason())
            .filter(|_| matches!(status, RowStatus::Transcribing(_) | RowStatus::Summarizing));
        match light {
            Some(reason) => format!("{words} · {}", processing::light_label(reason)),
            None => words,
        }
    }

    fn audio_of(&self, id: &RecordingId) -> Option<Ms> {
        self.recordings
            .iter()
            .find(|r| &r.recording.id == id)?
            .recording
            .take
            .map(|t| t.duration)
    }

    pub(super) fn track(&mut self, id: RecordingId, progress: Progress) {
        let now = self.epoch.elapsed().as_millis() as u64;
        let fresh = Run {
            stage: progress.stage,
            pace: progress.budget.pace(),
            from: progress.fraction,
            at_ms: now,
        };
        let run = *self
            .runs
            .entry(id.clone())
            .and_modify(|run| {
                if run.stage != fresh.stage || run.pace != fresh.pace {
                    *run = fresh;
                }
            })
            .or_insert(fresh);
        if progress.stage == Stage::Transcribe
            && let Some(audio) = self.audio_of(&id)
            && let Some(rate) = Rate::observed(audio, run.from, progress.fraction, now - run.at_ms)
        {
            self.rate = Some(rate);
        }
        self.progress.insert(id, progress);
    }

    pub(super) fn on_recorder(&mut self, status: RecorderStatus, cx: &mut Context<Self>) {
        let before = std::mem::replace(&mut self.recorder, status.clone());
        if let Some(words) = record_control::announcement(&before, &status) {
            a11y::announce(words);
        }
        let new_take = status.id().is_some() && before.id() != status.id();
        if new_take {
            self.reselect = status.id().cloned();
        }
        let recorded = match &status {
            RecorderStatus::Idle => None,
            RecorderStatus::Recording { recorded, .. }
            | RecorderStatus::Paused { recorded, .. }
            | RecorderStatus::Grace { recorded, .. } => Some(*recorded),
        };
        self.live.update(cx, |live, cx| {
            match recorded {
                Some(recorded) if !new_take => live.recorded = live.recorded.max(recorded),
                Some(recorded) => {
                    *live = Live {
                        recorded,
                        ..Live::default()
                    }
                }
                None => *live = Live::default(),
            }
            cx.notify();
        });
        let entering_grace = matches!(status, RecorderStatus::Grace { .. })
            && !matches!(before, RecorderStatus::Grace { .. });
        if entering_grace {
            self.count_down_grace(cx);
        }
    }

    fn count_down_grace(&mut self, cx: &mut Context<Self>) {
        self.grace_left = (Limits::default().grace_ms / 1000) as u8;
        self.grace_generation += 1;
        let generation = self.grace_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let more = this.update(cx, |state, cx| {
                    if state.grace_generation != generation || state.grace_left <= 1 {
                        return false;
                    }
                    state.grace_left -= 1;
                    cx.notify();
                    true
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn on_record_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Levels { peaks, recorded } => {
                let epoch = self.epoch;
                self.live.update(cx, |live, cx| {
                    if !live.sampled {
                        live.sampled = true;
                        eprintln!("first-sample: {} ms", epoch.elapsed().as_millis());
                    }
                    live.level = peaks.iter().copied().fold(0.0, f32::max);
                    live.recorded = recorded;
                    cx.notify();
                });
            }
            Event::TimeWarning { remaining } => {
                self.live.update(cx, |live, cx| {
                    live.cap_at = Some(Ms(live.recorded.0 + remaining.0));
                    cx.notify();
                });
                a11y::announce(&format!(
                    "{} of recording left",
                    a11y::spoken_duration(remaining)
                ));
                notify::time_warning(cx);
            }
            Event::Finished { id, reason } => {
                self.sealing.insert(id);
                if reason == FinalizeReason::CapReached {
                    a11y::announce("The recording reached its time limit and was saved");
                    notify::cap_reached(cx);
                }
            }
            Event::CaptureIssue(issue) => {
                if let Some(text) = issue_text(&issue) {
                    self.say(text, cx);
                }
            }
            Event::ProvisionProgress {
                model, done, total, ..
            } => {
                self.downloads.insert(model, Download { done, total });
            }
            Event::ProvisionFinished { model, result } => {
                self.downloads.remove(&model);
                if let Err(error) = result
                    && error != ProvisionError::Cancelled.to_string()
                {
                    let name = self.model_name(&model);
                    self.say(format!("Could not download {name}: {error}"), cx);
                }
            }
            Event::Error { context, message } => {
                self.say(format!("Problem {context}: {message}"), cx);
            }
            _ => {}
        }
    }

    /// After each reload: announce and notify jobs that just ended, and release takes whose
    /// audio is now listed.
    pub(super) fn on_rows(&mut self, before: &[RecordingRow], cx: &mut Context<Self>) {
        for (id, outcome) in processing::outcomes(before, &self.recordings) {
            let Some(row) = self.recordings.iter().find(|r| r.recording.id == id) else {
                continue;
            };
            let title = library::title(row, &chrono::Local);
            match outcome {
                Outcome::Finished => {
                    a11y::announce(&format!("Finished processing {title}"));
                    if cx.active_window().is_none() {
                        notify::job_finished(&id, &title, cx);
                    }
                }
                Outcome::Failed => a11y::announce(&format!("{title} needs attention")),
            }
        }
        let saved: Vec<RecordingId> = self
            .recordings
            .iter()
            .filter(|r| self.sealing.contains(&r.recording.id) && r.recording.take.is_some())
            .map(|r| r.recording.id.clone())
            .collect();
        for id in saved {
            self.sealing.remove(&id);
            cx.emit(Saved(id));
        }
    }
}

fn issue_text(issue: &CaptureIssue) -> Option<String> {
    use dory_core::domain::Channel;
    let source = |channel: &Channel| match channel {
        Channel::Mic => "The microphone",
        Channel::System => "System audio",
        Channel::Mixed => "The audio source",
    };
    Some(match issue {
        CaptureIssue::SourceLost(channel) => format!("{} stopped sending audio.", source(channel)),
        CaptureIssue::SourceRestored(channel) => format!("{} is back.", source(channel)),
        CaptureIssue::MicChanged { name } => format!("Now recording from {name}."),
        CaptureIssue::SystemAudioSilent => "System audio is silent.".into(),
        CaptureIssue::Fatal(message) => format!("Recording stopped: {message}"),
        CaptureIssue::Rebuilt { .. } => return None,
    })
}
