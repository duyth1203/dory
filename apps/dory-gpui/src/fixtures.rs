//! The sample recordings of section 6 of `docs/design/handoff-direction-c.md`, for tests and
//! previews, so screens match the canvas. Roadmap review comes from the canvas's October group,
//! so a month group has a member.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone};
use dory_core::domain::{
    Blocker, Channel, ChannelLayout, FailureClass, JobError, JobState, LanguagePref, Ms, Recording,
    RecordingId, RecordingRow, Revision, Segment, SpeakerId, Stage, Summary, SummaryFlag,
    TakeContainer, TakeInfo, TimeRange, Timestamp, Transcript,
};
use dory_core::models::{KindTag, ModelRef};

use crate::processing::Phase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleStatus {
    Transcribing,
    Done,
    NeedsAttention,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub title: &'static str,
    /// Days before the sample "today", Thursday 12 November 2026.
    pub days_ago: i64,
    pub start: (u32, u32),
    pub minutes: u64,
    pub status: SampleStatus,
    /// One sentence, with a word no other sample's transcript has, so search can find it.
    pub transcript: &'static str,
}

pub const SAMPLES: [Sample; 8] = [
    Sample {
        title: "Weekly sync",
        days_ago: 0,
        start: (14, 0),
        minutes: 62,
        status: SampleStatus::Transcribing,
        transcript: "",
    },
    Sample {
        title: "Design review",
        days_ago: 0,
        start: (10, 30),
        minutes: 48,
        status: SampleStatus::Done,
        transcript: "Let's settle the export formats before the waveform work.",
    },
    Sample {
        title: "1:1 with Sam",
        days_ago: 0,
        start: (9, 0),
        minutes: 27,
        status: SampleStatus::Done,
        transcript: "Sam wants a mentoring plan for the quarter.",
    },
    Sample {
        title: "Kundengespräch Müller GmbH",
        days_ago: 1,
        start: (16, 15),
        minutes: 35,
        status: SampleStatus::Done,
        transcript: "Die Lieferung kommt nächste Woche.",
    },
    Sample {
        title: "Partner call, Lyon office",
        days_ago: 1,
        start: (11, 0),
        minutes: 41,
        status: SampleStatus::Done,
        transcript: "On se retrouve à la gare Part-Dieu, then we switch to English.",
    },
    Sample {
        title: "Hiring debrief",
        days_ago: 1,
        start: (9, 30),
        minutes: 19,
        status: SampleStatus::NeedsAttention,
        transcript: "The candidate was strong on systems design.",
    },
    Sample {
        title: "Customer interview: Ortega",
        days_ago: 3,
        start: (13, 0),
        minutes: 94,
        status: SampleStatus::Done,
        transcript: "Ortega asked about offline transcription on flights.",
    },
    Sample {
        title: "Roadmap review",
        days_ago: 15,
        start: (15, 0),
        minutes: 71,
        status: SampleStatus::Done,
        transcript: "The roadmap puts the menu-bar extra in the next release.",
    },
];

/// Bangkok time, so tests never depend on the machine's zone.
pub fn zone() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).expect("a valid offset")
}

/// The canvas's "now": Thursday 12 November 2026, 16:00.
pub fn now() -> DateTime<FixedOffset> {
    zone()
        .with_ymd_and_hms(2026, 11, 12, 16, 0, 0)
        .single()
        .expect("a valid local time")
}

pub fn today() -> NaiveDate {
    now().date_naive()
}

impl Sample {
    pub fn created_at(&self) -> Timestamp {
        let day = now() - Duration::days(self.days_ago);
        let at = zone()
            .from_local_datetime(
                &day.date_naive()
                    .and_hms_opt(self.start.0, self.start.1, 0)
                    .expect("a valid time"),
            )
            .single()
            .expect("a valid local time");
        Timestamp(at.timestamp_millis())
    }

    pub fn id(&self) -> RecordingId {
        RecordingId::generate(self.created_at(), 0)
    }

    pub fn recording(&self) -> Recording {
        Recording {
            id: self.id(),
            title: Some(self.title.to_owned()),
            created_at: self.created_at(),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: Some(TakeInfo {
                container: TakeContainer::Flac,
                rate_hz: 48_000,
                duration: Ms(self.minutes * 60_000),
            }),
        }
    }

    pub fn transcript(&self) -> Option<Transcript> {
        (!self.transcript.is_empty()).then(|| Transcript {
            revision: Revision(1),
            segments: vec![Segment {
                range: TimeRange::new(Ms(0), Ms(4_000)),
                speaker: SpeakerId(1),
                channel: Channel::System,
                text: self.transcript.to_owned(),
            }],
            detected_languages: Vec::new(),
            produced_by: model(),
        })
    }

