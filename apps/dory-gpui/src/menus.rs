//! The menu bar and key bindings for P4's commands (UX-21), and the recording context menu.
//!
//! `install` binds the shortcuts of the table in `docs/plans/ux-redesign.md` and sets the menus
//! once. `watch` rebuilds them when what the commands need changes: a recording is selected, its
//! detail has a summary or a transcript, SRT is possible, or there is something to undo. Every
//! item dispatches an action from `crate::actions`, and the window root handles it, so a menu
//! item, a shortcut, and a button all take one path. The Recording menu follows the recorder:
//! New Recording while idle, Pause or Resume and Stop while a take is live, Undo Stop in grace.
//! Its **Re-transcribe in…** submenu and the row menu's offer Automatic and the meeting languages,
//! by `languages::choices` (UX-26), computed when the menu is built or opened.
//!
//! Re-transcribe in… and Translate Summary… grey out while a job for the recording is queued or
//! running, by the core's rule (`AppState::is_underway`), and say why in a dimmed line above
//! them ([`WAITS`]). Copy, Export, Rename, Move to Trash, and Play stay: none races the job.
//!
//! Edit > Find holds Find, Find Next, and Find Previous. The last two step through the matches of
//! the active search in the selected recording and are off when there are none (UX-32).
//!
//! Space for `TogglePlayback` is not bound here. A binding without a context would also take
//! Space from a focused button and from the search field, so the player owns that key.

use std::cell::Cell;
use std::rc::Rc;

use dory_core::app::RecorderStatus;
use dory_core::domain::{LangTag, LanguagePref, RecordingId};
use gpui_kit::component::input;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{
    App, Context, Entity, Global, KeyBinding, Menu, MenuItem, NoAction, OsAction, SystemMenuType,
    Window,
};

use crate::actions::*;
use crate::app::AppState;
use crate::export::{CopyWhat, ExportFormat, Has};
use crate::languages;
use crate::library::Selection;
use crate::theme::ThemeName;

/// What the commands need to know to be enabled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    pub has_selection: bool,
    pub has_summary: bool,
    pub has_transcript: bool,
    /// Markdown and Text export the summary or the transcript, whichever there is.
    pub export_text: bool,
    /// The transcript has times for every segment, which SRT needs.
    pub srt: bool,
    pub can_undo: bool,
    /// A search has matches in the selected recording, which Find Next and Find Previous step
    /// through.
    pub can_step: bool,
    pub recorder: Recorder,
    pub text_scale: Scale,
    /// View › Theme: the theme shown.
    pub theme: ThemeName,
    /// The meeting languages, which Re-transcribe in… offers.
    pub languages: Vec<LangTag>,
    /// The language the selected recording's summary is written in, which Translate Summary…
    /// leaves out.
    pub summary_language: Option<LangTag>,
    /// A job for the selected recording is queued or running, so Re-transcribe in… and Translate
    /// Summary… wait for it.
    pub underway: bool,
}

/// Why Re-transcribe in… and Translate Summary… are greyed out while the recording is processed.
pub const WAITS: &str = "Available when processing finishes";

/// Where the text scale sits, so View > Bigger and Smaller grey out at the ends (A11Y-7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scale {
    Smallest,
    Smaller,
    #[default]
    Actual,
    Bigger,
    Largest,
}

impl Scale {
    pub fn of(scale: f32) -> Scale {
        let (min, max) = (crate::theme::TEXT_SCALES[0], crate::theme::TEXT_SCALES[6]);
        if scale <= min + 0.001 {
            Scale::Smallest
        } else if scale >= max - 0.001 {
            Scale::Largest
        } else if (scale - 1.0).abs() < 0.001 {
            Scale::Actual
        } else if scale < 1.0 {
            Scale::Smaller
        } else {
            Scale::Bigger
        }
    }
}

/// The recorder phase the Recording menu needs, without the take or its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Recorder {
    #[default]
    Idle,
    Recording,
    Paused,
    Grace,
}

impl Recorder {
    pub fn of(status: &RecorderStatus) -> Recorder {
        match status {
            RecorderStatus::Idle => Recorder::Idle,
            RecorderStatus::Recording { .. } => Recorder::Recording,
            RecorderStatus::Paused { .. } => Recorder::Paused,
            RecorderStatus::Grace { .. } => Recorder::Grace,
        }
    }
}

