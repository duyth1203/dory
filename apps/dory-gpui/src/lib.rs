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
//!   shell files above. P5's other agents: `notify`, `processing_window`, `status_item`, and
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
//! - The visual pass: `show`, the launch state `scripts/visual-pass.sh` opens each view with.
//!
//! - Quit and the window's lifetime (UX-8): `in_progress`, `quit`, and the main-window functions
//!   below. The state outlives the main window, which closes without quitting.
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
pub mod find;
pub mod fixtures;
pub mod glass;
pub mod glyphs;
pub mod icons;
pub mod in_progress;
pub mod languages;
pub mod library;
pub mod marks;
pub mod menus;
pub mod models_view;
pub mod motion;
pub mod notify;
pub mod onboarding;
pub mod platform;
pub mod player_view;
pub mod prefs;
pub mod processing;
pub mod processing_queue;
pub mod processing_window;
pub mod quit;
pub mod radio;
pub mod record_control;
pub mod set_up_summaries;
pub mod settings_window;
pub mod shell;
pub mod show;
pub mod sidebar;
pub mod status_item;
pub mod summary_view;
pub mod surface;
pub mod system_prefs;
pub mod theme;
pub mod transcript_view;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, Global, TitlebarOptions, Window,
    WindowBounds, WindowOptions, point, px, size,
};

/// Gives a library with no meeting languages the system's, as onboarding would. A library
/// that skipped onboarding, such as an import, never had them chosen. `main` calls it once
/// before any window opens, so the list is set before the first frame reads it.
pub fn seed_meeting_languages(
    dory: &dory_core::app::Dory,
    language: Option<dory_core::domain::LangTag>,
) -> Result<(), dory_core::app::DoryError> {
    let Some(language) = language else {
        return Ok(());
    };
    dory.update_settings(move |settings| {
        if settings.meeting_languages.is_empty() {
            settings.meeting_languages = vec![language];
        }
    })
}

/// Builds the window's root view on `dory`. The tests that need only one window use it.
pub fn build(
    dory: dory_core::app::Dory,
    prefs: prefs::Prefs,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<app::AppState>, Entity<shell::Shell>) {
    let state = new_state(dory, prefs, cx);
    let shell = cx.new(|cx| shell::Shell::new(state.clone(), window, cx));
    cx.global_mut::<Main>().window = Some(window.window_handle());
    (state, shell)
}

/// The app's state on `dory`, which outlives any window: closing the main window keeps Dory
/// running in the menu bar (UX-8).
pub fn new_state(
    dory: dory_core::app::Dory,
    prefs: prefs::Prefs,
    cx: &mut App,
) -> Entity<app::AppState> {
    theme::set_display(|display| display.text_scale = prefs.text_scale, cx);
    let state = cx.new(|cx| app::AppState::new(dory, prefs, cx));
    models_view::install(&state, cx);
    cx.set_global(Main {
        app: state.clone(),
        window: None,
    });
    state
}

/// Holds `AppState` for the life of the process, and the main window while it is open.
struct Main {
    app: Entity<app::AppState>,
    window: Option<AnyWindowHandle>,
}

impl Global for Main {}

pub fn main_window_options(prefs: &prefs::Prefs, cx: &App) -> WindowOptions {
    let (width, height) = prefs.window_size;
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Dory".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(18.), px(18.))),
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(width), px(height)),
            cx,
        ))),
        window_background: glass::window_background(cx),
        window_min_size: Some(size(
            px(prefs::MIN_WINDOW_SIZE.0),
            px(prefs::MIN_WINDOW_SIZE.1),
        )),
        ..Default::default()
    }
}

/// Opens the main window on the state [`new_state`] made.
pub fn open_main_window(cx: &mut App) -> gpui_kit::Result<AnyWindowHandle> {
    let app = cx.global::<Main>().app.clone();
    let options = main_window_options(&app.read(cx).prefs, cx);
    let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
        let shell = cx.new(|cx| shell::Shell::new(app.clone(), window, cx));
        glass::follow(window, cx);
        platform::print_window_id(window);
        prefs::follow_window(&app, window, cx);
        shell
    })?;
    cx.global_mut::<Main>().window = Some(handle);
    Ok(handle)
}

/// The main window while it is open.
pub fn main_window(cx: &App) -> Option<AnyWindowHandle> {
    let window = cx.try_global::<Main>()?.window?;
    cx.windows().contains(&window).then_some(window)
}

/// Brings the main window forward, reopening it on the same state when it was closed: "Show
/// Dory", a click on the Dock icon or on a notification, and a quit that needs to ask.
pub fn show_main_window(cx: &mut App) -> Option<AnyWindowHandle> {
    cx.try_global::<Main>()?;
    let window = match main_window(cx) {
        Some(window) => window,
        None => match open_main_window(cx) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("dory: could not open the window: {error}");
                return None;
            }
        },
    };
    cx.activate(true);
    let _ = window.update(cx, |_, window, _| window.activate_window());
    Some(window)
}
