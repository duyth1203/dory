//! The library's product rules as plain data and pure functions: date groups, row status, the
//! status filter, and where the selection goes after a delete. Nothing here touches GPUI or the
//! store, so every rule is tested without a window.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone};
use dory_core::app::Budget;
use dory_core::domain::{Fraction, Ms, RecordingId, RecordingRow, Stage, Timestamp};
use dory_core::search::{Correction, Excerpt, Section, Source};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    One(RecordingId),
}

impl Selection {
    pub fn id(&self) -> Option<&RecordingId> {
        match self {
            Selection::None => None,
            Selection::One(id) => Some(id),
        }
    }
}

/// What the user typed in the sidebar search field, trimmed. Empty means no search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchText(String);

impl SearchText {
    pub fn new(text: &str) -> Self {
        Self(text.trim().to_owned())
    }

    pub fn text(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The heading of a search section (UX-32).
pub fn section_label(section: Section) -> &'static str {
    match section {
        Section::Titles => "Titles",
        Section::Conversations => "In conversations",
    }
}

/// The words a correction put in the query, such as "roadmap", for the line "Showing results for
/// roadmap". `None` when nothing was corrected.
pub fn corrected_words(corrections: &[Correction]) -> Option<String> {
    let words: Vec<&str> = corrections
        .iter()
        .flat_map(|c| c.used.iter().map(String::as_str))
        .collect();
    (!words.is_empty()).then(|| words.join(", "))
}

/// What VoiceOver says once a search settles: "12 results", "1 result", "No results", or, after a
/// correction, "Showing results for roadmap, 3 results".
pub fn results_announcement(count: usize, corrections: &[Correction]) -> String {
    let count = match count {
        0 => "No results".to_owned(),
        1 => "1 result".to_owned(),
        n => format!("{n} results"),
    };
    match corrected_words(corrections) {
        Some(words) if count != "No results" => format!("Showing results for {words}, {count}"),
        _ => count,
    }
}

/// Where a result matched, as its VoiceOver label says it after the row's usual sentence: "Title
/// match", "In transcript, we agreed the roadmap slips", "In summary, ...", or "Speaker name,
/// ...". A conversation hit whose snippet has not loaded yet says nothing.
pub fn match_words(section: Section, excerpt: Option<&Excerpt>) -> Option<String> {
    if section == Section::Titles {
        return Some("Title match".into());
    }
    let excerpt = excerpt?;
    let where_ = match excerpt.source {
        Source::Transcript => "In transcript",
        Source::Summary => "In summary",
        Source::Speakers => "Speaker name",
    };
    let said = excerpt.snippet.text.trim_matches('…').trim();
    Some(format!("{where_}, {said}"))
}

/// UX-1's filter menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Processing,
    NeedsAttention,
    Done,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 4] = [
        StatusFilter::All,
        StatusFilter::Processing,
        StatusFilter::NeedsAttention,
        StatusFilter::Done,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "All",
            StatusFilter::Processing => "Processing",
            StatusFilter::NeedsAttention => "Needs attention",
            StatusFilter::Done => "Done",
        }
    }

    /// Processing holds everything that is not finished and not failing, so a recording that
    /// waits to be transcribed is never hidden by every filter but All.
    pub fn admits(self, status: RowStatus) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Processing => {
                !matches!(status, RowStatus::NeedsAttention | RowStatus::Done)
            }
            StatusFilter::NeedsAttention => status == RowStatus::NeedsAttention,
            StatusFilter::Done => status == RowStatus::Done,
        }
    }
}

/// A whole percent, `0..=100`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Percent(u8);

impl Percent {
    pub fn of(fraction: Fraction) -> Percent {
        Percent((fraction.get() * 100.0).round() as u8)
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

/// The last `StageProgress` seen for a recording's running job. Events carry it, never rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub stage: Stage,
    pub fraction: Fraction,
    /// The pace the stage runs at, so the row can say why it runs light (UX-30). The summary has none.
    pub budget: Option<Budget>,
}

/// The status a sidebar row shows: always a glyph and a word (A11Y-8), in the short forms of
/// `docs/design/processing-view.md` section 4. `processing::Phase::row` derives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    Recording,
    ReadyToTranscribe,
    /// P1, the Undo window.
    Stopped,
    /// P2.
    Saving,
    /// P3a.
    Queued {
        ahead: usize,
    },
    /// P3b.
    Preparing,
    /// P4, and P5 when `light`.
    Transcribing {
        percent: Percent,
        light: bool,
    },
    Summarizing,
    Done,
    NeedsAttention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Dot,
    Doc,
    StopCircle,
    /// Three dots in a circle: saving.
    DotsCircle,
    Clock,
    /// Three dots that breathe: under way.
    Dots,
    Check,
    Triangle,
}

/// Which theme color a status glyph takes. The theme maps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Secondary,
    Accent,
    Attention,
    Record,
}

impl RowStatus {
    pub fn label(self) -> String {
        match self {
            RowStatus::Recording => "Recording".into(),
            RowStatus::ReadyToTranscribe => "Ready to transcribe".into(),
            RowStatus::Stopped => "Stopped".into(),
            RowStatus::Saving => "Saving".into(),
            RowStatus::Queued { ahead: 0 } => "Queued".into(),
            RowStatus::Queued { ahead } => format!("Queued, {ahead} ahead"),
            RowStatus::Preparing => "Preparing".into(),
            RowStatus::Transcribing {
                percent,
                light: false,
            } => format!("Transcribing {}%", percent.get()),
            RowStatus::Transcribing {
                percent,
                light: true,
            } => format!("{}%, light", percent.get()),
            RowStatus::Summarizing => "Summarizing".into(),
            RowStatus::Done => "Done".into(),
            RowStatus::NeedsAttention => "Needs attention".into(),
        }
    }

