//! The processing view's rules (`docs/design/processing-view.md` section 4, UX-11 to UX-13,
//! UX-30) as plain data and pure functions. [`Phase::derive`] turns what the core reports about
//! one recording into one of the states P1 to P8, and each state words itself: the card's
//! headline, detail, steps, and VoiceOver sentence, and the sidebar row's glyph and word. Nothing
//! here touches GPUI or the store.

use std::collections::HashMap;

use dory_core::app::{LightReason, RecorderStatus};
use dory_core::domain::{
    Blocker, FailureCause, FailureClass, Fraction, JobState, Ms, Processing, RecordingId,
    RecordingRow, Stage, SummaryFlag,
};
use dory_core::models::{KindTag, ModelRef, UnavailableReason};
use dory_core::ports::Pace;

use crate::estimate::Left;
use crate::library::{Percent, Progress, RowStatus};

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

/// A speech or speaker model the Download button names, from the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub kind: KindTag,
    pub name: String,
    /// What is left to download.
    pub bytes: Option<u64>,
}

/// Where a queued job stands: the jobs running or waiting before it, and when it starts, once
/// every one of them has an estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub ahead: usize,
    pub starts_in: Option<Ms>,
}

/// Everything one recording's phase is derived from. `AppState` gathers it from the core's rows,
/// queue, events, and its own estimates.
pub struct Facts<'a> {
    pub row: &'a RecordingRow,
    pub recorder: &'a RecorderStatus,
    /// Seconds left in the Undo window while this recording is in it.
    pub grace_left: Option<u8>,
    /// Closed and not yet listed with its audio.
    pub sealing: bool,
    /// The last progress of this recording's running stage.
    pub progress: Option<Progress>,
    /// Stages the recording's active job already finished.
    pub done: &'a [Stage],
    /// The job still has the translate stage ahead: the recording is being written in the
    /// output language.
    pub translating: bool,
    /// `None` until the queue is read.
    pub spot: Option<Spot>,
    /// The smoothed time left of the running stage.
    pub left: Left,
    /// Settings > Transcription > Process recordings automatically.
    pub auto_process: bool,
    pub models: &'a HashMap<ModelRef, ModelInfo>,
    pub downloads: &'a HashMap<ModelRef, Download>,
}

/// Where a recording is on its way from Stop to a summary. P1 to P8 are the spec's names.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Recording,
    /// P1: the Undo window.
    Stopped {
        starts_in: u8,
        auto: bool,
    },
    /// P2: sealing the take.
    Saving,
    /// Audio and no job: the finish card (UX-11).
    Ready,
    /// P3a.
    Queued(Spot),
    /// P3b: the transcribe stage started, and no window yet.
    Preparing,
    /// P4, and P5 when `light` says why it runs light.
    Transcribing {
        done: Fraction,
        left: Left,
        light: Option<LightReason>,
    },
    /// Between P4 and P6, when an output language is set: the transcript is ready to read as
    /// spoken, and Dory writes it in the output language.
    Translating {
        done: Fraction,
    },
    /// P6.
    Summarizing {
        left: Left,
    },
    /// P7.
    Done {
        summary: bool,
    },
    /// P8.
    Failed(Problem),
}

/// Why a job stopped, in the terms UX-13 gives the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    SpeechModelMissing {
        model: ModelRef,
        kind: KindTag,
        name: String,
        bytes: Option<u64>,
        download: Option<Download>,
    },
    /// The catalog has no speech model, or the choice is Off.
    NoSpeechModel,
    DiskFull,
    Crashed,
    AudioUnreadable,
    /// Anything else the transcribe stage reports, in the core's words.
    Transcription {
        text: String,
        started: bool,
    },
    SummaryModelMissing,
    /// Anything else the summary stage reports, in the core's words.
    Summary(String),
    /// The translate stage failed, in the core's words.
    Translation(String),
}

/// The one button a stopped job offers (UX-13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    Download(ModelRef),
    ChooseSpeechModel,
    SetUpSummaries,
    FreeUpSpace,
    TryAgain,
    ShowInFinder,
}

/// The glyph before a step's word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepGlyph {
    Stop,
    /// Under way: three dots that breathe.
    Dots,
    Check,
    Clock,
    /// Still to come.
    Pending,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: &'static str,
    pub glyph: StepGlyph,
    pub word: String,
}

/// What the transcript tab shows before there is a transcript to read.
pub const PLACEHOLDER: &str = "The transcript appears here, line by line, as it is written.";
/// The footnote under the lines that stream in while transcribing.
pub const FOOTNOTE: &str = "Speaker names are added when transcription finishes.";

impl Phase {
    pub fn derive(facts: &Facts<'_>) -> Phase {
        let row = facts.row;
        let id = &row.recording.id;
        if matches!(facts.recorder, RecorderStatus::Recording { id: r, .. } | RecorderStatus::Paused { id: r, .. } if r == id)
        {
            return Phase::Recording;
        }
        if let RecorderStatus::Grace { id: g, .. } = facts.recorder
            && g == id
        {
            return Phase::Stopped {
                starts_in: facts.grace_left.unwrap_or(1).max(1),
                auto: facts.auto_process,
            };
        }
        if facts.sealing {
            return Phase::Saving;
        }
        let progress = |stage| facts.progress.filter(|p| p.stage == stage);
        match &row.job {
            Some(JobState::Queued) if facts.done.contains(&Stage::Transcribe) => {
                if facts.translating && !facts.done.contains(&Stage::Translate) {
                    Phase::Translating {
                        done: Fraction::new(0.0),
                    }
                } else {
                    Phase::Summarizing { left: facts.left }
                }
            }
            Some(JobState::Queued) => Phase::Queued(facts.spot.unwrap_or(Spot {
                ahead: 0,
                starts_in: None,
            })),
            Some(JobState::Running {
                stage: Stage::Transcribe,
            }) => match progress(Stage::Transcribe) {
                Some(p) => Phase::Transcribing {
                    done: p.fraction,
                    left: facts.left,
                    light: p.budget.and_then(|b| b.reason()),
                },
                None => Phase::Preparing,
            },
            Some(JobState::Running {
                stage: Stage::Translate,
            }) => Phase::Translating {
                done: progress(Stage::Translate).map_or(Fraction::new(0.0), |p| p.fraction),
            },
            Some(JobState::Running {
                stage: Stage::Summarize,
            }) => Phase::Summarizing { left: facts.left },
            Some(job @ (JobState::Blocked { .. } | JobState::Failed { .. })) => {
                Phase::Failed(Problem::of(job, facts.models, facts.downloads))
            }
            Some(JobState::Succeeded) => Phase::Done {
                summary: row.summary != SummaryFlag::None,
            },
            Some(JobState::Cancelled) | None if row.has_transcript => Phase::Done {
                summary: row.summary != SummaryFlag::None,
            },
            Some(JobState::Cancelled) | None => Phase::Ready,
        }
    }

