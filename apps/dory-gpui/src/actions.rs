//! Every P4 command, in one list. `menus.rs` binds keys and builds the menu bar from these,
//! `shell.rs` handles each one at the window root so it fires whatever has focus, and views
//! dispatch them instead of declaring their own. A later phase adds its commands here.

use dory_core::domain::{LangTag, LanguagePref};

gpui_kit::actions!(
    dory,
    [
        Quit,
        NewRecording,
        Find,
        SelectNext,
        SelectPrevious,
        ShowSummary,
        ShowTranscript,
        TogglePlayback,
        CopySummary,
        CopyTranscript,
        ExportMarkdown,
        ExportText,
        ExportSrt,
        MoveToTrash,
        UndoTrash,
        PauseRecording,
        StopRecording,
        UndoStop,
        ShowProcessingQueue,
        Minimize,
        Zoom,
        ShowHelp,
        FilterAll,
        FilterProcessing,
        FilterNeedsAttention,
        FilterDone,
        OpenSettings,
    ]
);

/// **Re-transcribe in…** (UX-26) for the selected recording. `None` is Automatic; otherwise a
/// language tag such as `fr`.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct RetranscribeIn {
    pub language: Option<&'static str>,
}

impl RetranscribeIn {
    pub fn pref(&self) -> LanguagePref {
        match self.language.map(LangTag::parse) {
            Some(Ok(tag)) => LanguagePref::Pinned(tag),
            _ => LanguagePref::Auto,
        }
    }

    /// Whether this is the recording's current choice, for the menu's check mark.
    pub fn picks(&self, current: &LanguagePref) -> bool {
        match (self.pref(), current) {
            (LanguagePref::Auto, LanguagePref::Auto) => true,
            (LanguagePref::Pinned(a), LanguagePref::Pinned(b)) => a.primary() == b.primary(),
            _ => false,
        }
    }
}
