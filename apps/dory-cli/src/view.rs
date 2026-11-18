//! Plain-text rendering of domain values. Pure, so the shapes are tested without a data folder.

use dory_core::app::{Budget, LightReason};
use dory_core::domain::Channel;
use dory_core::domain::{
    Blocker, ChannelLayout, Job, JobState, Ms, Segment, SpeakerId, SpeakerNames, Stage, Timestamp,
    Transcript, Translation,
};
use dory_core::models::{Availability, Fit, Mismatch, UnavailableReason};

/// `2025-11-15 03:12:04`, UTC.
pub fn utc(at: Timestamp) -> String {
    let secs = at.0.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let in_day = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60
    )
}

/// `m:ss`, or `h:mm:ss` from an hour on. Rounds to the nearest second.
pub fn clock(ms: Ms) -> String {
    let secs = (ms.0 + 500) / 1000;
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn stage(stage: Stage) -> &'static str {
    match stage {
        Stage::Transcribe => "transcribe",
        Stage::Translate => "translate",
        Stage::Summarize => "summarize",
        Stage::TranslateSummary => "translate summary",
    }
}

/// `fast, 4 threads`, or `light, 2 threads, on battery`.
pub fn budget(budget: Budget) -> String {
    let threads = budget.threads();
    match budget.reason() {
        None => format!("fast, {threads} threads"),
        Some(reason) => format!(
            "light, {threads} threads, {}",
            match reason {
                LightReason::Chosen => "set to light",
                LightReason::Battery => "on battery",
                LightReason::LowPower => "Low Power Mode",
                LightReason::Hot => "the Mac is hot",
                LightReason::Recording => "recording",
            }
        ),
    }
}

pub fn job_state(state: &JobState) -> String {
    match state {
        JobState::Queued => "queued".into(),
        JobState::Running { stage: s } => format!("running {}", stage(*s)),
        JobState::Blocked { stage: s, .. } => format!("blocked at {}", stage(*s)),
        JobState::Succeeded => "done".into(),
        JobState::Failed { stage: s, .. } => format!("failed at {}", stage(*s)),
        JobState::Cancelled => "cancelled".into(),
    }
}

pub fn layout(layout: ChannelLayout) -> &'static str {
    match layout {
        ChannelLayout::MicSystem => "mic+system",
        ChannelLayout::MicOnly => "mic",
        ChannelLayout::SystemOnly => "system",
        ChannelLayout::Mixed => "mixed",
    }
}

pub fn dbfs(peak: f32) -> String {
    if peak <= 0.0 {
        "-inf dBFS".into()
    } else {
        format!("{:.1} dBFS", 20.0 * peak.log10())
    }
}

/// What is wrong and what to do about it.
pub fn blocker(blocker: &Blocker) -> (String, String) {
    match blocker {
        Blocker::NeedsProvision { model, bytes } => (
            match bytes {
                Some(b) => format!("{model} is not on this machine ({} MB)", b / 1_000_000),
                None => format!("{model} is not on this machine"),
            },
            format!("download {model}, then run process again"),
        ),
        Blocker::Unavailable { model, why } => (
            format!(
                "{model} is unavailable: {}",
                unavailable_reason(&why.reason)
            ),
            why.fix
                .clone()
                .unwrap_or_else(|| "pick another model, then run process again".into()),
        ),
        Blocker::Incompatible {
            model,
            why: Mismatch::NothingRecognized,
        } => (
            format!("{model} heard speech but recognized no words"),
            "add the meeting's languages with dory-cli languages TAG..., then run process again"
                .into(),
        ),
        Blocker::Incompatible {
            model,
            why: Mismatch::LanguagesUnknown,
        } => (
            format!("{model} {}", Mismatch::LanguagesUnknown),
            "confirm the meeting's languages with dory-cli languages TAG..., or pick another model, then run process again"
                .into(),
        ),
        Blocker::Incompatible { model, why } => (
            format!("{model} {why}"),
            "pick another model, or set the recording's language".into(),
        ),
        Blocker::NoModel { kind } => (
            format!("no {kind} model is registered"),
            "register an engine, or use --fake-engines to try the pipeline".into(),
        ),
    }
}

