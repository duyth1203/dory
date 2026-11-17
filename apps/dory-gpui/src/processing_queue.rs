//! The Processing window's rules (`docs/design/floating-surfaces.md` section 4 and the
//! operator's decisions) as plain data and pure functions: the four sections, the header's live
//! summary, each row's words and VoiceOver sentence, where a reorder lands, and what VoiceOver
//! hears when the queue changes. Nothing here touches GPUI or the store; `AppState` builds a
//! [`Queue`] and `processing_window` draws it.

use dory_core::domain::{JobId, Ms, RecordingId};

use crate::estimate::Left;
use crate::library::Percent;
use crate::processing::{Phase, Problem, because, spoken_start};

/// The job the worker runs now.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub job: JobId,
    pub recording: RecordingId,
    pub title: String,
    pub audio: Option<Ms>,
    /// Preparing, Transcribing, or Summarizing.
    pub phase: Phase,
}

/// A queued job, in the order the worker takes them.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub job: JobId,
    pub recording: RecordingId,
    pub title: String,
    pub audio: Option<Ms>,
    /// When it starts, once every job ahead has an estimate.
    pub starts_in: Option<Ms>,
}

/// A blocked or failed recording, with the cause and its one fix (UX-13). Not in line.
#[derive(Clone, Debug, PartialEq)]
pub struct Blocked {
    pub recording: RecordingId,
    pub title: String,
    pub problem: Problem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Done,
    Cancelled,
}

/// A job that finished or was cancelled since the app started. The list resets on quit.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub job: JobId,
    pub recording: RecordingId,
    pub title: String,
    pub ended: Ended,
    /// Local hour and minute.
    pub at: (u32, u32),
}

/// Everything the window shows, section by section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    pub now: Option<Running>,
    pub next: Vec<Waiting>,
    pub attention: Vec<Blocked>,
    pub finished: Vec<Finished>,
    /// The time left for every running and queued job, once each has an estimate.
    pub left: Option<Ms>,
}

impl Queue {
    pub fn is_empty(&self) -> bool {
        self.now.is_none()
            && self.next.is_empty()
            && self.attention.is_empty()
            && self.finished.is_empty()
    }

