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
