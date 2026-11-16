//! The menu bar and key bindings for P4's commands (UX-21), and the recording context menu.
//! Stub for agent 3, who owns this file together with `export.rs` and `prefs.rs`.
//!
//! Agent 3 builds, without editing any other file:
//! - `install`: `cx.set_menus` with Dory, File, Edit, View, Recording, Window, and Help, every
//!   item dispatching an action from `crate::actions` with the shortcut from the table in
//!   `docs/plans/ux-redesign.md`, and `cx.bind_keys` for the same shortcuts. Space for
//!   `TogglePlayback` binds with a context that excludes text inputs, so it still types a space.
//!   The minimal version below only quits and binds what the shell needs to be usable.
//! - `recording_menu`: the row context menu and the detail pane's "⋯" menu (handoff section 4):
//!   Rename, Copy Summary, Copy Transcript, Export, Show in Finder, Move to Trash. Every item
//!   is an action; Move to Trash waits for a core Trash API (UX-22), so leave it disabled.
//!
//! The shell calls `install` once from `main` and `recording_menu` from each sidebar row.

use dory_core::domain::RecordingId;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{App, KeyBinding, Menu, MenuItem, Window};

use crate::actions::{
    CopySummary, CopyTranscript, ExportMarkdown, Find, NewRecording, Quit, ShowSummary,
    ShowTranscript,
};

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-r", NewRecording, None),
        KeyBinding::new("cmd-f", Find, None),
        KeyBinding::new("cmd-1", ShowSummary, None),
        KeyBinding::new("cmd-2", ShowTranscript, None),
        KeyBinding::new("cmd-e", ExportMarkdown, None),
        KeyBinding::new("cmd-shift-c", CopySummary, None),
    ]);
    cx.set_menus([Menu::new("Dory").items([MenuItem::action("Quit Dory", Quit)])]);
}

pub fn recording_menu(
    menu: PopupMenu,
    _id: &RecordingId,
    _window: &mut Window,
    _cx: &mut App,
) -> PopupMenu {
    menu.menu("Copy Summary", Box::new(CopySummary))
        .menu("Copy Transcript", Box::new(CopyTranscript))
        .menu("Export…", Box::new(ExportMarkdown))
}