    /// The card shows from Stop until the summary is ready. Done, and a summary that failed
    /// after the transcript, show the player instead.
    pub fn is_processing(&self) -> bool {
        match self {
            Phase::Stopped { .. }
            | Phase::Saving
            | Phase::Queued(_)
            | Phase::Preparing
            | Phase::Transcribing { .. }
            | Phase::Translating { .. }
            | Phase::Summarizing { .. } => true,
            Phase::Failed(problem) => problem.stage() == Stage::Transcribe,
            Phase::Recording | Phase::Ready | Phase::Done { .. } => false,
        }
    }

    /// No transcript is ready to read yet, so the transcript tab shows the placeholder or the
    /// lines as they stream in.
    pub fn before_transcript(&self) -> bool {
        match self {
            Phase::Stopped { .. }
            | Phase::Saving
            | Phase::Queued(_)
            | Phase::Preparing
            | Phase::Transcribing { .. } => true,
            Phase::Failed(problem) => problem.stage() == Stage::Transcribe,
            _ => false,
        }
    }

    pub fn headline(&self) -> Option<String> {
        Some(
            match self {
                Phase::Stopped { .. } => "Recording stopped",
                Phase::Saving => "Saving the recording",
                Phase::Queued(_) => "Waiting to transcribe",
                Phase::Preparing => "Getting ready to transcribe",
                Phase::Transcribing { .. } => "Transcribing",
                Phase::Translating { .. } => "Translating the transcript",
                Phase::Summarizing { .. } => "Writing the summary",
                Phase::Failed(problem) => return Some(problem.headline().into()),
                Phase::Recording | Phase::Ready | Phase::Done { .. } => return None,
            }
            .into(),
        )
    }

    /// The line under the headline. `audio` is the take's length as far as it is known.
    pub fn detail(&self, audio: Option<Ms>) -> Option<String> {
        Some(match self {
            Phase::Stopped { starts_in, auto } => {
                let seconds = seconds(*starts_in);
                if *auto {
                    format!("Transcription starts in {seconds}.")
                } else {
                    format!("You can undo for {seconds}.")
                }
            }
            Phase::Saving => match audio.map(minutes_of_audio) {
                Some(minutes) => format!("{minutes} of audio. This takes a moment."),
                None => "This takes a moment.".into(),
            },
            Phase::Queued(spot) => {
                let ahead = match spot.ahead {
                    0 => "This recording is next.".to_owned(),
                    1 => "1 recording is ahead of this one.".to_owned(),
                    n => format!("{n} recordings are ahead of this one."),
                };
                match spot.starts_in {
                    Some(at) if spot.ahead > 0 => {
                        format!("{ahead} It starts {}.", Left::starts(at))
                    }
                    _ => ahead,
                }
            }
            Phase::Preparing => "Loading the speech model. This takes a few seconds.".into(),
            Phase::Transcribing { done, left, .. } => {
                format!("{}% · {}", Percent::of(*done).get(), left.short())
            }
            Phase::Translating { done } => format!(
                "The transcript is ready to read as it was spoken. {}% translated.",
                Percent::of(*done).get()
            ),
            Phase::Summarizing { left } => match left {
                Left::Estimating => "The transcript is ready to read.".into(),
                Left::About(ms) if ms.0 < 60_000 => {
                    "The transcript is ready to read. Summary in less than 1 min.".into()
                }
                Left::About(ms) => format!(
                    "The transcript is ready to read. Summary in about {} min.",
                    ms.0.div_ceil(60_000)
                ),
            },
            Phase::Failed(problem) => problem.detail(),
            Phase::Recording | Phase::Ready | Phase::Done { .. } => return None,
        })
    }

    /// Recording, Transcript, Summary, each with its glyph and word.
    pub fn steps(&self) -> Option<[Step; 3]> {
        use StepGlyph::*;
        let step = |name, glyph, word: &str| Step {
            name,
            glyph,
            word: word.to_owned(),
        };
        let saved = step("Recording", Check, "Saved");
        let waiting = |name| step(name, Pending, "Waiting");
        Some(match self {
            Phase::Stopped { starts_in, auto } => [
                step("Recording", Stop, "Stopped"),
                if *auto {
                    step("Transcript", Pending, &format!("Starts in {starts_in} s"))
                } else {
                    waiting("Transcript")
                },
                waiting("Summary"),
            ],
            Phase::Saving => [
                step("Recording", Dots, "Saving"),
                waiting("Transcript"),
                waiting("Summary"),
            ],
            Phase::Queued(spot) => [
                saved,
                match spot.ahead {
                    0 => step("Transcript", Clock, "Next"),
                    n => step("Transcript", Clock, &format!("{n} ahead")),
                },
                waiting("Summary"),
            ],
            Phase::Preparing => [
                saved,
                step("Transcript", Dots, "Preparing"),
                waiting("Summary"),
            ],
            Phase::Transcribing { done, light, .. } => {
                let percent = Percent::of(*done).get();
                let word = match light {
                    Some(_) => format!("{percent}%, light"),
                    None => format!("{percent}%"),
                };
                [saved, step("Transcript", Dots, &word), waiting("Summary")]
            }
            Phase::Translating { done } => [
                saved,
                step(
                    "Transcript",
                    Dots,
                    &format!("Translating {}%", Percent::of(*done).get()),
                ),
                waiting("Summary"),
            ],
            Phase::Summarizing { .. } => [
                saved,
                step("Transcript", Check, "Ready"),
                step("Summary", Dots, "Writing"),
            ],
            Phase::Failed(problem) if problem.stage() == Stage::Transcribe => [
                saved,
                step("Transcript", Warning, problem.step_word()),
                waiting("Summary"),
            ],
            Phase::Failed(_) | Phase::Recording | Phase::Ready | Phase::Done { .. } => {
                return None;
            }
        })
    }

    /// The card's `role=status` text. `title` names the recording once it is done.
    pub fn sentence(&self, title: &str) -> String {
        match self {
            Phase::Recording => "Recording.".into(),
            Phase::Ready => "Ready to transcribe.".into(),
            Phase::Stopped { .. } => {
                format!(
                    "Recording stopped. {}",
                    self.detail(None).unwrap_or_default()
                )
            }
            Phase::Saving => "Saving the recording.".into(),
            Phase::Queued(spot) => {
                let ahead = match spot.ahead {
                    0 => return "Waiting to transcribe. Next in line.".into(),
                    1 => "1 recording ahead".to_owned(),
                    n => format!("{n} recordings ahead"),
                };
                match spot.starts_in {
                    Some(at) => format!(
                        "Waiting to transcribe. {ahead}, starts {}.",
                        spoken_start(at)
                    ),
                    None => format!("Waiting to transcribe. {ahead}."),
                }
            }
            Phase::Preparing => "Getting ready to transcribe. Loading the speech model.".into(),
            Phase::Transcribing { done, left, light } => {
                let percent = Percent::of(*done).get();
                let mut said = match left {
                    Left::Estimating => format!("Transcribing, {percent}%. Estimating time left."),
                    Left::About(_) => format!("Transcribing, {percent}%, {}.", left.spoken()),
                };
                if let Some(reason) = light {
                    said.push_str(&format!(" Running light because {}.", because(*reason)));
                }
                said
            }
            Phase::Translating { done } => format!(
                "Transcript ready as spoken. Translating it, {}%.",
                Percent::of(*done).get()
            ),
            Phase::Summarizing { left } => match left {
                Left::Estimating => "Transcript ready. Writing the summary.".into(),
                Left::About(_) => {
                    format!("Transcript ready. Writing the summary, {}.", left.spoken())
                }
            },
            Phase::Done { summary: true } => format!("Summary ready for {title}."),
            Phase::Done { summary: false } => format!("Transcript ready for {title}."),
            Phase::Failed(problem) => problem.sentence(),
        }
    }

