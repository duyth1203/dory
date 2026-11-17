//! The native Dory app. `main.rs` opens the core and the window; everything else is here so the
//! tests can drive it with `TestAppContext`.
//!
//! Who owns what, so parallel work never edits the same file:
//! - The shell (P4 part 1): `app` (state, the core boundary, `Session`, `PlayerHandle`),
//!   `library` (pure rules), `theme`, `a11y`, `actions`, `sidebar`, `shell`, `platform`,
//!   `fixtures`, and `main.rs`.
//! - Agent 1: `detail`, `summary_view`.
//! - Agent 2: `transcript_view`, `player_view`.
//! - Agent 3: `export`, `menus`, `prefs`.
//! - P5 record flow: `record_control`, `processing`, and the record and processing parts of the
//!   shell files above. P5's other agents: `notify`, `queue_popover`, `status_item`, and
//!   `set_up_summaries`.
//! - P6 (onboarding and configuration): `onboarding` and the shell files above; a parallel agent
//!   owns `settings_window`, `models_view`, and `set_up_summaries`, wired only through
//!   `settings_window::open`, `models_view::speech_step`, `combobox::meeting_languages`,
//!   and `set_up_summaries::sheet`.
//!
//! - P7 (accessibility): `system_prefs`, and fixes across every view above.
//! - Window glass (`docs/design/settings-icons-blur.md` section 5): `glass`, used by the shell,
//!   the sidebar, and `settings_window`.
//! - The processing view (`docs/design/processing-view.md`): `processing` (the states P1 to P8
//!   and their words), `estimate` (time left), `motion` and `glyphs`, with the card in
//!   `player_view` and the lines below it in `detail`.
//!
//! - Icons (`docs/design/settings-icons-blur.md` section 2): `icons`, the vendored Lucide set every
//!   view draws from, and `about`, which carries its license.
//!
//! Each agent-owned file's doc comment is its contract.

pub mod a11y;
pub mod about;
pub mod actions;
pub mod app;
pub mod combobox;
pub mod detail;
pub mod estimate;
pub mod export;
pub mod fixtures;
pub mod glass;
pub mod glyphs;
pub mod icons;
pub mod languages;
pub mod library;
pub mod menus;
pub mod models_view;
pub mod motion;
pub mod notify;
pub mod onboarding;
pub mod platform;
pub mod player_view;
pub mod prefs;
pub mod processing;
pub mod queue_popover;
pub mod radio;
pub mod record_control;
pub mod set_up_summaries;
pub mod settings_window;
pub mod shell;
pub mod sidebar;
pub mod status_item;
pub mod summary_view;
pub mod system_prefs;
pub mod theme;
pub mod transcript_view;

use gpui_kit::{App, AppContext as _, Entity, Window};

/// Builds the window's root view on `dory`. `main` and the tests share it.
pub fn build(
    dory: dory_core::app::Dory,
    prefs: prefs::Prefs,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<app::AppState>, Entity<shell::Shell>) {
    theme::set_display(|display| display.text_scale = prefs.text_scale, cx);
    let state = cx.new(|cx| app::AppState::new(dory, prefs, cx));
    models_view::install(&state, cx);
    let shell = cx.new(|cx| shell::Shell::new(state.clone(), window, cx));
    (state, shell)
}
