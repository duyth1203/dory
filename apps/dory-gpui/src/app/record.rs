//! The record flow and processing on `AppState` (P5): recorder commands, the recorder's events,
//! progress, the time left and the phase of each recording, and the finish and fix actions.
//! Views call these methods; only this module tree reaches the facade.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use dory_core::app::{AppKind, Dory, Event, RecorderStatus, StartOptions};
use dory_core::domain::{
    Goal, JobId, JobState, Ms, RecordingId, RecordingRow, Segment, Stage, SummaryFlag,
};
use dory_core::models::{Availability, Diar, KindTag, ModelId, ModelRef, Stt};
use dory_core::ports::ProvisionError;
use dory_core::ports::capture::CaptureIssue;
use dory_core::recorder::{FinalizeReason, Limits};
use gpui_kit::Context;

use super::core::Core;
use super::{AppState, DetailTab, Download, ModelInfo, Saved};
use crate::a11y;
use crate::estimate::{self, Estimate, Left, Rate};
use crate::library::{self, Progress};
use crate::notify::{self, Note, NoteKind};
use crate::processing::{self, Facts, Outcome, Phase, Run, Spot};
use crate::record_control::Live;

/// Fills the model names the Download button shows.
pub(super) fn load_models(core: &Core, cx: &mut Context<AppState>) {
    let stt = core.dory().models().list::<Stt>();
    let diar = core.dory().models().list::<Diar>();
    cx.spawn(async move |this, cx| {
        let mut models = HashMap::new();
        let left_to_download = |availability: &Availability| match availability {
            Availability::NeedsProvision { bytes } => *bytes,
            _ => None,
        };
        for row in stt.await.into_iter().flatten() {
            let bytes = left_to_download(&row.status.availability);
            let entry = row.status.entry;
            models.insert(
                entry.id.erase(),
                ModelInfo {
                    kind: KindTag::Stt,
                    name: entry.display_name,
                    bytes,
                },
            );
        }
        for row in diar.await.into_iter().flatten() {
            let bytes = left_to_download(&row.status.availability);
            let entry = row.status.entry;
            models.insert(
                entry.id.erase(),
                ModelInfo {
                    kind: KindTag::Diar,
                    name: entry.display_name,
                    bytes,
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
        if self.recorder != RecorderStatus::Idle || self.shut_down {
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
        estimate::finish_estimate(audio, self.rates.any())
    }

    /// Where `row` is from Stop to its summary, P1 to P8 (`docs/design/processing-view.md`).
    pub fn phase(&self, row: &RecordingRow) -> Phase {
        let id = &row.recording.id;
        if let Some((phase, _)) = self.previews.get(id) {
            return phase.clone();
        }
        let job = self.queue.iter().find(|j| &j.recording == id);
        Phase::derive(&Facts {
            row,
            recorder: &self.recorder,
            grace_left: self.grace_left(),
            sealing: self.sealing.contains(id),
            progress: self.progress.get(id).copied(),
            done: job.map_or(&[], |j| j.done.as_slice()),
            spot: self.spot_of(id),
            left: self
                .estimates
                .get(id)
                .map_or(Left::Estimating, Estimate::left),
            auto_process: self.settings.auto_process,
            models: &self.models,
            downloads: &self.downloads,
        })
    }

    /// The phase of the recording `id`, if it is listed.
    pub fn phase_of(&self, id: &RecordingId) -> Option<Phase> {
        self.row(id).map(|row| self.phase(row))
    }

    pub fn row(&self, id: &RecordingId) -> Option<&RecordingRow> {
        self.recordings.iter().find(|r| &r.recording.id == id)
    }

    /// The audio's length: the saved take's, or the recorder's running total before it is saved.
    pub fn audio_of(&self, id: &RecordingId) -> Option<Ms> {
        match self.row(id)?.recording.take {
            Some(take) => Some(take.duration),
            None => (self.recorder.id() == Some(id) || self.sealing.contains(id))
                .then_some(self.last_recorded)
                .filter(|ms| ms.0 > 0),
        }
    }

    /// The lines the engine wrote for this recording's running job, oldest first (UX-12).
    pub fn drafts(&self, id: &RecordingId) -> &[Segment] {
        if let Some((_, lines)) = self.previews.get(id) {
            return lines;
        }
        self.drafts
            .iter()
            .find(|(job, _)| self.job_owner.get(*job) == Some(id))
            .map_or(&[], |(_, lines)| lines.as_slice())
    }

    /// The jobs running or waiting before `id`'s, and when it starts once each has an estimate.
    pub(super) fn spot_of(&self, id: &RecordingId) -> Option<Spot> {
        let active: Vec<_> = self
            .queue
            .iter()
            .filter(|j| matches!(j.state, JobState::Queued | JobState::Running { .. }))
            .collect();
        let ahead = active.iter().position(|j| &j.recording == id)?;
        let starts_in = active[..ahead]
            .iter()
            .try_fold(Ms(0), |sum, job| self.takes(job).map(|t| Ms(sum.0 + t.0)));
        Some(Spot {
            ahead,
            starts_in: starts_in.filter(|_| ahead > 0),
        })
    }

    pub(super) fn track(&mut self, id: RecordingId, progress: Progress) {
        let now = self.epoch.elapsed().as_millis() as u64;
        let fresh = Run {
            stage: progress.stage,
            pace: progress.budget.map(dory_core::app::Budget::pace),
            from: progress.fraction,
            at_ms: now,
        };
        let mut new_stage = false;
        let run = *self
            .runs
            .entry(id.clone())
            .and_modify(|run| {
                new_stage = run.stage != fresh.stage;
                if new_stage || run.pace != fresh.pace {
                    *run = fresh;
                }
            })
            .or_insert(fresh);
        if new_stage {
            self.estimates.remove(&id);
        }
        let raw = match progress.stage {
            Stage::Transcribe => {
                let audio = self.audio_of(&id);
                if let (Some(audio), Some(pace)) = (audio, run.pace)
                    && let Some(rate) =
                        Rate::observed(audio, run.from, progress.fraction, now - run.at_ms)
                {
                    self.rates.observe(pace, rate);
                }
                audio.and_then(|audio| {
                    let rate = run.pace.and_then(|pace| self.rates.at(pace));
                    estimate::transcribe_left(audio, progress.fraction, rate)
                })
            }
            Stage::Summarize => {
                estimate::summary_left(run.from, progress.fraction, now - run.at_ms)
            }
        };
        self.estimates
            .entry(id.clone())
            .or_default()
            .update(raw, now);
        self.progress.insert(id, progress);
    }

    /// Speaks the selected recording's sentence when its stage changes, and keeps its tab on
    /// what there is to read: the transcript while it is being written, then the summary,
    /// unless the user picked a tab (UX-12, `docs/design/processing-view.md` section 4).
    pub(super) fn follow_card(&mut self) {
        let Some(id) = self.selection.id().cloned() else {
            self.card = None;
            return;
        };
        let Some(row) = self.row(&id) else {
            return;
        };
        let phase = self.phase(row);
        let title = library::title(row, &chrono::Local);
        let before = match self.card.take() {
            Some((was, before)) if was == id => Some(before),
            _ => None,
        };
        if let Some(before) = &before
            && processing::announces(before, &phase)
        {
            a11y::announce(&phase.sentence(&title));
        }
        if !self.tab_chosen {
            let reading = phase.before_transcript() || matches!(phase, Phase::Summarizing { .. });
            let finished = before.as_ref().is_some_and(Phase::is_processing)
                && matches!(phase, Phase::Done { .. } | Phase::Failed(_));
            if reading {
                self.tab = DetailTab::Transcript;
            } else if finished {
                self.tab = DetailTab::Summary;
            }
        }
        self.card = Some((id, phase));
    }

    /// Pins `id` to `phase`, with `lines` streamed so far, whatever the core reports: for design
    /// previews and for the layout checks that walk every state of the card.
    pub fn preview_phase(
        &mut self,
        id: RecordingId,
        phase: Phase,
        lines: Vec<Segment>,
        cx: &mut Context<Self>,
    ) {
        self.previews.insert(id, (phase, lines));
        self.follow_card();
        cx.notify();
    }

    /// Use Fast This Time (UX-30): this one job runs fast, the setting stays.
    pub fn run_fast(&mut self, id: RecordingId, cx: &mut Context<Self>) {
        self.on_active_job(id, cx, |dory, job| dory.pipeline().run_fast(job));
    }

    fn on_active_job(
        &mut self,
        id: RecordingId,
        cx: &mut Context<Self>,
        act: impl FnOnce(&Dory, JobId) -> Result<(), dory_core::app::DoryError> + Send + 'static,
    ) {
        let done = self
            .core
            .run(cx, move |dory| match dory.library().jobs(&id)?.pop() {
                Some(job) if !job.is_terminal() => act(dory, job.id),
                _ => Ok(()),
            });
        self.report(done, cx);
    }

    /// **Show in Finder** (UX-13) for audio the app cannot read.
    pub fn show_in_finder(&mut self, id: RecordingId, cx: &mut Context<Self>) {
        let path = self
            .core
            .run(cx, move |dory| dory.library().playback_path(&id));
        cx.spawn(async move |this, cx| {
            let path = path.await;
            let _ = this.update(cx, |state, cx| match path {
                Ok(path) => cx.reveal_path(&path),
                Err(_) => state.say("The audio file is missing.", cx),
            });
        })
        .detach();
    }

    /// Posts `note` when its kind is on and the window can't show it (UX-14).
    fn notify(&self, kind: NoteKind, note: Note, cx: &mut Context<Self>) {
        notify::send(kind, note, &self.prefs.notifications, cx);
    }

    pub(super) fn on_recorder(&mut self, status: RecorderStatus, cx: &mut Context<Self>) {
        let before = std::mem::replace(&mut self.recorder, status.clone());
        if let (RecorderStatus::Recording { id, .. }, RecorderStatus::Idle) = (&status, &before) {
            let note = Note::recording_started(id, &self.title_or_new(id));
            self.notify(NoteKind::Recording, note, cx);
        }
        if let Some(said) = a11y::Announcement::of_recorder(&before, &status) {
            said.post();
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
        if let Some(recorded) = recorded {
            self.last_recorded = if new_take {
                recorded
            } else {
                self.last_recorded.max(recorded)
            };
        }
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
                self.last_recorded = recorded;
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
                a11y::Announcement::TimeLeft(remaining).post();
                self.notify(NoteKind::TimeLimit, Note::time_warning(), cx);
            }
            Event::Finished { id, reason } => {
                match reason {
                    FinalizeReason::CapReached => {
                        a11y::Announcement::CapReached.post();
                        self.notify(NoteKind::TimeLimit, Note::cap_reached(), cx);
                    }
                    FinalizeReason::Stopped => {
                        let note = Note::recording_saved(&id, &self.title_or_new(&id));
                        self.notify(NoteKind::Recording, note, cx);
                    }
                    FinalizeReason::Shutdown => {}
                }
                self.sealing.insert(id);
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
                let name = self.model_name(&model);
                match result {
                    Ok(()) => self.notify(NoteKind::Download, Note::model_downloaded(&name), cx),
                    Err(error) if error != ProvisionError::Cancelled.to_string() => {
                        self.say(format!("Could not download {name}: {error}"), cx);
                    }
                    Err(_) => {}
                }
            }
            Event::Error { context, message } => {
                self.say(format!("Problem {context}: {message}"), cx);
            }
            _ => {}
        }
    }

    /// After each reload: drop progress and lines of jobs that moved on, announce and notify
    /// jobs that just ended, and release takes whose audio is now listed. The selected
    /// recording's card speaks its own sentence instead (`follow_card`).
    pub(super) fn on_rows(&mut self, before: &[RecordingRow], cx: &mut Context<Self>) {
        let running: HashMap<RecordingId, Stage> = self
            .recordings
            .iter()
            .filter_map(|r| match r.job {
                Some(JobState::Running { stage }) => Some((r.recording.id.clone(), stage)),
                _ => None,
            })
            .collect();
        self.progress
            .retain(|id, p| running.get(id) == Some(&p.stage));
        self.runs
            .retain(|id, run| running.get(id) == Some(&run.stage));
        let runs = &self.runs;
        self.estimates.retain(|id, _| runs.contains_key(id));
        let owners = &self.job_owner;
        self.drafts.retain(|job, _| {
            owners
                .get(job)
                .is_none_or(|owner| running.get(owner) == Some(&Stage::Transcribe))
        });
        let selected = self.selection.id().cloned();
        for id in processing::transcripts_ready(before, &self.recordings) {
            let note = Note::transcript_ready(&id, &self.title_or_new(&id));
            self.notify(NoteKind::Transcript, note, cx);
        }
        for (id, outcome) in processing::outcomes(before, &self.recordings) {
            let Some(row) = self.recordings.iter().find(|r| r.recording.id == id) else {
                continue;
            };
            let title = library::title(row, &chrono::Local);
            let speaks = selected.as_ref() != Some(&id);
            match outcome {
                Outcome::Finished => {
                    if speaks {
                        a11y::Announcement::Finished {
                            title: title.clone(),
                        }
                        .post();
                    }
                    if row.summary != SummaryFlag::None {
                        let note = Note::summary_ready(&id, &title);
                        self.notify(NoteKind::Summary, note, cx);
                    }
                }
                Outcome::Failed => {
                    if speaks {
                        a11y::Announcement::Failed {
                            title: title.clone(),
                        }
                        .post();
                    }
                    if let Phase::Failed(problem) = self.phase(row) {
                        let note = Note::needs_attention(&id, &title, &problem.text());
                        self.notify(NoteKind::Attention, note, cx);
                    }
                }
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
        self.follow_card();
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