    /// "3 recordings · about 24 min left", then " · 1 needs attention" when that applies, or
    /// " · running light" in light pace. `None` with nothing to do.
    pub fn summary(&self) -> Option<String> {
        let count = usize::from(self.now.is_some()) + self.next.len();
        let mut parts = Vec::new();
        if count > 0 {
            parts.push(match count {
                1 => "1 recording".to_owned(),
                n => format!("{n} recordings"),
            });
            if let Some(left) = self.left {
                parts.push(Left::About(left).short());
            }
        }
        if !self.attention.is_empty() {
            parts.push(format!("{} needs attention", self.attention.len()));
        } else if self.now.as_ref().is_some_and(|r| r.phase.light().is_some()) {
            parts.push("running light".into());
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The queued jobs' ids in line, for the reorder rules.
    pub fn order(&self) -> Vec<JobId> {
        self.next.iter().map(|w| w.job).collect()
    }
}

impl Running {
    /// "Transcribing 40% · about 4 min left", with ", light" in light pace.
    pub fn stage_line(&self) -> String {
        match &self.phase {
            Phase::Transcribing { done, left, light } => {
                let light = if light.is_some() { ", light" } else { "" };
                format!(
                    "Transcribing {}%{light} · {}",
                    Percent::of(*done).get(),
                    left.short()
                )
            }
            Phase::Summarizing { left } => format!("Summarizing · {}", left.short()),
            Phase::Preparing => "Getting ready to transcribe".into(),
            other => other.headline().unwrap_or_default(),
        }
    }

    /// "Transcribed up to 24:48 / 1:02:00" under the mini waveform, in the card's clock.
    pub fn caption(&self) -> Option<String> {
        let audio = self.audio?;
        let done = self.phase.transcribed().get();
        let at = Ms((audio.0 as f64 * f64::from(done)).round() as u64);
        Some(format!(
            "Transcribed up to {} / {}",
            crate::a11y::clock(at),
            crate::a11y::clock(audio)
        ))
    }

    /// "Weekly sync, transcribing, 40%, about 4 minutes left.", with why it runs light.
    pub fn sentence(&self) -> String {
        let title = &self.title;
        match &self.phase {
            Phase::Transcribing { done, left, light } => {
                let mut said = format!(
                    "{title}, transcribing, {}%, {}",
                    Percent::of(*done).get(),
                    left.spoken()
                );
                if let Some(reason) = light {
                    said.push_str(&format!(", running light because {}", because(*reason)));
                }
                said.push('.');
                said
            }
            Phase::Summarizing { left } => format!("{title}, summarizing, {}.", left.spoken()),
            _ => format!("{title}, getting ready to transcribe."),
        }
    }

    /// The summary stage keeps no checkpoints, so cancelling it loses the work; transcription
    /// resumes from its last finished window.
    pub fn restarts_when_undone(&self) -> bool {
        matches!(self.phase, Phase::Summarizing { .. })
    }
}

impl Waiting {
    /// "48 min · starts in about 4 min".
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(audio) = self.audio {
            parts.push(format!("{} min", minutes(audio)));
        }
        if let Some(at) = self.starts_in {
            parts.push(format!("starts {}", Left::starts(at)));
        }
        parts.join(" · ")
    }

    /// "Design review, queued, 1 of 2, 48 minutes, starts in about 4 minutes." `position` counts
    /// from 1.
    pub fn sentence(&self, position: usize, of: usize) -> String {
        let mut said = format!("{}, queued, {position} of {of}", self.title);
        if let Some(audio) = self.audio {
            said.push_str(&match minutes(audio) {
                1 => ", 1 minute".to_owned(),
                n => format!(", {n} minutes"),
            });
        }
        if let Some(at) = self.starts_in {
            said.push_str(&format!(", starts {}", spoken_start(at)));
        }
        said.push('.');
        said
    }
}

impl Blocked {
    /// "Hiring debrief, needs attention. The transcript is ready, but summaries aren't set up."
    pub fn sentence(&self) -> String {
        format!("{}, needs attention. {}", self.title, self.problem.text())
    }
}

impl Finished {
    /// "Done at 14:32" or "Cancelled at 14:05".
    pub fn line(&self) -> String {
        let (h, m) = self.at;
        match self.ended {
            Ended::Done => format!("Done at {h:02}:{m:02}"),
            Ended::Cancelled => format!("Cancelled at {h:02}:{m:02}"),
        }
    }

    /// "Kundengespräch Müller GmbH, done at 14:32."
    pub fn sentence(&self) -> String {
        let line = self.line();
        let mut chars = line.chars();
        let first = chars.next().map(|c| c.to_lowercase().to_string());
        format!(
            "{}, {}{}.",
            self.title,
            first.unwrap_or_default(),
            chars.as_str()
        )
    }
}

fn minutes(audio: Ms) -> u64 {
    ((audio.0 + 30_000) / 60_000).max(1)
}

/// A reorder of a queued job (UX-15): the visible Move to Top, the ⋯ menu's and the keyboard's
/// Move Up and Move Down, and a drop at an index by the handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Top,
    Up,
    Down,
    To(usize),
}

/// The index `job` moves to in `order`, the queued jobs in line, or `None` when the move changes
/// nothing: it is not queued (the running job never moves), or it is already there.
pub fn target(order: &[JobId], job: JobId, to: Move) -> Option<usize> {
    let from = order.iter().position(|j| *j == job)?;
    let last = order.len() - 1;
    let at = match to {
        Move::Top => 0,
        Move::Up => from.checked_sub(1)?,
        Move::Down => (from + 1).min(last),
        Move::To(ix) => ix.min(last),
    };
    (at != from).then_some(at)
}

/// What VoiceOver hears after a move: "Design review moved to position 1".
pub fn moved(title: &str, to: usize) -> String {
    format!("{title} moved to position {}", to + 1)
}

/// The toast after Cancel: "Cancelled Design review", and a warning when Undo cannot resume.
pub fn cancelled(title: &str, restarts: bool) -> String {
    if restarts {
        format!("Cancelled {title}. Undo restarts its summary.")
    } else {
        format!("Cancelled {title}")
    }
}