    pub fn summary(&self) -> Option<Summary> {
        (self.status == SampleStatus::Done).then(|| Summary {
            markdown: format!("## Key points\n\n- {}\n", self.title),
            language: None,
            produced_by: model(),
            transcript_revision: Revision(1),
        })
    }

    pub fn job(&self) -> Option<JobState> {
        match self.status {
            SampleStatus::Transcribing => Some(JobState::Running {
                stage: Stage::Transcribe,
            }),
            SampleStatus::Done => Some(JobState::Succeeded),
            SampleStatus::NeedsAttention => Some(JobState::Blocked {
                stage: Stage::Summarize,
                blocker: Blocker::NoModel { kind: KindTag::Llm },
            }),
        }
    }

    /// A terminal failure, for seeding a store whose pipeline would otherwise rerun the job.
    pub fn failure(&self) -> JobError {
        JobError {
            class: FailureClass::Permanent,
            message: "The summary model isn't installed.".into(),
            cause: dory_core::domain::FailureCause::Other,
        }
    }

    pub fn row(&self) -> RecordingRow {
        RecordingRow {
            recording: self.recording(),
            job: self.job(),
            has_transcript: !self.transcript.is_empty(),
            summary: if self.status == SampleStatus::Done {
                SummaryFlag::Current
            } else {
                SummaryFlag::None
            },
        }
    }
}

/// Every sample as the store lists it: newest first.
pub fn rows() -> Vec<RecordingRow> {
    let mut rows: Vec<RecordingRow> = SAMPLES.iter().map(Sample::row).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.recording.created_at));
    rows
}

fn model() -> ModelRef {
    ModelRef::parse("fake/sample").expect("a valid model ref")
}

/// Every state of the processing card, P1 to P8 (`docs/design/processing-view.md` section 4),
/// with lines streamed in while it transcribes.
pub fn card_phases() -> Vec<(&'static str, Phase, Vec<Segment>)> {
    use crate::estimate::Left;
    use crate::processing::{Problem, Spot};
    use dory_core::app::LightReason;
    use dory_core::domain::Fraction;
    const MIN: u64 = 60_000;
    let lines: Vec<Segment> = [
        (
            0,
            "Let's start with the launch checklist and who owns each item this week.",
        ),
        (
            6_000,
            "Design is done. The last two screens went to review on Monday.",
        ),
        (
            14_000,
            "Then we can ship on Friday if the release notes are ready in time.",
        ),
    ]
    .into_iter()
    .map(|(at, text)| Segment {
        range: TimeRange::new(Ms(at), Ms(at + 5_000)),
        speaker: SpeakerId(1),
        channel: Channel::System,
        text: text.into(),
    })
    .collect();
    let transcribing = |light| Phase::Transcribing {
        done: Fraction::new(0.4),
        left: Left::About(Ms(4 * MIN)),
        light,
    };
    vec![
        (
            "P1 stopped",
            Phase::Stopped {
                starts_in: 5,
                auto: true,
            },
            Vec::new(),
        ),
        ("P2 saving", Phase::Saving, Vec::new()),
        (
            "P3a queued",
            Phase::Queued(Spot {
                ahead: 2,
                starts_in: Some(Ms(9 * MIN)),
            }),
            Vec::new(),
        ),
        ("P3b preparing", Phase::Preparing, Vec::new()),
        ("P4 transcribing", transcribing(None), lines.clone()),
        (
            "P5 transcribing light",
            transcribing(Some(LightReason::Battery)),
            lines,
        ),
        (
            "P6 summarizing",
            Phase::Summarizing {
                left: Left::About(Ms(MIN)),
            },
            Vec::new(),
        ),
        ("P7 done", Phase::Done { summary: true }, Vec::new()),
        (
            "P8 failed",
            Phase::Failed(Problem::SpeechModelMissing {
                model: ModelRef::parse("sherpa/parakeet").unwrap(),
                kind: KindTag::Stt,
                name: "Parakeet".into(),
                bytes: Some(487_211_443),
                download: None,
            }),
            Vec::new(),
        ),
    ]
}

/// The Processing window's states for the visual pass and the layout checks, as frames Q1 to Q5
/// of `docs/design/floating-surfaces.md` show them, on the sample recordings. `busy` holds
/// every section at once, for the 800 × 560 and 200% checks (Q6, Q7).
pub const QUEUE_STATES: [&str; 6] = ["running", "light", "blocked", "finished", "empty", "busy"];

