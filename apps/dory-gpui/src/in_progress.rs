//! What is in progress (UX-8): the take, the running job, the waiting count, and downloads. The
//! quit confirmation and the menu-bar item's menu both read this one model, so they never
//! disagree about what quitting would interrupt.

use dory_core::app::RecorderStatus;
use dory_core::domain::{Job, JobState, Ms, RecordingId, Stage};

use crate::library::{Percent, Progress};
use crate::processing::Download;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InProgress {
    pub take: Option<Take>,
    pub running: Option<Running>,
    /// Jobs waiting behind the running one. Blocked jobs wait for a fix, not for time, so they
    /// are not counted.
    pub queued: usize,
    pub downloads: Vec<Downloading>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Take {
    pub title: String,
    pub state: TakeState,
    pub elapsed: Ms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeState {
    Recording,
    Paused,
    /// Stopped, in the Undo window, before it is saved for good.
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub title: String,
    pub stage: Stage,
    /// `None` until the stage reports progress.
    pub percent: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloading {
    pub name: String,
    pub percent: Option<u8>,
}

/// What [`InProgress::of`] reads, already looked up by `AppState`.
pub struct Facts<'a> {
    pub recorder: &'a RecorderStatus,
    /// The take's recorded time, live while recording.
    pub elapsed: Ms,
    /// Running, queued, and blocked jobs in queue order.
    pub queue: &'a [Job],
    pub title_of: &'a dyn Fn(&RecordingId) -> String,
    pub progress_of: &'a dyn Fn(&RecordingId) -> Option<Progress>,
    /// Each download's model name, in a stable order.
    pub downloads: Vec<(String, Download)>,
}

impl InProgress {
    pub fn of(facts: Facts<'_>) -> InProgress {
        let take = match facts.recorder {
            RecorderStatus::Idle => None,
            RecorderStatus::Recording { id, .. } => Some((id, TakeState::Recording)),
            RecorderStatus::Paused { id, .. } => Some((id, TakeState::Paused)),
            RecorderStatus::Grace { id, .. } => Some((id, TakeState::Stopped)),
        }
        .map(|(id, state)| Take {
            title: (facts.title_of)(id),
            state,
            elapsed: facts.elapsed,
        });
        let running = facts.queue.iter().find_map(|job| match job.state {
            JobState::Running { stage } => Some(Running {
                title: (facts.title_of)(&job.recording),
                stage,
                percent: (facts.progress_of)(&job.recording)
                    .filter(|p| p.stage == stage)
                    .map(|p| Percent::of(p.fraction).get()),
            }),
            _ => None,
        });
        let queued = facts
            .queue
            .iter()
            .filter(|job| job.state == JobState::Queued)
            .count();
        let downloads = facts
            .downloads
            .into_iter()
            .map(|(name, download)| Downloading {
                name,
                percent: download.percent().map(Percent::get),
            })
            .collect();
        InProgress {
            take,
            running,
            queued,
            downloads,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.take.is_none() && !self.is_processing() && self.downloads.is_empty()
    }

    /// A job runs or waits.
    pub fn is_processing(&self) -> bool {
        self.running.is_some() || self.queued > 0
    }

    /// Recordings still being processed: the running one and those waiting.
    pub fn processing(&self) -> usize {
        usize::from(self.running.is_some()) + self.queued
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{Fraction, Goal, JobId, Priority, Timestamp};

    use super::*;

    fn id(n: u32) -> RecordingId {
        RecordingId::parse(&format!("r{n}")).unwrap()
    }

    fn job(n: u32, state: JobState) -> Job {
        Job {
            id: JobId(u64::from(n)),
            recording: id(n),
            goal: Goal::ALL,
            done: Vec::new(),
            state,
            priority: Priority::Normal,
            run_fast: false,
            attempts: 0,
            not_before: None,
            created_at: Timestamp(0),
            place: 0,
        }
    }

    fn title_of(id: &RecordingId) -> String {
        format!("Title {id}")
    }

    fn of(
        recorder: &RecorderStatus,
        queue: &[Job],
        progress: Option<Progress>,
        downloads: Vec<(String, Download)>,
    ) -> InProgress {
        InProgress::of(Facts {
            recorder,
            elapsed: Ms(754_000),
            queue,
            title_of: &title_of,
            progress_of: &|_| progress,
            downloads,
        })
    }

    #[test]
    fn nothing_running_is_empty() {
        let blocked = job(
            1,
            JobState::Blocked {
                stage: Stage::Summarize,
                blocker: dory_core::domain::Blocker::NoModel {
                    kind: dory_core::models::KindTag::Llm,
                },
            },
        );
        let state = of(&RecorderStatus::Idle, &[blocked], None, Vec::new());
        assert_eq!(state, InProgress::default());
        assert!(
            state.is_empty(),
            "a blocked job waits for a fix, not for quitting"
        );
    }

    #[test]
    fn the_take_is_in_progress_while_recording_paused_or_in_the_undo_window() {
        let recorded = Ms(0);
        for (status, state) in [
            (
                RecorderStatus::Recording {
                    id: id(7),
                    recorded,
                },
                TakeState::Recording,
            ),
            (
                RecorderStatus::Paused {
                    id: id(7),
                    recorded,
                },
                TakeState::Paused,
            ),
            (
                RecorderStatus::Grace {
                    id: id(7),
                    recorded,
                },
                TakeState::Stopped,
            ),
        ] {
            let found = of(&status, &[], None, Vec::new());
            assert_eq!(
                found.take,
                Some(Take {
                    title: "Title r7".into(),
                    state,
                    elapsed: Ms(754_000),
                }),
                "{status:?}"
            );
            assert!(!found.is_empty());
        }
    }

    #[test]
    fn the_running_job_has_its_stage_and_the_percent_of_that_stage_only() {
        let queue = [
            job(
                1,
                JobState::Running {
                    stage: Stage::Transcribe,
                },
            ),
            job(2, JobState::Queued),
            job(3, JobState::Queued),
        ];
        let transcribing = Progress {
            stage: Stage::Transcribe,
            fraction: Fraction::new(0.4),
            budget: None,
        };
        let found = of(
            &RecorderStatus::Idle,
            &queue,
            Some(transcribing),
            Vec::new(),
        );
        assert_eq!(
            found.running,
            Some(Running {
                title: "Title r1".into(),
                stage: Stage::Transcribe,
                percent: Some(40),
            })
        );
        assert_eq!(found.queued, 2);
        assert_eq!(found.processing(), 3);

        let stale = Progress {
            stage: Stage::Summarize,
            ..transcribing
        };
        let found = of(&RecorderStatus::Idle, &queue, Some(stale), Vec::new());
        assert_eq!(found.running.unwrap().percent, None);
    }

    #[test]
    fn queued_jobs_alone_are_processing() {
        let found = of(
            &RecorderStatus::Idle,
            &[job(2, JobState::Queued)],
            None,
            Vec::new(),
        );
        assert_eq!(found.running, None);
        assert_eq!(found.processing(), 1);
        assert!(found.is_processing());
    }

    #[test]
    fn downloads_keep_their_names_and_a_percent_once_the_size_is_known() {
        let found = of(
            &RecorderStatus::Idle,
            &[],
            None,
            vec![
                (
                    "Parakeet".into(),
                    Download {
                        done: 30,
                        total: Some(100),
                    },
                ),
                (
                    "Pyannote".into(),
                    Download {
                        done: 5,
                        total: None,
                    },
                ),
            ],
        );
        assert_eq!(
            found.downloads,
            vec![
                Downloading {
                    name: "Parakeet".into(),
                    percent: Some(30)
                },
                Downloading {
                    name: "Pyannote".into(),
                    percent: None
                },
            ]
        );
        assert!(!found.is_empty());
    }
}