/// What VoiceOver says when the queue goes from `before` to `after`, if anything: only a new
/// stage of the running job, a job that finished, and one that needs attention, never a percent
/// or an estimate. Cancel speaks through its toast.
pub fn announcement(before: &Queue, after: &Queue) -> Option<String> {
    let finished = after
        .finished
        .iter()
        .find(|f| f.ended == Ended::Done && !before.finished.iter().any(|b| b.job == f.job));
    if let Some(done) = finished {
        return Some(done.sentence());
    }
    let blocked = after
        .attention
        .iter()
        .find(|a| !before.attention.iter().any(|b| b.recording == a.recording));
    if let Some(blocked) = blocked {
        return Some(blocked.sentence());
    }
    let running = after.now.as_ref()?;
    let changed = match &before.now {
        Some(was) if was.job == running.job => {
            crate::processing::announces(&was.phase, &running.phase)
        }
        _ => true,
    };
    changed.then(|| running.sentence())
}

#[cfg(test)]
mod tests {
    use dory_core::app::LightReason;
    use dory_core::domain::Fraction;

    use super::*;

    const MIN: u64 = 60_000;

    fn id(s: &str) -> RecordingId {
        RecordingId::parse(s).unwrap()
    }

    fn transcribing(percent: f32, left: u64, light: Option<LightReason>) -> Phase {
        Phase::Transcribing {
            done: Fraction::new(percent / 100.),
            left: Left::About(Ms(left * MIN)),
            light,
        }
    }

    fn weekly(phase: Phase) -> Running {
        Running {
            job: JobId(1),
            recording: id("weekly"),
            title: "Weekly sync".into(),
            audio: Some(Ms(62 * MIN)),
            phase,
        }
    }

    fn waiting(job: u64, title: &str, minutes: u64, starts: Option<u64>) -> Waiting {
        Waiting {
            job: JobId(job),
            recording: id(&format!("r{job}")),
            title: title.into(),
            audio: Some(Ms(minutes * MIN)),
            starts_in: starts.map(|m| Ms(m * MIN)),
        }
    }

    fn hiring() -> Blocked {
        Blocked {
            recording: id("hiring"),
            title: "Hiring debrief".into(),
            problem: Problem::SummaryModelMissing,
        }
    }

    fn busy() -> Queue {
        Queue {
            now: Some(weekly(transcribing(40., 4, None))),
            next: vec![
                waiting(2, "Design review", 48, Some(4)),
                waiting(3, "1:1 with Sam", 27, Some(15)),
            ],
            attention: Vec::new(),
            finished: Vec::new(),
            left: Some(Ms(24 * MIN)),
        }
    }

    #[test]
    fn the_header_counts_the_line_and_adds_attention_or_light() {
        let mut queue = busy();
        assert_eq!(
            queue.summary().as_deref(),
            Some("3 recordings · about 24 min left")
        );
        queue.now = Some(weekly(transcribing(40., 9, Some(LightReason::Battery))));
        assert_eq!(
            queue.summary().as_deref(),
            Some("3 recordings · about 24 min left · running light")
        );
        queue.attention.push(hiring());
        assert_eq!(
            queue.summary().as_deref(),
            Some("3 recordings · about 24 min left · 1 needs attention"),
            "attention wins over light"
        );
        queue.left = None;
        queue.next.clear();
        queue.attention.clear();
        assert_eq!(
            queue.summary().as_deref(),
            Some("1 recording · running light")
        );
        assert_eq!(
            Queue::default().summary(),
            None,
            "nothing to do, no subtitle"
        );
        let finished_only = Queue {
            finished: vec![Finished {
                job: JobId(9),
                recording: id("k"),
                title: "K".into(),
                ended: Ended::Done,
                at: (14, 32),
            }],
            ..Queue::default()
        };
        assert_eq!(finished_only.summary(), None);
        assert!(!finished_only.is_empty());
    }

    #[test]
    fn the_running_card_drops_the_comma_and_says_light() {
        let running = weekly(transcribing(40., 4, None));
        assert_eq!(running.stage_line(), "Transcribing 40% · about 4 min left");
        assert_eq!(
            running.caption().as_deref(),
            Some("Transcribed up to 24:48 / 1:02:00"),
            "the card's clock, which counts hours"
        );
        let light = weekly(transcribing(40., 9, Some(LightReason::Battery)));
        assert_eq!(
            light.stage_line(),
            "Transcribing 40%, light · about 9 min left"
        );
        let summary = weekly(Phase::Summarizing {
            left: Left::About(Ms(MIN)),
        });
        assert_eq!(summary.stage_line(), "Summarizing · about 1 min left");
        assert!(summary.restarts_when_undone());
        assert!(!running.restarts_when_undone());
    }