pub fn unavailable_reason(reason: &UnavailableReason) -> String {
    match reason {
        UnavailableReason::UnsupportedPlatform => "not supported on this platform".into(),
        UnavailableReason::OsTooOld { required } => format!("needs {required}"),
        UnavailableReason::NotEligible { detail }
        | UnavailableReason::Preparing { detail }
        | UnavailableReason::TurnedOff { detail, .. } => detail.clone(),
        UnavailableReason::ServerUnreachable { endpoint } => format!("cannot reach {endpoint}"),
    }
}

pub fn availability(availability: &Availability) -> String {
    match availability {
        Availability::Ready => "ready".into(),
        Availability::NeedsProvision { bytes: Some(b) } => {
            format!("needs download ({} MB)", b / 1_000_000)
        }
        Availability::NeedsProvision { bytes: None } => "needs download".into(),
        Availability::Unavailable(u) => {
            let mut text = format!("unavailable, {}", unavailable_reason(&u.reason));
            if let Some(fix) = &u.fix {
                text.push_str(&format!(" ({fix})"));
            }
            text
        }
    }
}

pub fn fit(fit: &Fit) -> String {
    match fit {
        Fit::Yes => "fits".into(),
        Fit::Degraded(why) => format!("fits, degraded: {why}"),
        Fit::No(why) => format!("does not fit: {why}"),
    }
}

/// One speaker's consecutive segments, joined.
#[derive(Debug, PartialEq)]
pub struct Turn {
    pub speaker: SpeakerId,
    pub channel: Channel,
    pub start: Option<Ms>,
    pub text: String,
}

pub fn turns(segments: &[Segment]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for segment in segments {
        match turns.last_mut() {
            Some(turn) if turn.speaker == segment.speaker => {
                turn.text.push(' ');
                turn.text.push_str(&segment.text);
            }
            _ => turns.push(Turn {
                speaker: segment.speaker,
                channel: segment.channel,
                start: segment.range.map(|r| r.start()),
                text: segment.text.clone(),
            }),
        }
    }
    turns
}

pub fn speaker_label(turn: &Turn, transcript: &Transcript, names: &SpeakerNames) -> String {
    if let Some(name) = names.name_of(turn.speaker, transcript.revision) {
        return name.to_owned();
    }
    if turn.speaker == SpeakerId::LOCAL && turn.channel == Channel::Mic {
        return "Me".into();
    }
    format!("Speaker {}", turn.speaker.0)
}

/// The transcript as turns. With a translation that applies, each segment reads as translated,
/// else as spoken.
pub fn transcript_lines(
    transcript: &Transcript,
    names: &SpeakerNames,
    translation: Option<&Translation>,
) -> Vec<String> {
    let shown: Vec<Segment> = transcript
        .segments
        .iter()
        .enumerate()
        .map(|(ix, segment)| Segment {
            text: translation
                .map_or(segment.text.as_str(), |t| t.shown(ix, segment))
                .to_owned(),
            ..segment.clone()
        })
        .collect();
    turns(&shown)
        .iter()
        .map(|turn| {
            let who = speaker_label(turn, transcript, names);
            match turn.start {
                Some(start) => format!("[{}] {who}: {}", clock(start), turn.text),
                None => format!("{who}: {}", turn.text),
            }
        })
        .collect()
}

/// The newest job, which is the one that says where the recording stands.
pub fn latest_job(jobs: Vec<Job>) -> Option<Job> {
    jobs.into_iter().max_by_key(|j| j.id)
}

/// A model's explainer, indented under its row: what the UX-28 popover shows.
pub fn about(about: &dory_core::models::ModelAbout, disk_bytes: Option<u64>) -> String {
    let mut text = format!("    {}\n", about.summary);
    for pro in &about.pros {
        text.push_str(&format!("    + {pro}\n"));
    }
    for con in &about.cons {
        text.push_str(&format!("    - {con}\n"));
    }
    if let Some(bytes) = disk_bytes {
        text.push_str(&format!("    {:.1} GB on disk\n", bytes as f64 / 1e9));
    }
    text
}

