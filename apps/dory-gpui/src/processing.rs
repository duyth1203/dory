//! The processing rules of UX-11 to UX-13 and UX-30 as plain data and pure functions: what a
//! stopped job says and the one button it offers, how long the rest of a run takes at the rate
//! observed so far, why a job runs light, the Processing choices, and which rows just finished or
//! failed. Nothing here touches GPUI or the store.

use dory_core::app::LightReason;
use dory_core::domain::{
    Blocker, Fraction, JobState, Ms, Processing, RecordingId, RecordingRow, Stage,
};
use dory_core::models::{KindTag, ModelRef, UnavailableReason};
use dory_core::ports::Pace;

/// The one button a stopped job offers (UX-13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// The summary model is missing or unreachable: the sheet with the `ollama pull` command.
    SetUpSummaries,
    /// A speech or speaker model is not on this Mac.
    Download(ModelRef),
    /// Unblocks a blocked job, or retries a failed one.
    TryAgain,
}

/// What a job that stopped needs from the user: one sentence with the cause, and its fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub text: String,
    pub fix: Fix,
}

impl Problem {
    /// `None` unless the job is blocked or failed.
    pub fn of(job: &JobState) -> Option<Problem> {
        match job {
            JobState::Blocked { stage, blocker } => {
                let summaries = *stage == Stage::Summarize
                    || matches!(blocker, Blocker::NoModel { kind: KindTag::Llm });
                let fix = match blocker {
                    _ if summaries => Fix::SetUpSummaries,
                    Blocker::NeedsProvision { model, .. } => Fix::Download(model.clone()),
                    _ => Fix::TryAgain,
                };
                Some(Problem {
                    text: blocked_text(blocker),
                    fix,
                })
            }
            JobState::Failed { stage, error } => Some(Problem {
                text: format!("{} failed: {}", stage_noun(*stage), error.message),
                fix: Fix::TryAgain,
            }),
            _ => None,
        }
    }
}