pub fn queue_state(name: &str) -> Option<crate::processing_queue::Queue> {
    use crate::estimate::Left;
    use crate::processing::Problem;
    use crate::processing_queue::{Blocked, Ended, Finished, Queue, Running, Waiting};
    use dory_core::app::LightReason;
    use dory_core::domain::{Fraction, JobId};
    const MIN: u64 = 60_000;
    let sample = |title: &str| {
        SAMPLES
            .iter()
            .find(|s| s.title == title)
            .expect("a sample recording")
    };
    let running = |light: bool| Running {
        job: JobId(1),
        recording: sample("Weekly sync").id(),
        title: "Weekly sync".into(),
        audio: Some(Ms(62 * MIN)),
        phase: Phase::Transcribing {
            done: Fraction::new(0.4),
            left: Left::About(Ms(if light { 9 } else { 4 } * MIN)),
            light: light.then_some(LightReason::Battery),
        },
    };
    let waiting = |job, title: &str, starts| {
        let s = sample(title);
        Waiting {
            job: JobId(job),
            recording: s.id(),
            title: title.into(),
            audio: Some(Ms(s.minutes * MIN)),
            starts_in: Some(Ms(starts * MIN)),
        }
    };
    let next = || {
        vec![
            waiting(2, "Design review", 4),
            waiting(3, "1:1 with Sam", 15),
        ]
    };
    let blocked = || Blocked {
        recording: sample("Hiring debrief").id(),
        title: "Hiring debrief".into(),
        problem: Problem::SummaryModelMissing,
    };
    let finished = || {
        vec![
            Finished {
                job: JobId(7),
                recording: sample("Kundengespräch Müller GmbH").id(),
                title: "Kundengespräch Müller GmbH".into(),
                ended: Ended::Done,
                at: (14, 32),
            },
            Finished {
                job: JobId(8),
                recording: sample("Partner call, Lyon office").id(),
                title: "Partner call, Lyon office".into(),
                ended: Ended::Cancelled,
                at: (14, 5),
            },
        ]
    };
    Some(match name {
        "running" => Queue {
            now: Some(running(false)),
            next: next(),
            left: Some(Ms(24 * MIN)),
            ..Queue::default()
        },
        "light" => Queue {
            now: Some(running(true)),
            next: next(),
            left: Some(Ms(41 * MIN)),
            ..Queue::default()
        },
        "blocked" => Queue {
            now: Some(running(false)),
            next: next(),
            attention: vec![blocked()],
            left: Some(Ms(24 * MIN)),
            ..Queue::default()
        },
        "finished" => Queue {
            next: vec![Waiting {
                starts_in: None,
                ..waiting(3, "1:1 with Sam", 0)
            }],
            finished: finished(),
            left: Some(Ms(9 * MIN)),
            ..Queue::default()
        },
        "empty" => Queue::default(),
        "busy" => Queue {
            now: Some(running(true)),
            next: vec![
                waiting(2, "Design review", 9),
                waiting(3, "1:1 with Sam", 31),
                waiting(4, "Customer interview: Ortega", 45),
                waiting(5, "Roadmap review", 80),
            ],
            attention: vec![blocked()],
            finished: finished(),
            left: Some(Ms(118 * MIN)),
        },
        _ => return None,
    })
}

/// Set Up Summaries' states U1 to U3, with the catalog's summary model (`gemma4:e4b`, as
/// `dory-engines` lists it) and a download a third of the way.
pub fn summaries_step(name: &str) -> Option<crate::set_up_summaries::Step> {
    use crate::processing::Download;
    use crate::set_up_summaries::{Model, Step};
    let gemma = || Model {
        name: "Gemma 4 E4B".into(),
        bytes: Some(9_461_576_343),
    };
    Some(match name.to_ascii_uppercase().as_str() {
        "U1" => Step::Install {
            model: Some(gemma()),
        },
        "U2" => Step::Downloading {
            name: gemma().name,
            download: Download {
                done: 3_400_000_000,
                total: gemma().bytes,
            },
        },
        "U3" => Step::Ready {
            waiting: Some("Hiring debrief".into()),
        },
        _ => return None,
    })
}

/// A recording, a running job with one waiting, and a download: everything quitting asks about.
pub fn busy() -> crate::in_progress::InProgress {
    use crate::in_progress::{Downloading, InProgress, Running, Take, TakeState};
    InProgress {
        take: Some(Take {
            title: "Weekly sync".into(),
            state: TakeState::Recording,
            elapsed: dory_core::domain::Ms(754_000),
        }),
        running: Some(Running {
            title: "Design review".into(),
            stage: dory_core::domain::Stage::Transcribe,
            percent: Some(40),
        }),
        queued: 1,
        downloads: vec![Downloading {
            name: "Parakeet".into(),
            percent: Some(30),
        }],
    }
}