pub fn menu_state(app: &AppState, cx: &App) -> MenuState {
    let detail = app.session().and_then(|session| session.read(cx).detail());
    let phase = app.selection.id().and_then(|id| app.phase_of(id));
    let has = detail.as_ref().map(|d| Has::of(d, phase.as_ref()));
    MenuState {
        has_selection: app.selection != Selection::None,
        has_summary: has.is_some_and(|h| h.can_copy(CopyWhat::Summary)),
        has_transcript: has.is_some_and(|h| h.can_copy(CopyWhat::Transcript)),
        export_text: has.is_some_and(|h| h.can_export(ExportFormat::Markdown)),
        srt: has.is_some_and(|h| h.can_export(ExportFormat::Srt)),
        can_undo: app.can_undo(),
        can_step: app
            .session()
            .is_some_and(|session| session.read(cx).found().is_some()),
        recorder: Recorder::of(app.recorder()),
        text_scale: Scale::of(app.prefs.text_scale),
        theme: app.prefs.theme,
        languages: app.settings().meeting_languages.clone(),
        summary_language: detail
            .as_ref()
            .and_then(|d| d.summary.as_ref())
            .and_then(|s| s.language.clone()),
        underway: app.selection.id().is_some_and(|id| app.is_underway(id)),
    }
}

pub fn build(state: MenuState) -> Vec<Menu> {
    let exportable = state.export_text;
    let undo = if state.can_undo {
        "Undo Move to Trash"
    } else {
        "Undo"
    };
    let idle = state.recorder == Recorder::Idle;
    let live = matches!(state.recorder, Recorder::Recording | Recorder::Paused);
    let pause = if state.recorder == Recorder::Paused {
        "Resume Recording"
    } else {
        "Pause Recording"
    };
    vec![
        Menu::new("Dory").items([
            MenuItem::action("About Dory", ShowAbout),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Dory", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Recording", NewRecording).disabled(!idle),
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::separator(),
            MenuItem::action("Export as Markdown…", ExportMarkdown).disabled(!exportable),
            MenuItem::action("Export as Text…", ExportText).disabled(!exportable),
            MenuItem::action("Export as SRT…", ExportSrt).disabled(!state.srt),
            MenuItem::separator(),
            MenuItem::action("Copy Summary", CopySummary).disabled(!state.has_summary),
            MenuItem::action("Copy Transcript", CopyTranscript).disabled(!state.has_transcript),
            MenuItem::separator(),
            MenuItem::action("Rename", Rename).disabled(!state.has_selection),
            MenuItem::action("Move to Trash", MoveToTrash).disabled(!state.has_selection),
        ]),
        Menu::new("Edit").items([
            MenuItem::action(undo, UndoTrash).disabled(!state.can_undo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Find").items([
                MenuItem::action("Find", Find),
                MenuItem::action("Find Next", FindNext).disabled(!state.can_step),
                MenuItem::action("Find Previous", FindPrevious).disabled(!state.can_step),
            ])),
        ]),
        Menu::new("View").items([
            MenuItem::action("Show Summary", ShowSummary).disabled(!state.has_selection),
            MenuItem::action("Show Transcript", ShowTranscript).disabled(!state.has_selection),
            MenuItem::separator(),
            MenuItem::action("Show All Recordings", FilterAll),
            MenuItem::action("Show Processing", FilterProcessing),
            MenuItem::action("Show Needs Attention", FilterNeedsAttention),
            MenuItem::action("Show Done", FilterDone),
            MenuItem::separator(),
            MenuItem::action("Bigger", Bigger).disabled(state.text_scale >= Scale::Largest),
            MenuItem::action("Smaller", Smaller).disabled(state.text_scale <= Scale::Smallest),
            MenuItem::action("Actual Size", ActualSize).disabled(state.text_scale == Scale::Actual),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Theme").items(theme_items(&state))),
        ]),
        Menu::new("Recording").items([
            MenuItem::action("New Recording", NewRecording).disabled(!idle),
            MenuItem::action(pause, PauseRecording).disabled(!live),
            MenuItem::action("Stop Recording", StopRecording).disabled(!live),
            MenuItem::action("Undo Stop", UndoStop).disabled(state.recorder != Recorder::Grace),
            MenuItem::separator(),
            MenuItem::submenu(
                Menu::new("Re-transcribe in").items(
                    waits(state.underway).chain(
                        retranscribe_choices(&state.languages).into_iter().map(
                            |choice| match choice {
                                Some((name, action)) => MenuItem::action(name, action)
                                    .disabled(!state.has_selection || state.underway),
                                None => MenuItem::separator(),
                            },
                        ),
                    ),
                ),
            ),
            MenuItem::submenu(
                Menu::new("Translate Summary…").items(
                    waits(state.underway).chain(
                        translate_choices(&state.languages, state.summary_language.as_ref())
                            .into_iter()
                            .map(|choice| match choice {
                                Some((name, action)) => MenuItem::action(name, action)
                                    .disabled(!state.has_summary || state.underway),
                                None => MenuItem::separator(),
                            }),
                    ),
                ),
            ),
            MenuItem::action("Play or Pause", TogglePlayback).disabled(!state.has_selection),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Processing Queue", ShowProcessingQueue),
        ]),
        Menu::new("Help").items([MenuItem::action("Dory Help", ShowHelp).disabled(true)]),
    ]
}

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-shift-r", NewRecording, None),
        KeyBinding::new("cmd-shift-p", PauseRecording, None),
        KeyBinding::new("cmd-.", StopRecording, None),
        KeyBinding::new("cmd-f", Find, None),
        KeyBinding::new("cmd-g", FindNext, None),
        KeyBinding::new("cmd-shift-g", FindPrevious, None),
        KeyBinding::new("cmd-1", ShowSummary, None),
        KeyBinding::new("cmd-2", ShowTranscript, None),
        KeyBinding::new("cmd-e", ExportMarkdown, None),
        KeyBinding::new("cmd-shift-c", CopySummary, None),
        KeyBinding::new("cmd-backspace", MoveToTrash, None),
        KeyBinding::new("cmd-z", UndoTrash, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-=", Bigger, None),
        KeyBinding::new("cmd-+", Bigger, None),
        KeyBinding::new("cmd--", Smaller, None),
        KeyBinding::new("cmd-0", ActualSize, None),
        KeyBinding::new("f6", NextRegion, None),
        KeyBinding::new("shift-f6", PreviousRegion, None),
    ]);
    crate::processing_window::install(cx);
    cx.on_action(|_: &ShowAbout, cx| crate::about::open(cx));
    // Closing the main window keeps Dory running in the menu bar (UX-8).
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.zoom_window());
        }
    });
    cx.set_menus(build(MenuState::default()));
}