fn blocked_text(blocker: &Blocker) -> String {
    match blocker {
        Blocker::NeedsProvision { model, bytes } => match bytes {
            Some(b) => format!("{model} is not on this Mac ({} MB).", b / 1_000_000),
            None => format!("{model} is not on this Mac."),
        },
        // The provider's own fix, such as turning on Apple Intelligence, is the only way out.
        Blocker::Unavailable { model, why } => match &why.fix {
            Some(fix) => format!(
                "{model} is unavailable: {}. {fix}",
                unavailable(&why.reason)
            ),
            None => format!("{model} is unavailable: {}.", unavailable(&why.reason)),
        },
        Blocker::Incompatible { model, why } => format!("{model} {why}."),
        Blocker::NoModel { kind } => format!("No {kind} model is set up."),
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

fn stage_noun(stage: Stage) -> &'static str {
    match stage {
        Stage::Transcribe => "Transcription",
        Stage::Summarize => "The summary",
    }
}

/// Audio milliseconds processed per wall-clock millisecond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate(f64);

impl Rate {
    /// A guess for a Mac Dory has not watched transcribe yet: a quarter of real time.
    pub const GUESS: Rate = Rate(4.0);

    /// The rate over `elapsed_ms` in which `audio` went from `from` to `to` done. `None` until
    /// some audio and some time have passed.
    pub fn observed(audio: Ms, from: Fraction, to: Fraction, elapsed_ms: u64) -> Option<Rate> {
        let done = f64::from(to.get() - from.get()) * audio.0 as f64;
        (done > 0.0 && elapsed_ms > 0).then(|| Rate(done / elapsed_ms as f64))
    }

    pub fn time_for(self, audio: Ms) -> Ms {
        Ms((audio.0 as f64 / self.0).round() as u64)
    }
}

/// UX-12 shows the time left only once this much audio is done, so the rate has settled.
pub const ESTIMATE_AFTER: Ms = Ms(60_000);

/// The time the rest of `audio` takes at `rate`, once [`ESTIMATE_AFTER`] of it is done.
pub fn time_left(audio: Ms, done: Fraction, rate: Rate) -> Option<Ms> {
    let done_ms = f64::from(done.get()) * audio.0 as f64;
    (done_ms >= ESTIMATE_AFTER.0 as f64)
        .then(|| rate.time_for(Ms((audio.0 as f64 - done_ms).max(0.0) as u64)))
}

/// "about 3 min left", rounded up so the estimate never promises too much.
pub fn left_label(left: Ms) -> String {
    match left.0.div_ceil(60_000) {
        0 | 1 => "about 1 min left".into(),
        minutes => format!("about {minutes} min left"),
    }
}

/// The finish card's sentence with the estimated time (UX-11).
pub fn finish_estimate(audio: Ms, rate: Option<Rate>) -> String {
    let minutes = rate
        .unwrap_or(Rate::GUESS)
        .time_for(audio)
        .0
        .div_ceil(60_000)
        .max(1);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!("Transcribing takes about {minutes} {unit} on this Mac.")
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

/// What the row and the detail add after the stage while a job runs light (UX-30).
pub fn light_label(reason: LightReason) -> &'static str {
    match reason {
        LightReason::Chosen => "Light, chosen in Settings",
        LightReason::Battery => "Light, on battery",
        LightReason::LowPower => "Light, Low Power Mode",
        LightReason::Hot => "Light, Mac is hot",
        LightReason::Recording => "Light, while recording",
    }
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

#[cfg(test)]
mod tests {
    use dory_core::domain::{FailureClass, JobError};

    use super::*;
    use crate::fixtures;

    #[test]
    fn problems_read_as_the_cli_words_them_with_one_fix_each() {
        let failed = JobState::Failed {
            stage: Stage::Summarize,
            error: JobError {
                class: FailureClass::Permanent,
                message: "The summary model isn't installed.".into(),
                cause: dory_core::domain::FailureCause::Other,
            },
        };
        assert_eq!(
            Problem::of(&failed),
            Some(Problem {
                text: "The summary failed: The summary model isn't installed.".into(),
                fix: Fix::TryAgain,
            })
        );
        let model = ModelRef::parse("fake/whisper").unwrap();
        let provision = JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NeedsProvision {
                model: model.clone(),
                bytes: Some(1_500_000_000),
            },
        };
        assert_eq!(
            Problem::of(&provision),
            Some(Problem {
                text: "fake/whisper is not on this Mac (1500 MB).".into(),
                fix: Fix::Download(model.clone()),
            })
        );
        let summary_missing = JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::NeedsProvision { model, bytes: None },
        };
        assert_eq!(
            Problem::of(&summary_missing).unwrap().fix,
            Fix::SetUpSummaries
        );
        let no_llm = JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NoModel { kind: KindTag::Llm },
        };
        assert_eq!(Problem::of(&no_llm).unwrap().fix, Fix::SetUpSummaries);
        let no_stt = JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NoModel { kind: KindTag::Stt },
        };
        assert_eq!(
            Problem::of(&no_stt),
            Some(Problem {
                text: "No speech model is set up.".into(),
                fix: Fix::TryAgain,
            })
        );
        assert_eq!(Problem::of(&JobState::Queued), None);
    }

    #[test]
    fn time_left_waits_for_a_minute_of_audio_then_uses_the_observed_rate() {
        let audio = Ms(10 * 60_000);
        let rate = Rate::observed(audio, Fraction::new(0.0), Fraction::new(0.2), 30_000).unwrap();
        assert_eq!(rate.time_for(Ms(120_000)), Ms(30_000), "4x real time");
        assert_eq!(
            time_left(audio, Fraction::new(0.05), rate),
            None,
            "30 s done"
        );
        assert_eq!(
            time_left(audio, Fraction::new(0.2), rate),
            Some(Ms(120_000)),
            "8 min of audio left at 4x"
        );
        assert_eq!(left_label(Ms(120_000)), "about 2 min left");
        assert_eq!(left_label(Ms(121_000)), "about 3 min left");
        assert_eq!(left_label(Ms(5_000)), "about 1 min left");
        assert_eq!(
            Rate::observed(audio, Fraction::new(0.2), Fraction::new(0.2), 1_000),
            None
        );
    }

    #[test]
    fn the_finish_estimate_uses_the_observed_rate_or_a_guess() {
        let audio = Ms(12 * 60_000);
        assert_eq!(
            finish_estimate(audio, None),
            "Transcribing takes about 3 minutes on this Mac."
        );
        let slow = Rate::observed(audio, Fraction::new(0.0), Fraction::new(0.5), 6 * 60_000);
        assert_eq!(
            finish_estimate(audio, slow),
            "Transcribing takes about 12 minutes on this Mac."
        );
        assert_eq!(
            finish_estimate(Ms(10_000), None),
            "Transcribing takes about 1 minute on this Mac."
        );
    }

    #[test]
    fn outcomes_name_rows_whose_job_just_finished_or_stopped() {
        let rows = fixtures::rows();
        let mut before = rows.clone();
        let mut after = rows.clone();
        let running = Some(JobState::Running {
            stage: Stage::Transcribe,
        });
        before[0].job = running.clone();
        after[0].job = Some(JobState::Succeeded);
        before[1].job = Some(JobState::Queued);
        after[1].job = Some(JobState::Failed {
            stage: Stage::Transcribe,
            error: JobError {
                class: FailureClass::Permanent,
                message: "broke".into(),
                cause: dory_core::domain::FailureCause::Other,
            },
        });
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