/// One progress line, in 5% steps so a long download prints about twenty lines.
pub fn provision_progress(event: &dory_core::app::Event) -> String {
    match event {
        dory_core::app::Event::ProvisionProgress {
            step, done, total, ..
        } => match total {
            Some(total) if *total > 0 => {
                format!("{step} {}%", done * 100 / total / 5 * 5)
            }
            _ => format!("{step} {:.0} MB", *done as f64 / 1e6),
        },
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{Revision, TimeRange};
    use dory_core::models::ModelRef;

    use super::*;

    fn segment(speaker: u16, channel: Channel, range: Option<(u64, u64)>, text: &str) -> Segment {
        Segment {
            range: range.and_then(|(a, b)| TimeRange::new(Ms(a), Ms(b))),
            speaker: SpeakerId(speaker),
            channel,
            text: text.into(),
            language: None,
        }
    }

    fn transcript(segments: Vec<Segment>) -> Transcript {
        Transcript {
            revision: Revision(1),
            segments,
            detected_languages: vec![],
            produced_by: ModelRef::parse("fake/stt").unwrap(),
        }
    }

    #[test]
    fn a_model_blocked_by_unknown_meeting_languages_points_at_the_languages_command() {
        let model = ModelRef::parse("fake/en-only-llm").unwrap();
        let blocked = |why| Blocker::Incompatible {
            model: model.clone(),
            why,
        };
        let (what, fix) = blocker(&blocked(Mismatch::LanguagesUnknown));
        assert_eq!(
            what,
            "fake/en-only-llm needs your meeting languages set in Settings"
        );
        assert!(
            fix.contains("dory-cli languages TAG..."),
            "the fix names the command that confirms them: {fix}"
        );
        assert!(!fix.contains("recording's language"), "{fix}");
        let (_, other) = blocker(&blocked(Mismatch::LanguageUnsupported(Vec::new())));
        assert_eq!(
            other, "pick another model, or set the recording's language",
            "every other mismatch keeps its hint"
        );
    }

    #[test]
    fn utc_table() {
        for (ms, want) in [
            (0, "1970-01-01 00:00:00"),
            (1_731_234_567_000, "2024-11-10 10:29:27"),
            (1_763_300_000_999, "2025-11-16 13:33:20"),
        ] {
            assert_eq!(utc(Timestamp(ms)), want);
        }
    }

    #[test]
    fn clock_rounds_and_adds_hours() {
        for (ms, want) in [
            (0, "0:00"),
            (499, "0:00"),
            (500, "0:01"),
            (7_990, "0:08"),
            (1_832_400, "30:32"),
            (3_600_000, "1:00:00"),
        ] {
            assert_eq!(clock(Ms(ms)), want, "{ms}");
        }
    }

    #[test]
    fn timed_turns_merge_a_speakers_run_and_name_the_local_user() {
        let t = transcript(vec![
            segment(0, Channel::Mic, Some((0, 900)), "Hello."),
            segment(0, Channel::Mic, Some((1_000, 1_500)), "Again."),
            segment(2, Channel::System, Some((65_000, 66_000)), "Hi."),
        ]);
        assert_eq!(
            transcript_lines(&t, &SpeakerNames::default(), None),
            ["[0:00] Me: Hello. Again.", "[1:05] Speaker 2: Hi."]
        );
    }

    #[test]
    fn untimed_turns_have_no_timestamp_and_names_win() {
        let t = transcript(vec![
            segment(1, Channel::Mixed, None, "Chào."),
            segment(2, Channel::Mixed, None, "Okay."),
        ]);
        let mut names = SpeakerNames::default();
        names.rename(Revision(1), SpeakerId(2), Some("Ana".into()));
        assert_eq!(
            transcript_lines(&t, &names, None),
            ["Speaker 1: Chào.", "Ana: Okay."]
        );
    }
}
