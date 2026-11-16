//! Export and copy (UX-4, UX-23). Stub for agent 3, who owns this file together with
//! `menus.rs` and `prefs.rs`.
//!
//! Agent 3 builds, without editing any other file:
//! - `render`: pure, tested without a window. Markdown is the title, the metadata line, the
//!   summary, then the transcript as `**Label** [12:24]` turns. Text is the same without
//!   markup. SRT numbers each segment (not each turn) with `00:12:24,000 --> 00:12:27,500`
//!   from `Segment.range`, and returns `ExportBlocked::Untimed` when `Transcript::is_timed`
//!   is false (legacy transcripts) and `NoTranscript` when there is none.
//! - `export`: `cx.prompt_for_new_path` with `suggested_name`, then write off the UI thread.
//!   Failures surface as a notification, never a panic.
//! - `copy`: the summary Markdown or the transcript text to the clipboard.
//!
//! The shell calls `export` and `copy` from the window-root handlers of the `Export*` and
//! `Copy*` actions, with the selected session's `Detail`. Names come from `Detail.turns`, so an
//! export always matches what the transcript tab shows.

use std::sync::Arc;

use gpui_kit::{App, Window};

use crate::app::Detail;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyWhat {
    Summary,
    Transcript,
}

pub fn render(_format: ExportFormat, _detail: &Detail) -> Result<String, ExportBlocked> {
    Err(ExportBlocked::NoTranscript)
}

pub fn suggested_name(detail: &Detail, format: ExportFormat) -> String {
    let stem = detail
        .recording
        .title
        .clone()
        .unwrap_or_else(|| detail.recording.id.to_string());
    let ext = match format {
        ExportFormat::Markdown => "md",
        ExportFormat::Text => "txt",
        ExportFormat::Srt => "srt",
    };
    format!("{stem}.{ext}")
}

pub fn export(_detail: Arc<Detail>, format: ExportFormat, _window: &mut Window, _cx: &mut App) {
    eprintln!("export {format:?}: agent 3 wires this");
}

pub fn copy(_detail: &Detail, what: CopyWhat, _cx: &mut App) {
    eprintln!("copy {what:?}: agent 3 wires this");
}