    pub fn glyph(self) -> Glyph {
        match self {
            RowStatus::Recording => Glyph::Dot,
            RowStatus::ReadyToTranscribe => Glyph::Doc,
            RowStatus::Stopped => Glyph::StopCircle,
            RowStatus::Saving => Glyph::DotsCircle,
            RowStatus::Queued { .. } => Glyph::Clock,
            RowStatus::Preparing | RowStatus::Transcribing { .. } | RowStatus::Summarizing => {
                Glyph::Dots
            }
            RowStatus::Done => Glyph::Check,
            RowStatus::NeedsAttention => Glyph::Triangle,
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            RowStatus::Recording => Tone::Record,
            RowStatus::Saving
            | RowStatus::Preparing
            | RowStatus::Transcribing { .. }
            | RowStatus::Summarizing => Tone::Accent,
            RowStatus::NeedsAttention => Tone::Attention,
            RowStatus::ReadyToTranscribe
            | RowStatus::Stopped
            | RowStatus::Queued { .. }
            | RowStatus::Done => Tone::Secondary,
        }
    }
}

/// UX-1's sidebar groups, newest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateGroup {
    Today,
    Yesterday,
    /// The 7 days before yesterday.
    Previous7Days,
    Month {
        year: i32,
        month: u32,
    },
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

impl DateGroup {
    /// A recording dated after today (a clock moved back) counts as today.
    pub fn of(day: NaiveDate, today: NaiveDate) -> DateGroup {
        match (today - day).num_days() {
            ..=0 => DateGroup::Today,
            1 => DateGroup::Yesterday,
            2..=7 => DateGroup::Previous7Days,
            _ => DateGroup::Month {
                year: day.year(),
                month: day.month(),
            },
        }
    }

    /// A month in another year carries its year, so two Octobers never merge on screen.
    pub fn label(self, today: NaiveDate) -> String {
        match self {
            DateGroup::Today => "Today".into(),
            DateGroup::Yesterday => "Yesterday".into(),
            DateGroup::Previous7Days => "Previous 7 days".into(),
            DateGroup::Month { year, month } if year == today.year() => {
                MONTHS[month as usize - 1].into()
            }
            DateGroup::Month { year, month } => format!("{} {year}", MONTHS[month as usize - 1]),
        }
    }
}

pub fn local<Tz: TimeZone>(at: Timestamp, tz: &Tz) -> DateTime<Tz> {
    tz.timestamp_millis_opt(at.0)
        .earliest()
        .unwrap_or_else(|| tz.timestamp_millis_opt(0).unwrap())
}

/// Consecutive runs of `rows` that share a date group. The store already orders rows newest
/// first, so a group never repeats.
pub fn group_by_date<'a, Tz: TimeZone>(
    rows: impl IntoIterator<Item = &'a RecordingRow>,
    now: &DateTime<Tz>,
) -> Vec<(DateGroup, Vec<&'a RecordingRow>)> {
    let today = now.date_naive();
    let tz = now.timezone();
    let mut groups: Vec<(DateGroup, Vec<&RecordingRow>)> = Vec::new();
    for row in rows {
        let group = DateGroup::of(local(row.recording.created_at, &tz).date_naive(), today);
        match groups.last_mut() {
            Some((last, members)) if *last == group => members.push(row),
            _ => groups.push((group, vec![row])),
        }
    }
    groups
}

/// The selection after `removed` leaves a list that showed `visible`, in order: the next row,
/// else the previous one, else nothing.
pub fn after_removal(visible: &[RecordingId], removed: &RecordingId) -> Selection {
    let Some(ix) = visible.iter().position(|id| id == removed) else {
        return Selection::None;
    };
    visible
        .get(ix + 1)
        .or_else(|| ix.checked_sub(1).and_then(|prev| visible.get(prev)))
        .map_or(Selection::None, |id| Selection::One(id.clone()))
}

/// "62 min", or "45 s" under a minute. It rounds down, like the player's clock, so a 42.6 s
/// take reads "42 s" here and "0:42" in the player.
pub fn short_duration(ms: Ms) -> String {
    let secs = ms.0 / 1000;
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min", secs / 60)
    }
}

/// The sidebar row's meta line, the start time as the canvas shows it per group:
/// "14:00 · 62 min" today and yesterday, "Tue 10 Nov · 12 min" in the last week, and
/// "28 Oct · 71 min" before that.
pub fn row_meta<Tz: TimeZone>(row: &RecordingRow, group: DateGroup, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let at = local(row.recording.created_at, &now.timezone());
    let when = match group {
        DateGroup::Today | DateGroup::Yesterday => at.format("%H:%M").to_string(),
        DateGroup::Previous7Days => at.format("%a %-d %b").to_string(),
        DateGroup::Month { year, .. } if year == now.year() => at.format("%-d %b").to_string(),
        DateGroup::Month { .. } => at.format("%-d %b %Y").to_string(),
    };
    match row.recording.take {
        Some(take) => format!("{when} · {}", short_duration(take.duration)),
        None => when,
    }
}

/// The row title, with a date-based fallback for an untitled recording.
pub fn title<Tz: TimeZone>(row: &RecordingRow, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    match row.recording.title.as_deref().map(str::trim) {
        Some(title) if !title.is_empty() => title.to_owned(),
        _ => format!(
            "Recording {}",
            local(row.recording.created_at, tz).format("%-d %b, %H:%M")
        ),
    }
}
