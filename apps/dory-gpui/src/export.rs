//! Export and copy (UX-4, UX-23).
//!
//! `render` is pure and turns a `Detail` into a document. Markdown is the title, the metadata
//! line, the summary, then the transcript as `**Label** [12:24]` turns, with the spoken language
//! after the label, `**Label (English)** [12:24]`, on the turns the tab tags. Text is the same
//! without markup. SRT numbers each segment (not each turn) with `00:12:24,000 --> 00:12:27,500`. Names
//! come from `Detail.turns` and the speaker names, so an export always matches what the
//! transcript tab shows. A translated transcript or summary exports the text its tab shows,
//! translated or as written, whichever the Original / Translated switch is on.
//!
//! `export` asks for a path with the native save panel and writes off the UI thread. `copy`
//! puts the summary Markdown or the transcript text on the clipboard. A failure becomes a
//! status line through `AppState::say`, never a panic.
//!
//! [`Has`] is the one rule for what can be copied or exported right now. `copy` and `export`
//! check it, so the buttons, both menus, and the key bindings all reach the same answer, and the
//! menus draw their items from it.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local, TimeZone as _};
use dory_core::domain::{Ms, RecordingId, Segment};
use gpui_kit::{App, Entity};

use crate::a11y;
use crate::app::{AppState, Detail, DetailTab, Shown, Turn, speaker_label};
use crate::library::{local, short_duration};
use crate::processing::Phase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Text,
    Srt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportBlocked {
    NoTranscript,
    /// SRT needs segment times, which transcripts from the old app do not have.
    Untimed,
}

impl ExportBlocked {
    pub fn message(self) -> &'static str {
        match self {
            ExportBlocked::NoTranscript => "There is nothing to export yet.",
            ExportBlocked::Untimed => {
                "This transcript has no times, so it cannot be exported as SRT."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyWhat {
    Summary,
    Transcript,
}

impl CopyWhat {
    /// What `tab`'s own Copy button copies.
    pub fn of(tab: DetailTab) -> CopyWhat {
        match tab {
            DetailTab::Summary => CopyWhat::Summary,
            DetailTab::Transcript => CopyWhat::Transcript,
        }
    }

    /// The [`crate::copied`] key the tab row's Copy button flashes by.
    pub fn key(self) -> &'static str {
        match self {
            CopyWhat::Summary => "copy-summary",
            CopyWhat::Transcript => "copy-transcript",
        }
    }
}

/// What the open recording has to copy or export. Copy and Export act on text that is shown, so
/// they wait for it. While the transcript is on its way the tab shows the placeholder or the lines
/// streaming in, and whatever transcript the store still holds is about to be replaced. A
/// transcript with no words in it is no transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Has {
    pub summary: bool,
    pub transcript: bool,
    /// Every line of the transcript has a time, which SRT needs.
    pub timed: bool,
}

impl Has {
    pub fn of(detail: &Detail, phase: Option<&Phase>) -> Has {
        let arriving = phase.is_some_and(Phase::before_transcript);
        let said = detail
            .transcript
            .as_ref()
            .filter(|transcript| !transcript.segments.is_empty());
        Has {
            summary: detail.summary.is_some(),
            transcript: !arriving && said.is_some(),
            timed: said.is_some_and(|transcript| transcript.is_timed()),
        }
    }

    /// For a recording whose detail has not been read yet, such as a row that was just
    /// right-clicked, from what the library's row says.
    pub fn of_row(row: &dory_core::domain::RecordingRow, phase: Option<&Phase>) -> Has {
        let arriving = phase.is_some_and(Phase::before_transcript);
        Has {
            summary: row.summary != dory_core::domain::SummaryFlag::None,
            transcript: !arriving && row.has_transcript,
            timed: row.has_transcript,
        }
    }

    pub fn can_copy(self, what: CopyWhat) -> bool {
        match what {
            CopyWhat::Summary => self.summary,
            CopyWhat::Transcript => self.transcript,
        }
    }

    /// Markdown and Text write whichever of the summary and the transcript there is. SRT writes
    /// the transcript's times.
    pub fn can_export(self, format: ExportFormat) -> bool {
        match format {
            ExportFormat::Markdown | ExportFormat::Text => self.summary || self.transcript,
            ExportFormat::Srt => self.transcript && self.timed,
        }
    }

    /// The tab's own Copy: the summary on Summary, the transcript on Transcript.
    pub fn copy(self, tab: DetailTab) -> bool {
        self.can_copy(CopyWhat::of(tab))
    }

