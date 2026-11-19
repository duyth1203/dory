//! Every P4 command, in one list. `menus.rs` binds keys and builds the menu bar from these,
//! `shell.rs` handles each one at the window root so it fires whatever has focus, and views
//! dispatch them instead of declaring their own. A later phase adds its commands here.

use dory_core::domain::{LangTag, LanguagePref};
use dory_core::models::{Llm, ModelId, Stt};

gpui_kit::actions!(
    dory,
    [
        Quit,
        NewRecording,
        Find,
        FindNext,
        FindPrevious,
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
        CloseWindow,
        Minimize,
        Zoom,
        ShowHelp,
        FilterAll,
        FilterProcessing,
        FilterNeedsAttention,
        FilterDone,
        OpenSettings,
        ShowAbout,
        Rename,
        Bigger,
        Smaller,
        ActualSize,
        UseModernTheme,
        UseBookTheme,
        NextRegion,
        PreviousRegion,
    ]
);

/// **Re-transcribe in…** (UX-26) for the selected recording: Automatic or one of the meeting
/// languages.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct RetranscribeIn {
    pub language: LanguagePref,
}

impl RetranscribeIn {
    pub fn pref(&self) -> LanguagePref {
        self.language.clone()
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

/// **Translate Summary…** for the selected recording: its summary in `target`, beside the
/// original.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct TranslateSummaryTo {
    pub target: LangTag,
    /// The summary model for this one translation, `None` for the Settings choice.
    pub llm: Option<ModelId<Llm>>,
}

/// **Re-transcribe with…** for the selected recording: transcribes it again with this speech
/// model, `None` for the Settings choice, without changing Settings.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct RetranscribeWith {
    pub stt: Option<ModelId<Stt>>,
}

/// **Translate with…** for the selected recording: shows its summary and opens **Translate…**
/// to translate with this model, `None` for the Settings choice.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct TranslateWith {
    pub llm: Option<ModelId<Llm>>,
}

/// **Re-summarize with…** for the selected recording: writes its summary again with this model,
/// `None` for the Settings choice, without changing Settings.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct ResummarizeWith {
    pub llm: Option<ModelId<Llm>>,
}

/// What a Processing window row's ⋯ menu does to its job (UX-15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueVerb {
    MoveToTop,
    MoveUp,
    MoveDown,
    Cancel,
}

/// A ⋯ menu item in the Processing window, aimed at one job.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct QueueCommand {
    pub job: u64,
    pub verb: QueueVerb,
}