    pub fn row(&self) -> RowStatus {
        match self {
            Phase::Recording => RowStatus::Recording,
            Phase::Stopped { .. } => RowStatus::Stopped,
            Phase::Saving => RowStatus::Saving,
            Phase::Ready => RowStatus::ReadyToTranscribe,
            Phase::Queued(spot) => RowStatus::Queued { ahead: spot.ahead },
            Phase::Preparing => RowStatus::Preparing,
            Phase::Transcribing { done, light, .. } => RowStatus::Transcribing {
                percent: Percent::of(*done),
                light: light.is_some(),
            },
            Phase::Translating { .. } => RowStatus::Translating,
            Phase::Summarizing { .. } => RowStatus::Summarizing,
            Phase::Done { .. } => RowStatus::Done,
            Phase::Failed(_) => RowStatus::NeedsAttention,
        }
    }

    /// The chip that says why transcription runs light, with Use Fast This Time (P5).
    pub fn light(&self) -> Option<LightReason> {
        match self {
            Phase::Transcribing { light, .. } => *light,
            _ => None,
        }
    }

    /// Move to Top sits in the headline row while other jobs are ahead (UX-15).
    pub fn can_move_to_top(&self) -> bool {
        matches!(self, Phase::Queued(spot) if spot.ahead > 0)
    }

    /// Whether the waveform draws the reading head: only while transcription moves through the
    /// audio. Saving, queued, and preparing show no head, so it never sits idle at 0:00.
    pub fn has_reading_head(&self) -> bool {
        matches!(self, Phase::Transcribing { .. })
    }

    /// How far transcription got through the audio, for the waveform and its caption.
    pub fn transcribed(&self) -> Fraction {
        match self {
            Phase::Transcribing { done, .. } => *done,
            Phase::Translating { .. } | Phase::Summarizing { .. } | Phase::Done { .. } => {
                Fraction::new(1.0)
            }
            Phase::Failed(problem) if problem.stage() != Stage::Transcribe => Fraction::new(1.0),
            _ => Fraction::new(0.0),
        }
    }

    /// What the transcript tab shows while there is nothing to read yet.
    pub fn placeholder(&self) -> &'static str {
        match self {
            Phase::Failed(Problem::SpeechModelMissing { .. }) => {
                "The transcript appears here once the speech model is installed."
            }
            Phase::Failed(Problem::NoSpeechModel) => {
                "The transcript appears here once a speech model is set up."
            }
            Phase::Failed(_) => "The transcript appears here once transcription finishes.",
            _ => PLACEHOLDER,
        }
    }

    /// The fix while the job is stopped, unless a download is already running.
    pub fn fix(&self) -> Option<Fix> {
        match self {
            Phase::Failed(problem) => problem.fix(),
            _ => None,
        }
    }

    /// The stage this phase stands for, so VoiceOver speaks when it changes and not on each
    /// percent, countdown second, or estimate (UX-12). A new problem is a new stage.
    fn stage_key(&self) -> (u8, Option<std::mem::Discriminant<Problem>>) {
        let key = match self {
            Phase::Recording => 0,
            Phase::Stopped { .. } => 1,
            Phase::Saving => 2,
            Phase::Ready => 3,
            Phase::Queued(_) => 4,
            Phase::Preparing => 5,
            Phase::Transcribing { .. } => 6,
            Phase::Summarizing { .. } => 7,
            Phase::Done { .. } => 8,
            Phase::Failed(problem) => return (9, Some(std::mem::discriminant(problem))),
            Phase::Translating { .. } => 10,
        };
        (key, None)
    }
}

/// Whether VoiceOver should speak the card's sentence when the selected recording goes from
/// `before` to `after`: when the stage changes, when processing fails, and when it is done.
/// Stop has its own announcement ("Recording saved"), and the finish card has its own focus.
pub fn announces(before: &Phase, after: &Phase) -> bool {
    if before.stage_key() == after.stage_key() {
        return false;
    }
    !matches!(after, Phase::Recording | Phase::Ready)
        && !matches!((before, after), (Phase::Recording, Phase::Stopped { .. }))
}

impl Problem {
    pub fn of(
        job: &JobState,
        models: &HashMap<ModelRef, ModelInfo>,
        downloads: &HashMap<ModelRef, Download>,
    ) -> Problem {
        match job {
            JobState::Blocked { stage, blocker } => {
                // Translating runs on the summary model, so it is blocked the way a summary is.
                if matches!(stage, Stage::Summarize | Stage::Translate)
                    || matches!(blocker, Blocker::NoModel { kind: KindTag::Llm })
                {
                    return Problem::SummaryModelMissing;
                }
                match blocker {
                    Blocker::NeedsProvision { model, bytes } => {
                        let info = models.get(model);
                        Problem::SpeechModelMissing {
                            model: model.clone(),
                            kind: info.map_or(KindTag::Stt, |m| m.kind),
                            name: info.map_or_else(|| model.to_string(), |m| m.name.clone()),
                            bytes: bytes.or(info.and_then(|m| m.bytes)),
                            download: downloads.get(model).copied(),
                        }
                    }
                    Blocker::NoModel { .. } => Problem::NoSpeechModel,
                    other => Problem::Transcription {
                        text: blocked_text(other),
                        started: false,
                    },
                }
            }
            JobState::Failed {
                stage: Stage::Summarize,
                error,
            } => Problem::Summary(error.message.clone()),
            JobState::Failed {
                stage: Stage::Translate,
                error,
            } => Problem::Translation(error.message.clone()),
            JobState::Failed { error, .. } => match error.cause {
                FailureCause::DiskFull => Problem::DiskFull,
                FailureCause::AudioUnreadable => Problem::AudioUnreadable,
                FailureCause::Crashed => Problem::Crashed,
                FailureCause::Other if error.class == FailureClass::Transient => Problem::Crashed,
                FailureCause::Other => Problem::Transcription {
                    text: format!("transcription failed: {}", sentence_end(&error.message)),
                    started: true,
                },
            },
            _ => Problem::Transcription {
                text: "transcription stopped.".into(),
                started: true,
            },
        }
    }