    /// Export writes the text of the tab it is on. The Summary tab also offers the transcript's
    /// formats, so a recording whose summary failed can still be exported.
    pub fn export(self, tab: DetailTab) -> bool {
        match tab {
            DetailTab::Summary => self.can_export(ExportFormat::Markdown),
            DetailTab::Transcript => self.transcript,
        }
    }
}

/// What recording `id` has to copy or export right now. From its detail when that is the one
/// open, and from its row when the detail is still being read, as after a right-click on a row
/// that was not selected.
pub fn has_of(app: &AppState, id: &RecordingId, cx: &App) -> Option<Has> {
    let phase = app.phase_of(id);
    match app
        .session()
        .and_then(|session| session.read(cx).detail())
        .filter(|detail| &detail.recording.id == id)
    {
        Some(detail) => Some(Has::of(&detail, phase.as_ref())),
        None => app.row(id).map(|row| Has::of_row(row, phase.as_ref())),
    }
}

/// The open recording and what it has to copy or export right now.
pub fn open_recording(app: &AppState, cx: &App) -> Option<(Arc<Detail>, Has)> {
    let detail = app.session()?.read(cx).detail()?;
    let has = Has::of(&detail, app.phase_of(&detail.recording.id).as_ref());
    Some((detail, has))
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Text => "txt",
            ExportFormat::Srt => "srt",
        }
    }
}

/// Renders in the Mac's current time zone, at the recording's own moment so a daylight saving
/// change does not shift the date.
pub fn render(
    format: ExportFormat,
    detail: &Detail,
    shown: impl Into<Shown>,
) -> Result<String, ExportBlocked> {
    let at = Local
        .timestamp_millis_opt(detail.recording.created_at.0)
        .earliest()
        .map_or_else(|| FixedOffset::east_opt(0).unwrap(), |t| *t.offset());
    render_in(format, detail, shown, &at)
}

pub fn render_in(
    format: ExportFormat,
    detail: &Detail,
    shown: impl Into<Shown>,
    tz: &FixedOffset,
) -> Result<String, ExportBlocked> {
    let shown = shown.into();
    match format {
        ExportFormat::Srt => srt(detail, shown),
        ExportFormat::Markdown | ExportFormat::Text => {
            if detail.transcript.is_none() && detail.summary.is_none() {
                return Err(ExportBlocked::NoTranscript);
            }
            Ok(document(
                format == ExportFormat::Markdown,
                detail,
                shown,
                tz,
            ))
        }
    }
}

fn title(detail: &Detail, tz: &FixedOffset) -> String {
    match detail.recording.title.as_deref().map(str::trim) {
        Some(title) if !title.is_empty() => title.to_owned(),
        _ => format!(
            "Recording {}",
            local(detail.recording.created_at, tz).format("%-d %b, %H:%M")
        ),
    }
}

fn metadata(detail: &Detail, tz: &FixedOffset) -> String {
    let at: DateTime<FixedOffset> = local(detail.recording.created_at, tz);
    let mut parts = vec![at.format("%-d %B %Y, %H:%M").to_string()];
    if let Some(take) = detail.recording.take {
        parts.push(short_duration(take.duration));
    }
    let speakers = detail.transcript.as_ref().map_or(0, |t| t.speakers().len());
    if speakers > 0 {
        parts.push(if speakers == 1 {
            "1 speaker".to_owned()
        } else {
            format!("{speakers} speakers")
        });
    }
    parts.join(" · ")
}

fn document(markdown: bool, detail: &Detail, shown: Shown, tz: &FixedOffset) -> String {
    let mut out = String::new();
    let title = title(detail, tz);
    let meta = metadata(detail, tz);
    if markdown {
        let _ = write!(out, "# {title}\n\n{meta}\n");
    } else {
        let _ = write!(out, "{title}\n{meta}\n");
    }
    if let Some(summary) = detail.shown_summary(shown.summary) {
        if markdown {
            let _ = write!(out, "\n## Summary\n\n{}\n", summary.trim());
        } else {
            let _ = write!(out, "\nSummary\n\n{}\n", plain(summary));
        }
    }
    if detail.transcript.is_some() {
        out.push_str(if markdown {
            "\n## Transcript\n"
        } else {
            "\nTranscript\n"
        });
        out.push_str(&turns_text(markdown, detail.shown(shown.transcript)));
    }
    out
}

fn turns_text(markdown: bool, turns: &[Turn]) -> String {
    let mut out = String::new();
    for turn in turns {
        let stamp = turn
            .range
            .map(|range| format!(" [{}]", a11y::clock(range.start())))
            .unwrap_or_default();
        let who = match &turn.tag {
            Some(tag) => format!("{} ({tag})", turn.label),
            None => turn.label.to_string(),
        };
        if markdown {
            let _ = write!(out, "\n**{who}**{stamp}\n{}\n", turn.text);
        } else {
            let _ = write!(out, "\n{who}{stamp}\n{}\n", turn.text);
        }
    }
    out
}

