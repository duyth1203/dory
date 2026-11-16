//! Every P4 command, in one list. `menus.rs` binds keys and builds the menu bar from these,
//! `shell.rs` handles each one at the window root so it fires whatever has focus, and views
//! dispatch them instead of declaring their own. A later phase adds its commands here.

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
    ]
);