    pub fn stage(&self) -> Stage {
        match self {
            Problem::SummaryModelMissing | Problem::Summary(_) => Stage::Summarize,
            Problem::Translation(_) => Stage::Translate,
            _ => Stage::Transcribe,
        }
    }

    pub fn headline(&self) -> &'static str {
        match self {
            Problem::SpeechModelMissing { .. } | Problem::NoSpeechModel => {
                "Transcription didn't start"
            }
            Problem::Transcription { started: false, .. } => "Transcription didn't start",
            Problem::DiskFull | Problem::Crashed | Problem::Transcription { .. } => {
                "Transcription didn't finish"
            }
            Problem::AudioUnreadable => "Can't read the audio",
            Problem::SummaryModelMissing | Problem::Summary(_) => "No summary yet",
            Problem::Translation(_) => "No translation yet",
        }
    }

    /// UX-13's one sentence. When the audio is safe it says so first.
    pub fn text(&self) -> String {
        match self {
            Problem::SpeechModelMissing { kind, .. } => format!(
                "Your recording is saved, but the {} model isn't on this Mac.",
                model_noun(*kind)
            ),
            Problem::NoSpeechModel => {
                "Your recording is saved, but no speech model is set up.".into()
            }
            Problem::DiskFull => {
                "Your recording is saved, but there isn't enough space to transcribe it.".into()
            }
            Problem::Crashed => "Transcription stopped partway. Your recording is saved.".into(),
            Problem::AudioUnreadable => "This recording's audio can't be read.".into(),
            Problem::Transcription { text, .. } => format!("Your recording is saved, but {text}"),
            Problem::SummaryModelMissing => {
                "The transcript is ready, but summaries aren't set up.".into()
            }
            Problem::Summary(message) => format!(
                "The transcript is ready, but the summary failed: {}",
                sentence_end(message)
            ),
            Problem::Translation(message) => format!(
                "The transcript is ready as it was spoken, but translating it failed: {}",
                sentence_end(message)
            ),
        }
    }

    /// The card's detail line: the sentence, or the download while it runs.
    pub fn detail(&self) -> String {
        match self {
            Problem::SpeechModelMissing {
                name,
                download: Some(download),
                ..
            } => downloading(name, *download, "MB"),
            _ => self.text(),
        }
    }

    fn step_word(&self) -> &'static str {
        match self {
            Problem::SpeechModelMissing {
                kind: KindTag::Diar,
                ..
            } => "Needs the speaker model",
            Problem::SpeechModelMissing { .. } => "Needs the speech model",
            Problem::NoSpeechModel => "Needs a speech model",
            Problem::DiskFull => "Needs space",
            Problem::Crashed => "Stopped",
            Problem::AudioUnreadable => "Can't read audio",
            Problem::Transcription { started: false, .. } => "Didn't start",
            Problem::Transcription { .. } => "Needs attention",
            Problem::SummaryModelMissing | Problem::Summary(_) | Problem::Translation(_) => {
                "Waiting"
            }
        }
    }

    pub fn fix(&self) -> Option<Fix> {
        Some(match self {
            Problem::SpeechModelMissing {
                download: Some(_), ..
            } => return None,
            Problem::SpeechModelMissing { model, .. } => Fix::Download(model.clone()),
            Problem::NoSpeechModel => Fix::ChooseSpeechModel,
            Problem::SummaryModelMissing => Fix::SetUpSummaries,
            Problem::DiskFull => Fix::FreeUpSpace,
            Problem::AudioUnreadable => Fix::ShowInFinder,
            Problem::Crashed
            | Problem::Transcription { .. }
            | Problem::Summary(_)
            | Problem::Translation(_) => Fix::TryAgain,
        })
    }

    /// The button's visible label and its spoken name.
    pub fn button(&self) -> Option<(String, String)> {
        let plain = |label: &str| Some((label.to_owned(), label.to_owned()));
        match self.fix()? {
            Fix::Download(_) => {
                let Problem::SpeechModelMissing { name, bytes, .. } = self else {
                    return None;
                };
                match bytes {
                    Some(bytes) => Some((
                        format!("Download {name} ({} MB)", megabytes(*bytes)),
                        format!("Download {name}, {} megabytes", megabytes(*bytes)),
                    )),
                    None => plain(&format!("Download {name}")),
                }
            }
            Fix::ChooseSpeechModel => plain("Choose a Speech Model…"),
            Fix::SetUpSummaries => plain("Set Up Summaries"),
            Fix::FreeUpSpace => plain("Free Up Space…"),
            Fix::TryAgain => plain("Try Again"),
            Fix::ShowInFinder => plain("Show in Finder"),
        }
    }

    /// The VoiceOver sentence: the headline, the sentence, and the button (UX-13).
    pub fn sentence(&self) -> String {
        let detail = match self {
            Problem::SpeechModelMissing {
                name,
                download: Some(download),
                ..
            } => format!("{}.", downloading(name, *download, "megabytes")),
            _ => self.text(),
        };
        match self.button() {
            Some((_, spoken)) => format!(
                "{}. {detail} {}.",
                self.headline(),
                spoken.trim_end_matches('…')
            ),
            None => format!("{}. {detail}", self.headline()),
        }
    }
}

fn model_noun(kind: KindTag) -> &'static str {
    match kind {
        KindTag::Diar => "speaker",
        _ => "speech",
    }
}

fn megabytes(bytes: u64) -> u64 {
    (bytes + 500_000) / 1_000_000
}

/// "Downloading Parakeet, 120 of 487 MB", or a percent-free form before the size is known.
fn downloading(name: &str, download: Download, unit: &str) -> String {
    let done = megabytes(download.done);
    match download.total {
        Some(total) => format!("Downloading {name}, {done} of {} {unit}", megabytes(total)),
        None => format!("Downloading {name}, {done} {unit}"),
    }
}

fn sentence_end(message: &str) -> String {
    let message = message.trim();
    if message.ends_with(['.', '!', '?']) {
        message.to_owned()
    } else {
        format!("{message}.")
    }
}

fn seconds(n: u8) -> String {
    if n == 1 {
        "1 second".into()
    } else {
        format!("{n} seconds")
    }
}

fn minutes_of_audio(audio: Ms) -> String {
    let secs = (audio.0 + 500) / 1000;
    match (secs + 30) / 60 {
        _ if secs < 60 => seconds(secs.max(1) as u8),
        1 => "1 minute".into(),
        n => format!("{n} minutes"),
    }
}

pub(crate) fn spoken_start(at: Ms) -> String {
    if at.0 < 60_000 {
        "in less than 1 minute".into()
    } else {
        match at.0.div_ceil(60_000) {
            1 => "in about 1 minute".into(),
            n => format!("in about {n} minutes"),
        }
    }
}