/// The summary without Markdown marks: headings and paragraphs as lines, list items as "- ".
fn plain(markdown: &str) -> String {
    use gpui_kit::Role;
    let mut out = String::new();
    for block in a11y::markdown_blocks(markdown) {
        match block.role {
            Role::ListItem => {
                let _ = writeln!(out, "- {}", block.text);
            }
            Role::Heading => {
                let _ = write!(out, "\n{}\n", block.text);
            }
            _ => {
                let _ = writeln!(out, "{}", block.text);
            }
        }
    }
    out.trim().to_owned()
}

fn srt(detail: &Detail, shown: Shown) -> Result<String, ExportBlocked> {
    let Some(transcript) = detail
        .transcript
        .as_ref()
        .filter(|t| !t.segments.is_empty())
    else {
        return Err(ExportBlocked::NoTranscript);
    };
    if !transcript.is_timed() {
        return Err(ExportBlocked::Untimed);
    }
    let translation = detail
        .translation
        .as_ref()
        .filter(|_| detail.translated(shown.transcript));
    let mut out = String::new();
    for (ix, segment) in transcript.segments.iter().enumerate() {
        let Segment {
            range,
            speaker,
            channel,
            ..
        } = segment;
        let text = translation.map_or(segment.text.as_str(), |t| t.shown(ix, segment));
        let Some(range) = range else { continue };
        let label = speaker_label(*speaker, *channel, transcript, &detail.names);
        let _ = write!(
            out,
            "{}\n{} --> {}\n{label}: {}\n\n",
            ix + 1,
            srt_time(range.start()),
            srt_time(range.end()),
            text.trim(),
        );
    }
    Ok(out)
}

/// `HH:MM:SS,mmm`.
pub fn srt_time(at: Ms) -> String {
    let ms = at.0;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms % 3_600_000 / 60_000,
        ms % 60_000 / 1000,
        ms % 1000
    )
}

/// What the save panel offers as the file name.
pub fn suggested_name(detail: &Detail, format: ExportFormat) -> String {
    let stem = detail
        .recording
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| detail.recording.id.to_string(), safe_stem);
    format!("{stem}.{}", format.extension())
}

fn safe_stem(title: &str) -> String {
    title.replace(['/', ':', '\\'], "-")
}

/// Renders what `has` says is there to export. A transcript that is not shown is left out, so
/// the old one is not written while a new one is on its way.
pub fn render_open(
    format: ExportFormat,
    detail: &Detail,
    has: Has,
    shown: Shown,
) -> Result<String, ExportBlocked> {
    if !has.can_export(format) {
        return Err(if format == ExportFormat::Srt && has.transcript {
            ExportBlocked::Untimed
        } else {
            ExportBlocked::NoTranscript
        });
    }
    if has.transcript || detail.transcript.is_none() {
        render(format, detail, shown)
    } else {
        let summary_only = Detail {
            transcript: None,
            ..detail.clone()
        };
        render(format, &summary_only, shown)
    }
}

pub fn export(
    app: &Entity<AppState>,
    detail: Arc<Detail>,
    has: Has,
    format: ExportFormat,
    shown: Shown,
    cx: &mut App,
) {
    let text = match render_open(format, &detail, has, shown) {
        Ok(text) => text,
        Err(blocked) => {
            app.update(cx, |app, cx| app.say(blocked.message(), cx));
            return;
        }
    };
    let name = suggested_name(&detail, format);
    let directory = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Documents"))
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let picked = cx.prompt_for_new_path(&directory, Some(&name));
    let app = app.clone();
    cx.spawn(async move |cx| {
        let path = match picked.await {
            Ok(Ok(Some(path))) => path,
            Ok(Ok(None)) | Err(_) => return,
            Ok(Err(error)) => {
                app.update(cx, |app, cx| {
                    app.say(format!("Could not open the save panel: {error}"), cx)
                });
                return;
            }
        };
        let shown = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let write = cx
            .background_executor()
            .spawn(async move { std::fs::write(&path, text) });
        let message = match write.await {
            Ok(()) => format!("Exported {shown}."),
            Err(error) => format!("Could not write {shown}: {error}"),
        };
        app.update(cx, |app, cx| app.say(message, cx));
    })
    .detach();
}

/// Does nothing when `has` says the recording has no such text to show.
pub fn copy(detail: &Detail, has: Has, what: CopyWhat, shown: Shown, cx: &mut App) {
    if !has.can_copy(what) {
        return;
    }
    let text = match what {
        CopyWhat::Summary => detail.shown_summary(shown.summary).map(str::to_owned),
        CopyWhat::Transcript => detail.transcript.as_ref().map(|_| {
            turns_text(false, detail.shown(shown.transcript))
                .trim()
                .to_owned()
        }),
    };
    if let Some(text) = text {
        let said = match what {
            CopyWhat::Summary => "Summary copied",
            CopyWhat::Transcript => "Transcript copied",
        };
        crate::copied::copy(what.key(), text, said, cx);
    }
}