/// The app the context menu acts on, held so a right-click can select its row first.
struct Target(Entity<AppState>);

impl Global for Target {}

/// The dimmed [`WAITS`] line and a separator, heading a submenu whose items wait for the job.
fn waits(underway: bool) -> impl Iterator<Item = MenuItem> {
    underway
        .then(|| {
            [
                MenuItem::action(WAITS, NoAction).disabled(true),
                MenuItem::separator(),
            ]
        })
        .into_iter()
        .flatten()
}

/// View › Theme: Modern and Book, the shown one checked.
fn theme_items(state: &MenuState) -> Vec<MenuItem> {
    vec![
        MenuItem::action("Modern", UseModernTheme).checked(state.theme == ThemeName::Modern),
        MenuItem::action("Book", UseBookTheme).checked(state.theme == ThemeName::Book),
    ]
}

/// Keeps the menus in step with `app`. Called once by `main`, before the window opens.
pub fn watch(app: &Entity<AppState>, cx: &mut App) {
    cx.set_global(Target(app.clone()));
    // Global rather than at the window root, so Command-Comma also works from the Settings window.
    cx.on_action(|_: &OpenSettings, cx| {
        let app = cx.global::<Target>().0.clone();
        crate::settings_window::open(&app, cx);
    });
    // Global too: text size applies to every window, Settings included (A11Y-7).
    let scale = |next: fn(f32) -> f32| {
        move |cx: &mut App| {
            let app = cx.global::<Target>().0.clone();
            app.update(cx, |app, cx| {
                let scale = next(app.prefs.text_scale);
                app.set_text_scale(scale, cx)
            });
        }
    };
    let bigger = scale(crate::theme::bigger);
    let smaller = scale(crate::theme::smaller);
    let actual = scale(|_| 1.0);
    cx.on_action(move |_: &Bigger, cx| bigger(cx));
    cx.on_action(move |_: &Smaller, cx| smaller(cx));
    cx.on_action(move |_: &ActualSize, cx| actual(cx));
    let choose = |name: ThemeName| {
        move |cx: &mut App| {
            let app = cx.global::<Target>().0.clone();
            app.update(cx, |app, cx| app.set_theme(name, cx));
        }
    };
    let (modern, book) = (choose(ThemeName::Modern), choose(ThemeName::Book));
    cx.on_action(move |_: &UseModernTheme, cx| modern(cx));
    cx.on_action(move |_: &UseBookTheme, cx| book(cx));
    let shown = Rc::new(Cell::new(MenuState::default()));
    let refresh = {
        let app = app.clone();
        let shown = shown.clone();
        Rc::new(move |cx: &mut App| {
            let state = menu_state(app.read(cx), cx);
            if shown.replace(state.clone()) != state {
                cx.set_menus(build(state));
            }
        })
    };
    let session = Rc::new(Cell::new(None));
    cx.observe(app, {
        let refresh = refresh.clone();
        move |app, cx| {
            let current = app.read(cx).session().cloned();
            let id = current.as_ref().map(|s| s.entity_id());
            if session.replace(id) != id
                && let Some(current) = current
            {
                let refresh = refresh.clone();
                cx.observe(&current, move |_, cx| refresh(cx)).detach();
            }
            refresh(cx);
        }
    })
    .detach();
}