fn blocked_text(blocker: &Blocker) -> String {
    match blocker {
        Blocker::NeedsProvision { model, .. } => format!("{model} isn't on this Mac."),
        Blocker::Unavailable { model, why } => match &why.fix {
            Some(fix) => format!(
                "{model} is unavailable: {}. {fix}",
                unavailable(&why.reason)
            ),
            None => format!("{model} is unavailable: {}.", unavailable(&why.reason)),
        },
        Blocker::Incompatible { model, why } => format!("{model} {why}."),
        Blocker::NoModel { kind } => format!("no {kind} model is set up."),
    }
}

fn unavailable(reason: &UnavailableReason) -> String {
    match reason {
        UnavailableReason::UnsupportedPlatform => "not supported on this Mac".into(),
        UnavailableReason::OsTooOld { required } => format!("needs {required}"),
        UnavailableReason::NotEligible { detail } => detail.clone(),
        UnavailableReason::ServerUnreachable { endpoint } => format!("cannot reach {endpoint}"),
    }
}

/// The chip in the card while transcription runs light (UX-30).
pub fn light_chip(reason: LightReason) -> &'static str {
    match reason {
        LightReason::Chosen => "Light, chosen in Settings",
        LightReason::Battery => "Light, on battery",
        LightReason::LowPower => "Light, Low Power Mode",
        LightReason::Hot => "Light, Mac is warm",
        LightReason::Recording => "Light, while you record",
    }
}

pub(crate) fn because(reason: LightReason) -> &'static str {
    match reason {
        LightReason::Chosen => "Light is chosen in Settings",
        LightReason::Battery => "the Mac is on battery",
        LightReason::LowPower => "Low Power Mode is on",
        LightReason::Hot => "the Mac is warm",
        LightReason::Recording => "you're recording",
    }
}

/// The first progress seen for a recording's running stage at its current pace, so a rate covers
/// only this run at this pace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    pub stage: Stage,
    pub pace: Option<Pace>,
    pub from: Fraction,
    pub at_ms: u64,
}

/// A Processing choice in Settings > Transcription and its effect in one line (UX-30).
pub struct PaceChoice {
    pub setting: Processing,
    pub id: &'static str,
    pub label: &'static str,
    pub effect: &'static str,
}

