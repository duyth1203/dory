//! Export and copy (UX-4, UX-23).
//!
//! `render` is pure and turns a `Detail` into a document. Markdown is the title, the metadata
//! line, the summary, then the transcript as `**Label** [12:24]` turns. Text is the same without
//! markup. SRT numbers each segment (not each turn) with `00:12:24,000 --> 00:12:27,500`. Names
//! come from `Detail.turns` and the speaker names, so an export always matches what the
//! transcript tab shows. A translated transcript exports the text the tab shows, translated or
//! as spoken, whichever the Original / Translated switch is on.
//!
//! `export` asks for a path with the native save panel and writes off the UI thread. `copy`
//! puts the summary Markdown or the transcript text on the clipboard. A failure becomes a
//! status line through `AppState::say`, never a panic.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local, TimeZone as _};
use dory_core::domain::{Ms, Segment};
use gpui_kit::{App, ClipboardItem, Entity};

use crate::a11y;
use crate::app::{AppState, Detail, TranscriptText, Turn, speaker_label};
use crate::library::{local, short_duration};

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
    text: TranscriptText,
) -> Result<String, ExportBlocked> {
    let at = Local
        .timestamp_millis_opt(detail.recording.created_at.0)
        .earliest()
        .map_or_else(|| FixedOffset::east_opt(0).unwrap(), |t| *t.offset());
    render_in(format, detail, text, &at)
}

pub fn render_in(
    format: ExportFormat,
    detail: &Detail,
    text: TranscriptText,
    tz: &FixedOffset,
) -> Result<String, ExportBlocked> {
    match format {
        ExportFormat::Srt => srt(detail, text),
        ExportFormat::Markdown | ExportFormat::Text => {
            if detail.transcript.is_none() && detail.summary.is_none() {
                return Err(ExportBlocked::NoTranscript);
            }
            Ok(document(format == ExportFormat::Markdown, detail, text, tz))
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

fn document(markdown: bool, detail: &Detail, text: TranscriptText, tz: &FixedOffset) -> String {
    let mut out = String::new();
    let title = title(detail, tz);
    let meta = metadata(detail, tz);
    if markdown {
        let _ = write!(out, "# {title}\n\n{meta}\n");
    } else {
        let _ = write!(out, "{title}\n{meta}\n");
    }
    if let Some(summary) = &detail.summary {
        if markdown {
            let _ = write!(out, "\n## Summary\n\n{}\n", summary.markdown.trim());
        } else {
            let _ = write!(out, "\nSummary\n\n{}\n", plain(&summary.markdown));
        }
    }
    if detail.transcript.is_some() {
        out.push_str(if markdown {
            "\n## Transcript\n"
        } else {
            "\nTranscript\n"
        });
        out.push_str(&turns_text(markdown, detail.shown(text)));
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
        if markdown {
            let _ = write!(out, "\n**{}**{stamp}\n{}\n", turn.label, turn.text);
        } else {
            let _ = write!(out, "\n{}{stamp}\n{}\n", turn.label, turn.text);
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

fn srt(detail: &Detail, text: TranscriptText) -> Result<String, ExportBlocked> {
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
        .filter(|_| detail.translated(text));
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

pub fn export(
    app: &Entity<AppState>,
    detail: Arc<Detail>,
    format: ExportFormat,
    shown: TranscriptText,
    cx: &mut App,
) {
    let text = match render(format, &detail, shown) {
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

/// Does nothing when the recording has no such text.
pub fn copy(detail: &Detail, what: CopyWhat, shown: TranscriptText, cx: &mut App) {
    let text = match what {
        CopyWhat::Summary => detail.summary.as_ref().map(|s| s.markdown.clone()),
        CopyWhat::Transcript => detail
            .transcript
            .as_ref()
            .map(|_| turns_text(false, detail.shown(shown)).trim().to_owned()),
    };
    if let Some(text) = text {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        a11y::announce(match what {
            CopyWhat::Summary => "Summary copied",
            CopyWhat::Transcript => "Transcript copied",
        });
    }
}