/// The row context menu and the detail pane's "⋯" menu. Right-clicking a row selects it first,
/// as a Mac source list does, so every item acts on the row the menu came from.
pub fn recording_menu(
    menu: PopupMenu,
    id: &RecordingId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let target = cx.try_global::<Target>().map(|t| t.0.clone());
    if let Some(app) = &target {
        app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
    }
    let meeting = target
        .as_ref()
        .map(|app| app.read(cx).settings().meeting_languages.clone())
        .unwrap_or_default();
    // Copy and Export wait for text as the File menu's do, by the same rule.
    let has = target
        .as_ref()
        .and_then(|app| crate::export::has_of(app.read(cx), id, cx))
        .unwrap_or(Has {
            summary: false,
            transcript: false,
            timed: false,
        });
    let underway = target
        .as_ref()
        .is_some_and(|app| app.read(cx).is_underway(id));
    let current = target.and_then(|app| {
        let session = app.read(cx).session()?.read(cx).detail()?;
        Some(session.recording.language.clone())
    });
    let menu = menu
        .menu_with_disabled(
            "Copy Summary",
            Box::new(CopySummary),
            !has.can_copy(CopyWhat::Summary),
        )
        .menu_with_disabled(
            "Copy Transcript",
            Box::new(CopyTranscript),
            !has.can_copy(CopyWhat::Transcript),
        )
        .separator()
        .menu_with_disabled(
            "Export as Markdown…",
            Box::new(ExportMarkdown),
            !has.can_export(ExportFormat::Markdown),
        )
        .menu_with_disabled(
            "Export as Text…",
            Box::new(ExportText),
            !has.can_export(ExportFormat::Text),
        )
        .menu_with_disabled(
            "Export as SRT…",
            Box::new(ExportSrt),
            !has.can_export(ExportFormat::Srt),
        )
        .separator();
    // The kit cannot dim a submenu, so while the job runs the entry is a dimmed item that opens
    // nothing, with the reason under it.
    let menu = if underway {
        menu.menu_with_disabled("Re-transcribe in…", Box::new(NoAction), true)
            .label(WAITS)
    } else {
        menu.submenu("Re-transcribe in…", window, cx, move |menu, _, _| {
            retranscribe_choices(&meeting)
                .into_iter()
                .fold(menu, |menu, choice| match choice {
                    Some((name, action)) => {
                        let checked = current.as_ref().is_some_and(|c| action.picks(c));
                        menu.menu_with_check(name, checked, Box::new(action))
                    }
                    None => menu.separator(),
                })
        })
    };
    menu.separator()
        .menu("Rename", Box::new(Rename))
        .menu("Move to Trash", Box::new(MoveToTrash))
}

/// **Recording > Translate Summary…**: the meeting languages, a separator (`None`), then every
/// other language, each with the action that asks for it. The **Translate…** combobox lists the
/// same, through `languages::Offer::translate`.
pub fn translate_choices(
    meeting: &[LangTag],
    written: Option<&LangTag>,
) -> Vec<Option<(String, TranslateSummaryTo)>> {
    languages::translate_choices(meeting, written)
        .into_iter()
        .map(|choice| {
            choice.map(|target| (languages::name(&target), TranslateSummaryTo { target }))
        })
        .collect()
}

/// Automatic, a separator (`None`), then each meeting language: the Language options of
/// Settings, computed the same way.
pub fn retranscribe_choices(meeting: &[LangTag]) -> Vec<Option<(String, RetranscribeIn)>> {
    let mut items = Vec::new();
    for (ix, language) in languages::choices(meeting, &LanguagePref::Auto)
        .options
        .into_iter()
        .enumerate()
    {
        if ix == 1 {
            items.push(None);
        }
        items.push(Some((
            languages::label(&language),
            RetranscribeIn { language },
        )));
    }
    items
}