pub const PACE_CHOICES: [PaceChoice; 3] = [
    PaceChoice {
        setting: Processing::Auto,
        id: "processing-auto",
        label: "Automatic",
        effect: "Runs fast when the Mac is plugged in and cool. Runs light on battery, in Low Power Mode, when the Mac is hot, and while you record.",
    },
    PaceChoice {
        setting: Processing::Fast,
        id: "processing-fast",
        label: "Fast",
        effect: "Uses up to 4 cores. Finishes soonest, but the fans may spin up and the battery drains faster.",
    },
    PaceChoice {
        setting: Processing::Light,
        id: "processing-light",
        label: "Light",
        effect: "Uses 2 cores. Takes longer, stays quieter, and is easier on the battery.",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Failed,
}

/// Rows whose job went from waiting or running to done or stopped between two reloads, which
/// is when the app announces and notifies (A11Y-4, UX-14).
pub fn outcomes(before: &[RecordingRow], after: &[RecordingRow]) -> Vec<(RecordingId, Outcome)> {
    after
        .iter()
        .filter_map(|row| {
            let was = before.iter().find(|b| b.recording.id == row.recording.id)?;
            let active = matches!(was.job, Some(JobState::Queued | JobState::Running { .. }));
            let outcome = match row.job {
                Some(JobState::Succeeded) => Outcome::Finished,
                Some(JobState::Blocked { .. } | JobState::Failed { .. }) => Outcome::Failed,
                _ => return None,
            };
            active.then(|| (row.recording.id.clone(), outcome))
        })
        .collect()
}

/// Rows that had no transcript at the last reload and have one now, which is when the app posts
/// Transcript ready (UX-14). A re-transcribe replaces a transcript, so it does not count.
pub fn transcripts_ready(before: &[RecordingRow], after: &[RecordingRow]) -> Vec<RecordingId> {
    after
        .iter()
        .filter(|row| row.has_transcript)
        .filter(|row| {
            before
                .iter()
                .any(|b| b.recording.id == row.recording.id && !b.has_transcript)
        })
        .map(|row| row.recording.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_transcript_is_ready_once_when_it_first_appears() {
        let rows = fixtures::rows();
        let with = |ix: usize, has_transcript| RecordingRow {
            has_transcript,
            ..rows[ix].clone()
        };
        let before = [with(0, false), with(1, true), with(2, false)];
        let after = [with(0, true), with(1, true), with(2, false), with(3, true)];
        assert_eq!(
            transcripts_ready(&before, &after),
            vec![rows[0].recording.id.clone()],
            "1 already had one, 2 has none yet, and 3 was not listed before"
        );
    }

    #[test]
    fn the_reading_head_shows_only_while_transcribing() {
        assert!(!Phase::Saving.has_reading_head());
        assert!(!Phase::Preparing.has_reading_head());
        assert!(!Phase::Ready.has_reading_head());
        assert!(
            !Phase::Summarizing {
                left: Left::Estimating
            }
            .has_reading_head()
        );
    }

    use dory_core::app::Budget;
    use dory_core::domain::JobError;

    use super::*;
    use crate::fixtures;
    use crate::library::{Glyph, Tone};

    const MIN: u64 = 60_000;

    /// One recording's facts, starting from Weekly sync with nothing running, recorded or queued.
    struct Case {
        row: RecordingRow,
        recorder: RecorderStatus,
        grace_left: Option<u8>,
        sealing: bool,
        progress: Option<Progress>,
        done: Vec<Stage>,
        translating: bool,
        spot: Option<Spot>,
        left: Left,
        models: HashMap<ModelRef, ModelInfo>,
        downloads: HashMap<ModelRef, Download>,
    }

    fn weekly() -> Case {
        let mut row = fixtures::rows()
            .into_iter()
            .find(|r| r.recording.title.as_deref() == Some("Weekly sync"))
            .unwrap();
        row.job = None;
        row.has_transcript = false;
        row.summary = SummaryFlag::None;
        Case {
            row,
            recorder: RecorderStatus::Idle,
            grace_left: None,
            sealing: false,
            progress: None,
            done: Vec::new(),
            translating: false,
            spot: None,
            left: Left::Estimating,
            models: HashMap::new(),
            downloads: HashMap::new(),
        }
    }

    impl Case {
        fn job(mut self, job: JobState) -> Case {
            self.row.job = Some(job);
            self
        }

        fn progress(mut self, stage: Stage, fraction: f32, budget: Option<Budget>) -> Case {
            self.progress = Some(Progress {
                stage,
                fraction: Fraction::new(fraction),
                budget,
            });
            self
        }

        fn phase(&self) -> Phase {
            Phase::derive(&Facts {
                row: &self.row,
                recorder: &self.recorder,
                grace_left: self.grace_left,
                sealing: self.sealing,
                progress: self.progress,
                done: &self.done,
                translating: self.translating,
                spot: self.spot,
                left: self.left,
                auto_process: true,
                models: &self.models,
                downloads: &self.downloads,
            })
        }
    }

    /// One row of section 4: what the card and the row say.
    #[derive(Debug, PartialEq)]
    struct Words {
        headline: Option<String>,
        detail: Option<String>,
        steps: Option<Vec<(StepGlyph, String)>>,
        sentence: String,
        row: (Glyph, String, Tone),
    }

    fn words(phase: &Phase) -> Words {
        let row = phase.row();
        Words {
            headline: phase.headline(),
            detail: phase.detail(Some(Ms(62 * MIN))),
            steps: phase
                .steps()
                .map(|steps| steps.into_iter().map(|s| (s.glyph, s.word)).collect()),
            sentence: phase.sentence("Weekly sync"),
            row: (row.glyph(), row.label(), row.tone()),
        }
    }

    fn want(
        headline: Option<&str>,
        detail: Option<&str>,
        steps: Option<[(StepGlyph, &str); 3]>,
        sentence: &str,
        row: (Glyph, &str, Tone),
    ) -> Words {
        Words {
            headline: headline.map(str::to_owned),
            detail: detail.map(str::to_owned),
            steps: steps.map(|s| s.into_iter().map(|(g, w)| (g, w.to_owned())).collect()),
            sentence: sentence.to_owned(),
            row: (row.0, row.1.to_owned(), row.2),
        }
    }

    use StepGlyph::{Check, Clock, Dots, Pending, Stop, Warning};

    fn running(stage: Stage) -> JobState {
        JobState::Running { stage }
    }

    #[test]
    fn p1_stopped_in_the_undo_window() {
        let mut case = weekly();
        case.recorder = RecorderStatus::Grace {
            id: case.row.recording.id.clone(),
            recorded: Ms(62 * MIN),
        };
        case.grace_left = Some(5);
        assert_eq!(
            words(&case.phase()),
            want(
                Some("Recording stopped"),
                Some("Transcription starts in 5 seconds."),
                Some([
                    (Stop, "Stopped"),
                    (Pending, "Starts in 5 s"),
                    (Pending, "Waiting")
                ]),
                "Recording stopped. Transcription starts in 5 seconds.",
                (Glyph::StopCircle, "Stopped", Tone::Secondary),
            )
        );
    }

    #[test]
    fn p2_saving() {
        let mut case = weekly();
        case.sealing = true;
        assert_eq!(
            words(&case.phase()),
            want(
                Some("Saving the recording"),
                Some("62 minutes of audio. This takes a moment."),
                Some([(Dots, "Saving"), (Pending, "Waiting"), (Pending, "Waiting")]),
                "Saving the recording.",
                (Glyph::DotsCircle, "Saving", Tone::Accent),
            )
        );
    }

    #[test]
    fn p3a_queued_with_the_number_ahead_and_the_start() {
        let mut case = weekly().job(JobState::Queued);
        case.spot = Some(Spot {
            ahead: 2,
            starts_in: Some(Ms(8 * MIN + 20_000)),
        });
        let phase = case.phase();
        assert!(phase.can_move_to_top());
        assert_eq!(
            words(&phase),
            want(
                Some("Waiting to transcribe"),
                Some("2 recordings are ahead of this one. It starts in about 9 min."),
                Some([(Check, "Saved"), (Clock, "2 ahead"), (Pending, "Waiting")]),
                "Waiting to transcribe. 2 recordings ahead, starts in about 9 minutes.",
                (Glyph::Clock, "Queued, 2 ahead", Tone::Secondary),
            )
        );
        case.spot = Some(Spot {
            ahead: 1,
            starts_in: None,
        });
        assert_eq!(
            case.phase().detail(None).as_deref(),
            Some("1 recording is ahead of this one."),
            "no start without estimates for the jobs ahead"
        );
        case.spot = Some(Spot {
            ahead: 0,
            starts_in: None,
        });
        assert!(!case.phase().can_move_to_top());
        assert_eq!(case.phase().row().label(), "Queued");
    }

    #[test]
    fn p3b_loading_the_model() {
        let case = weekly().job(running(Stage::Transcribe));
        assert_eq!(
            words(&case.phase()),
            want(
                Some("Getting ready to transcribe"),
                Some("Loading the speech model. This takes a few seconds."),
                Some([(Check, "Saved"), (Dots, "Preparing"), (Pending, "Waiting")]),
                "Getting ready to transcribe. Loading the speech model.",
                (Glyph::Dots, "Preparing", Tone::Accent),
            )
        );
    }

    #[test]
    fn p4_transcribing_with_the_time_left() {
        let mut case = weekly().job(running(Stage::Transcribe)).progress(
            Stage::Transcribe,
            0.4,
            Some(Budget::Fast { threads: 4 }),
        );
        case.left = Left::About(Ms(4 * MIN));
        let phase = case.phase();
        assert_eq!(phase.light(), None);
        assert_eq!(
            words(&phase),
            want(
                Some("Transcribing"),
                Some("40% · about 4 min left"),
                Some([(Check, "Saved"), (Dots, "40%"), (Pending, "Waiting")]),
                "Transcribing, 40%, about 4 minutes left.",
                (Glyph::Dots, "Transcribing 40%", Tone::Accent),
            )
        );
    }

    #[test]
    fn p4_in_the_first_minute_is_still_estimating() {
        let case = weekly().job(running(Stage::Transcribe)).progress(
            Stage::Transcribe,
            0.03,
            Some(Budget::Fast { threads: 4 }),
        );
        let said = words(&case.phase());
        assert_eq!(said.detail.as_deref(), Some("3% · estimating time left"));
        assert_eq!(said.sentence, "Transcribing, 3%. Estimating time left.");
    }

    #[test]
    fn p5_transcribing_light_says_why_and_offers_fast() {
        let mut case = weekly().job(running(Stage::Transcribe)).progress(
            Stage::Transcribe,
            0.4,
            Some(Budget::Light {
                reason: LightReason::Battery,
            }),
        );
        case.left = Left::About(Ms(9 * MIN));
        let phase = case.phase();
        assert_eq!(phase.light().map(light_chip), Some("Light, on battery"));
        assert_eq!(
            words(&phase),
            want(
                Some("Transcribing"),
                Some("40% · about 9 min left"),
                Some([(Check, "Saved"), (Dots, "40%, light"), (Pending, "Waiting")]),
                "Transcribing, 40%, about 9 minutes left. Running light because the Mac is on battery.",
                (Glyph::Dots, "40%, light", Tone::Accent),
            )
        );
        for (reason, chip) in [
            (LightReason::LowPower, "Light, Low Power Mode"),
            (LightReason::Hot, "Light, Mac is warm"),
            (LightReason::Recording, "Light, while you record"),
        ] {
            assert_eq!(light_chip(reason), chip);
        }
    }

    #[test]
    fn p6_summarizing() {
        let mut case = weekly().job(running(Stage::Summarize));
        case.row.has_transcript = true;
        case.left = Left::About(Ms(50_000));
        assert_eq!(
            words(&case.phase()),
            want(
                Some("Writing the summary"),
                Some("The transcript is ready to read. Summary in less than 1 min."),
                Some([(Check, "Saved"), (Check, "Ready"), (Dots, "Writing")]),
                "Transcript ready. Writing the summary, less than 1 minute left.",
                (Glyph::Dots, "Summarizing", Tone::Accent),
            )
        );
        case.left = Left::About(Ms(MIN));
        assert_eq!(
            case.phase().sentence("Weekly sync"),
            "Transcript ready. Writing the summary, about 1 minute left."
        );
        let mut between = weekly().job(JobState::Queued);
        between.done = vec![Stage::Transcribe];
        assert_eq!(
            between.phase().row(),
            RowStatus::Summarizing,
            "queued for the summary after the transcript"
        );
    }

    #[test]
    fn translating_sits_between_the_transcript_and_the_summary() {
        let mut case =
            weekly()
                .job(running(Stage::Translate))
                .progress(Stage::Translate, 0.4, None);
        case.row.has_transcript = true;
        case.translating = true;
        let phase = case.phase();
        assert_eq!(
            phase,
            Phase::Translating {
                done: Fraction::new(0.4)
            }
        );
        assert!(phase.is_processing());
        assert!(
            !phase.before_transcript(),
            "the spoken transcript is readable"
        );
        assert_eq!(
            words(&phase),
            want(
                Some("Translating the transcript"),
                Some("The transcript is ready to read as it was spoken. 40% translated."),
                Some([
                    (Check, "Saved"),
                    (Dots, "Translating 40%"),
                    (Pending, "Waiting")
                ]),
                "Transcript ready as spoken. Translating it, 40%.",
                (Glyph::Dots, "Translating", Tone::Accent),
            )
        );

        let mut between = weekly().job(JobState::Queued);
        between.done = vec![Stage::Transcribe];
        between.translating = true;
        assert_eq!(
            between.phase(),
            Phase::Translating {
                done: Fraction::new(0.0)
            },
            "queued for the translation after the transcript"
        );
        between.done = vec![Stage::Transcribe, Stage::Translate];
        between.translating = false;
        assert_eq!(between.phase().row(), RowStatus::Summarizing);
    }

    #[test]
    fn same_as_spoken_never_shows_a_translating_phase() {
        let mut case = weekly().job(JobState::Queued);
        case.done = vec![Stage::Transcribe];
        assert_eq!(case.phase().row(), RowStatus::Summarizing);
    }

    #[test]
    fn a_translation_blocked_for_want_of_the_summary_model_takes_the_summary_path() {
        let blocked = JobState::Blocked {
            stage: Stage::Translate,
            blocker: Blocker::NoModel { kind: KindTag::Llm },
        };
        let problem = Problem::of(&blocked, &HashMap::new(), &HashMap::new());
        assert_eq!(problem, Problem::SummaryModelMissing);
        assert_eq!(problem.fix(), Some(Fix::SetUpSummaries));
        let missing = JobState::Blocked {
            stage: Stage::Translate,
            blocker: Blocker::NeedsProvision {
                model: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
                bytes: None,
            },
        };
        assert_eq!(
            Problem::of(&missing, &HashMap::new(), &HashMap::new()),
            Problem::SummaryModelMissing
        );
        let failed = JobState::Failed {
            stage: Stage::Translate,
            error: JobError {
                class: FailureClass::Permanent,
                message: "the model said no".into(),
                cause: FailureCause::Other,
            },
        };
        let problem = Problem::of(&failed, &HashMap::new(), &HashMap::new());
        assert_eq!(problem, Problem::Translation("the model said no".into()));
        assert_eq!(problem.stage(), Stage::Translate);
        assert_eq!(problem.fix(), Some(Fix::TryAgain));
    }

    #[test]
    fn p7_done_becomes_the_player() {
        let mut case = weekly().job(JobState::Succeeded);
        case.row.has_transcript = true;
        case.row.summary = SummaryFlag::Current;
        let phase = case.phase();
        assert!(!phase.is_processing());
        assert_eq!(
            words(&phase),
            want(
                None,
                None,
                None,
                "Summary ready for Weekly sync.",
                (Glyph::Check, "Done", Tone::Secondary),
            )
        );
        case.row.summary = SummaryFlag::None;
        assert_eq!(
            case.phase().sentence("Weekly sync"),
            "Transcript ready for Weekly sync."
        );
    }

    fn parakeet() -> ModelRef {
        ModelRef::parse("sherpa/parakeet").unwrap()
    }

    fn missing_parakeet() -> Case {
        let mut case = weekly().job(JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NeedsProvision {
                model: parakeet(),
                bytes: None,
            },
        });
        case.models.insert(
            parakeet(),
            ModelInfo {
                kind: KindTag::Stt,
                name: "Parakeet".into(),
                bytes: Some(487_211_443),
            },
        );
        case
    }

    #[test]
    fn p8_a_missing_speech_model_offers_its_download() {
        let case = missing_parakeet();
        let phase = case.phase();
        assert!(phase.is_processing());
        assert_eq!(
            words(&phase),
            want(
                Some("Transcription didn't start"),
                Some("Your recording is saved, but the speech model isn't on this Mac."),
                Some([
                    (Check, "Saved"),
                    (Warning, "Needs the speech model"),
                    (Pending, "Waiting")
                ]),
                "Transcription didn't start. Your recording is saved, but the speech model isn't on this Mac. Download Parakeet, 487 megabytes.",
                (Glyph::Triangle, "Needs attention", Tone::Attention),
            )
        );
        let Phase::Failed(problem) = &phase else {
            unreachable!()
        };
        assert_eq!(
            problem.button(),
            Some((
                "Download Parakeet (487 MB)".into(),
                "Download Parakeet, 487 megabytes".into()
            ))
        );
        assert_eq!(phase.fix(), Some(Fix::Download(parakeet())));
        assert_eq!(
            phase.placeholder(),
            "The transcript appears here once the speech model is installed."
        );
    }

    #[test]
    fn p8_the_download_shows_in_the_same_card() {
        let mut case = missing_parakeet();
        case.downloads.insert(
            parakeet(),
            Download {
                done: 120_000_000,
                total: Some(487_211_443),
            },
        );
        let phase = case.phase();
        assert_eq!(
            phase.detail(None).as_deref(),
            Some("Downloading Parakeet, 120 of 487 MB")
        );
        assert_eq!(phase.fix(), None, "no button while it downloads");
        assert_eq!(
            phase.sentence("Weekly sync"),
            "Transcription didn't start. Downloading Parakeet, 120 of 487 megabytes."
        );
    }

    fn failed(stage: Stage, class: FailureClass, cause: FailureCause) -> JobState {
        JobState::Failed {
            stage,
            error: JobError {
                class,
                message: "the engine said no".into(),
                cause,
            },
        }
    }

    /// UX-13's table: cause, sentence, button, and whether the card stays (the transcript failed)
    /// or the Summary tab shows it (the summary failed).
    #[test]
    fn ux_13_every_cause_says_what_to_do() {
        let summary_blocked = JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::NoModel { kind: KindTag::Llm },
        };
        let rows: [(&str, JobState, &str, &str, Fix, bool); 6] = [
            (
                "summary model missing",
                summary_blocked,
                "The transcript is ready, but summaries aren't set up.",
                "Set Up Summaries",
                Fix::SetUpSummaries,
                false,
            ),
            (
                "disk full",
                failed(
                    Stage::Transcribe,
                    FailureClass::NeedsUser,
                    FailureCause::DiskFull,
                ),
                "Your recording is saved, but there isn't enough space to transcribe it.",
                "Free Up Space…",
                Fix::FreeUpSpace,
                true,
            ),
            (
                "transcriber crashed",
                failed(
                    Stage::Transcribe,
                    FailureClass::Transient,
                    FailureCause::Crashed,
                ),
                "Transcription stopped partway. Your recording is saved.",
                "Try Again",
                Fix::TryAgain,
                true,
            ),
            (
                "retries ran out",
                failed(
                    Stage::Transcribe,
                    FailureClass::Transient,
                    FailureCause::Other,
                ),
                "Transcription stopped partway. Your recording is saved.",
                "Try Again",
                Fix::TryAgain,
                true,
            ),
            (
                "audio unreadable",
                failed(
                    Stage::Transcribe,
                    FailureClass::Permanent,
                    FailureCause::AudioUnreadable,
                ),
                "This recording's audio can't be read.",
                "Show in Finder",
                Fix::ShowInFinder,
                true,
            ),
            (
                "something else",
                failed(
                    Stage::Transcribe,
                    FailureClass::Permanent,
                    FailureCause::Other,
                ),
                "Your recording is saved, but transcription failed: the engine said no.",
                "Try Again",
                Fix::TryAgain,
                true,
            ),
        ];
        for (name, job, text, button, fix, card) in rows {
            let mut case = weekly().job(job);
            case.row.has_transcript = !card;
            let phase = case.phase();
            let Phase::Failed(problem) = &phase else {
                panic!("{name}: {phase:?}")
            };
            assert_eq!(problem.text(), text, "{name}");
            assert_eq!(
                problem.button(),
                Some((button.into(), button.into())),
                "{name}"
            );
            assert_eq!(phase.fix(), Some(fix), "{name}");
            assert_eq!(phase.is_processing(), card, "{name}");
            assert_eq!(phase.row(), RowStatus::NeedsAttention, "{name}");
            assert_eq!(
                phase.sentence("Weekly sync"),
                format!(
                    "{}. {text} {}.",
                    problem.headline(),
                    button.trim_end_matches('…')
                ),
                "{name}: the sentence plus the button's label"
            );
        }
    }

    #[test]
    fn no_speech_model_asks_to_choose_one() {
        let case = weekly().job(JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NoModel { kind: KindTag::Stt },
        });
        let Phase::Failed(problem) = case.phase() else {
            unreachable!()
        };
        assert_eq!(problem, Problem::NoSpeechModel);
        assert_eq!(problem.fix(), Some(Fix::ChooseSpeechModel));
    }

    #[test]
    fn rows_outside_processing_keep_their_words() {
        let mut case = weekly();
        assert_eq!(case.phase(), Phase::Ready);
        assert_eq!(case.phase().row().label(), "Ready to transcribe");
        case.recorder = RecorderStatus::Recording {
            id: case.row.recording.id.clone(),
            recorded: Ms(1_000),
        };
        assert_eq!(case.phase().row(), RowStatus::Recording);
        let mut cancelled = weekly().job(JobState::Cancelled);
        cancelled.row.has_transcript = true;
        assert_eq!(cancelled.phase().row(), RowStatus::Done);
    }

    #[test]
    fn voiceover_speaks_on_a_new_stage_a_failure_and_done_but_not_on_each_percent() {
        let transcribing = |percent: f32, left: Left| Phase::Transcribing {
            done: Fraction::new(percent),
            left,
            light: None,
        };
        let queued = Phase::Queued(Spot {
            ahead: 2,
            starts_in: None,
        });
        let crashed = Phase::Failed(Problem::Crashed);
        let stopped = |starts_in| Phase::Stopped {
            starts_in,
            auto: true,
        };
        let silent = [
            (
                transcribing(0.40, Left::Estimating),
                transcribing(0.41, Left::About(Ms(MIN))),
            ),
            (stopped(5), stopped(4)),
            (
                queued.clone(),
                Phase::Queued(Spot {
                    ahead: 1,
                    starts_in: None,
                }),
            ),
            (Phase::Recording, stopped(5)),
            (Phase::Saving, Phase::Ready),
        ];
        for (before, after) in silent {
            assert!(!announces(&before, &after), "{before:?} -> {after:?}");
        }
        let summarizing = Phase::Summarizing {
            left: Left::Estimating,
        };
        let spoken = [
            (queued, Phase::Preparing),
            (Phase::Preparing, transcribing(0.0, Left::Estimating)),
            (transcribing(0.9, Left::Estimating), summarizing.clone()),
            (summarizing, Phase::Done { summary: true }),
            (transcribing(0.4, Left::Estimating), crashed.clone()),
            (crashed, Phase::Failed(Problem::DiskFull)),
        ];
        for (before, after) in spoken {
            assert!(announces(&before, &after), "{before:?} -> {after:?}");
        }
    }

    #[test]
    fn outcomes_name_rows_whose_job_just_finished_or_stopped() {
        let rows = fixtures::rows();
        let mut before = rows.clone();
        let mut after = rows.clone();
        before[0].job = Some(running(Stage::Transcribe));
        after[0].job = Some(JobState::Succeeded);
        before[1].job = Some(JobState::Queued);
        after[1].job = Some(failed(
            Stage::Transcribe,
            FailureClass::Permanent,
            FailureCause::Other,
        ));
        before[2].job = Some(JobState::Succeeded);
        after[2].job = Some(JobState::Succeeded);
        assert_eq!(
            outcomes(&before, &after),
            vec![
                (rows[0].recording.id.clone(), Outcome::Finished),
                (rows[1].recording.id.clone(), Outcome::Failed),
            ]
        );
        assert_eq!(outcomes(&rows, &rows), vec![]);
    }
}