    #[test]
    fn every_row_is_one_sentence() {
        assert_eq!(
            weekly(transcribing(40., 4, None)).sentence(),
            "Weekly sync, transcribing, 40%, about 4 minutes left."
        );
        assert_eq!(
            weekly(transcribing(40., 4, Some(LightReason::Battery))).sentence(),
            "Weekly sync, transcribing, 40%, about 4 minutes left, running light because the Mac is on battery."
        );
        let design = waiting(2, "Design review", 48, Some(4));
        assert_eq!(design.line(), "48 min · starts in about 4 min");
        assert_eq!(
            design.sentence(1, 2),
            "Design review, queued, 1 of 2, 48 minutes, starts in about 4 minutes."
        );
        assert_eq!(
            hiring().sentence(),
            "Hiring debrief, needs attention. The transcript is ready, but summaries aren't set up."
        );
        let done = Finished {
            job: JobId(4),
            recording: id("k"),
            title: "Kundengespräch Müller GmbH".into(),
            ended: Ended::Done,
            at: (14, 32),
        };
        assert_eq!(done.line(), "Done at 14:32");
        assert_eq!(
            done.sentence(),
            "Kundengespräch Müller GmbH, done at 14:32."
        );
        let gone = Finished {
            ended: Ended::Cancelled,
            at: (9, 5),
            ..done
        };
        assert_eq!(gone.line(), "Cancelled at 09:05");
    }

    #[test]
    fn reorders_land_where_each_command_says_and_the_running_job_never_moves() {
        let order = [JobId(2), JobId(3), JobId(4)];
        assert_eq!(target(&order, JobId(4), Move::Top), Some(0));
        assert_eq!(target(&order, JobId(3), Move::Up), Some(0));
        assert_eq!(target(&order, JobId(3), Move::Down), Some(2));
        assert_eq!(target(&order, JobId(2), Move::To(9)), Some(2), "clamped");
        assert_eq!(target(&order, JobId(2), Move::Top), None, "already first");
        assert_eq!(target(&order, JobId(2), Move::Up), None);
        assert_eq!(target(&order, JobId(4), Move::Down), None, "already last");
        assert_eq!(
            target(&order, JobId(1), Move::Top),
            None,
            "the running job is not in line"
        );
        assert_eq!(
            moved("Design review", 0),
            "Design review moved to position 1"
        );
    }

    #[test]
    fn the_cancel_toast_warns_only_when_undo_restarts() {
        assert_eq!(cancelled("Design review", false), "Cancelled Design review");
        assert_eq!(
            cancelled("Weekly sync", true),
            "Cancelled Weekly sync. Undo restarts its summary."
        );
    }

    #[test]
    fn voiceover_hears_stage_changes_not_percents() {
        let before = busy();
        let mut after = busy();
        after.now = Some(weekly(transcribing(41., 3, None)));
        assert_eq!(announcement(&before, &after), None, "a percent is quiet");
        after.now = Some(weekly(Phase::Summarizing {
            left: Left::Estimating,
        }));
        assert_eq!(
            announcement(&before, &after).as_deref(),
            Some("Weekly sync, summarizing, estimating time left.")
        );
        let mut done = busy();
        done.finished.push(Finished {
            job: JobId(1),
            recording: id("weekly"),
            title: "Weekly sync".into(),
            ended: Ended::Done,
            at: (14, 32),
        });
        assert_eq!(
            announcement(&before, &done).as_deref(),
            Some("Weekly sync, done at 14:32.")
        );
        let mut blocked = busy();
        blocked.attention.push(hiring());
        assert!(
            announcement(&before, &blocked)
                .unwrap()
                .contains("needs attention")
        );
        let mut cancelled = busy();
        cancelled.finished.push(Finished {
            ended: Ended::Cancelled,
            ..done.finished[0].clone()
        });
        assert_eq!(
            announcement(&before, &cancelled),
            None,
            "cancel speaks through its toast"
        );
    }
}
