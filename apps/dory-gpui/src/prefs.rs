//! Window state that survives a relaunch (UX-24). Stub for agent 3, who owns this file together
//! with `export.rs` and `menus.rs`.
//!
//! Agent 3 builds, without editing any other file:
//! - `load` and the saving side of `watch`: a versioned JSON file at
//!   `<config.data_dir>/ui-prefs.json`, so `DORY_DATA_DIR` isolates it in tests and lanes. The
//!   crate already depends on `serde` and `serde_json` for this. A missing or unreadable file
//!   gives `Prefs::default()`, never an error.
//! - `watch`: subscribe to `AppState`'s `AppEvent::SelectionChanged` and the window's bounds
//!   changes, update `prefs` through `app.update`, and save with a short debounce, plus once
//!   more on quit (`cx.on_app_quit`) with `quit_at` set.
//! - Selection restore: on the first `AppEvent::RowsReloaded`, call
//!   `restore_selection(&prefs, now)` and `AppState::select` with it. The proposed rule restores
//!   only if the app quit less than 4 hours ago (handoff section 7.2), else the start screen.
//! - `window_size`: the shell passes it to `WindowOptions` at launch.
//! - `sidebar_resize_handle`: a drag handle on the sidebar's trailing edge that clamps to
//!   `SIDEBAR_WIDTH` and writes `prefs.sidebar_width`. The shell already places it and already
//!   reads `prefs.sidebar_width`.
//!
//! The shell reads only the fields below. Adding a field is free; renaming one breaks the shell.

use std::ops::RangeInclusive;
use std::time::Duration;

use dory_core::domain::{RecordingId, Timestamp};
use gpui_kit::{AnyElement, App, Entity, IntoElement as _, Window, div};

use crate::app::AppState;
use crate::library::Selection;

pub const SIDEBAR_WIDTH: RangeInclusive<f32> = 240.0..=380.0;
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 300.0;
pub const DEFAULT_WINDOW_SIZE: (f32, f32) = (1100.0, 720.0);
/// UX-24's proposed rule: restore the selection only after a short absence.
pub const RESTORE_WITHIN: Duration = Duration::from_secs(4 * 3600);

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Points, width then height.
    pub window_size: (f32, f32),
    /// Always within [`SIDEBAR_WIDTH`].
    pub sidebar_width: f32,
    pub last_selection: Option<RecordingId>,
    pub quit_at: Option<Timestamp>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            window_size: DEFAULT_WINDOW_SIZE,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            last_selection: None,
            quit_at: None,
        }
    }
}

pub fn load(_config: &dory::Config) -> Prefs {
    Prefs::default()
}

/// Called once by `main` after the window opens.
pub fn watch(_app: &Entity<AppState>, _config: &dory::Config, _window: &mut Window, _cx: &mut App) {
}

pub fn restore_selection(_prefs: &Prefs, _now: Timestamp) -> Selection {
    Selection::None
}

pub fn sidebar_resize_handle(_app: &Entity<AppState>, _cx: &mut App) -> AnyElement {
    div().into_any_element()
}
