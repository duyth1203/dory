//! Launch state for the visual pass (`scripts/visual-pass.sh`): `DORY_SHOW` opens one view,
//! `DORY_APPEARANCE` forces light or dark for Dory only, and `DORY_SYSTEM_PREFS` stands in for the
//! macOS display settings. The script reaches every view this way, without sending the app any
//! input. All three are refused unless `DORY_DATA_DIR` is set, like `DORY_MACHINE` and the
//! `--fake-*` flags, so they never touch the real library.
//!
//! `DORY_SHOW` takes one of:
//! - `settings:<pane>`, with `:languages-open` on Transcription for the combobox popover,
//!   `:theme` on General scrolled to the Theme section as a search would, and an optional
//!   trailing `:min-height` for Settings at its shortest
//! - `about`
//! - `recording:<title>` and `recording:<title>:transcript`, and `recording:<title>:translate-open`
//!   for the Summary tab with the **Translate…** combobox popover open, or
//!   `recording:<title>:translate-with:<model id>` with it open as **Translate Summary with…**
//!   opens it for that summary model
//! - `phase:<P1, P2, P3a, P3b, P4, P5, P5f, P6, P7, or P8>[:<title>]`, on that recording or the
//!   newest
//! - `explainer`, the first speech model's info popover in Settings › Transcription, and
//!   `explainer:<model name>` for the model of that name, speech or summary
//! - `queue`, the Processing window, and `queue:<state>` pinned to one of
//!   [`fixtures::QUEUE_STATES`] (running, light, blocked, finished, empty, busy)
//! - `summaries`, Set Up Summaries as the library's models leave it, and `summaries:<U1, U2, or
//!   U3>` pinned to that step
//! - `quit`, the "Quit Dory?" dialog with a recording, two jobs, and a download in progress
//! - `search:<query>`, the query in the sidebar's search field and its first hit open, with the
//!   matches marked
//!
//! The app prints `show: ready` once the view is in place.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use dory_core::models::{Llm, ModelId};
use gpui_kit::{App, Entity, Window, px, size};

use crate::app::{AppEvent, AppState, DetailTab};
use crate::library::Selection;
use crate::settings_window::{self, Pane};
use crate::system_prefs::SystemPrefs;
use crate::{about, fixtures, models_view, processing_window, quit, set_up_summaries};

pub const SHOW_VAR: &str = "DORY_SHOW";
pub const APPEARANCE_VAR: &str = "DORY_APPEARANCE";
pub const SYSTEM_PREFS_VAR: &str = "DORY_SYSTEM_PREFS";
const DATA_DIR_VAR: &str = "DORY_DATA_DIR";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Settings {
        pane: Pane,
        languages_open: bool,
        /// Scrolled to General's Theme section.
        theme: bool,
        min_height: bool,
    },
    About,
    Recording {
        title: String,
        tab: DetailTab,
        /// The Summary tab's **Translate…** popover is open.
        translate_open: bool,
        /// It was opened by **Translate Summary with…** for this model.
        translate_with: Option<ModelId<Llm>>,
    },
    /// `ix` indexes [`fixtures::card_phases`]. No title means the newest recording.
    Phase {
        ix: usize,
        title: Option<String>,
    },
    /// A model's explainer, by display name. `None` is the first speech model's.
    Explainer(Option<String>),
    /// `None` shows the live queue.
    Queue(Option<String>),
    /// U1, U2, or U3. `None` follows the models.
    Summaries(Option<String>),
    Quit,
    /// The query, searched as if typed, with its first hit open.
    Search(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub view: Option<View>,
    pub appearance: Option<Appearance>,
    pub system_prefs: Option<SystemPrefs>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchError {
    NeedsDataDir(&'static str),
    View(String),
    Appearance(String),
    SystemPrefs(String),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::NeedsDataDir(var) => write!(
                f,
                "{var} is for test lanes, so it needs {DATA_DIR_VAR} set to a throwaway folder"
            ),
            LaunchError::View(value) => write!(
                f,
                "{SHOW_VAR} takes settings:<pane>, settings:transcription:languages-open, about, recording:<title>[:transcript], phase:<P1..P8>[:<title>], explainer[:<model name>], queue[:<running|light|blocked|finished|empty|busy>], summaries[:<U1|U2|U3>], or quit, not {value:?}"
            ),
            LaunchError::Appearance(value) => {
                write!(f, "{APPEARANCE_VAR} takes light or dark, not {value:?}")
            }
            LaunchError::SystemPrefs(value) => write!(
                f,
                "{SYSTEM_PREFS_VAR} takes increase-contrast, reduce-transparency, reduce-motion, and differentiate-without-color, comma-separated, not {value:?}"
            ),
        }
    }
}

impl std::error::Error for LaunchError {}

impl Launch {
    pub fn from_env() -> Result<Launch, LaunchError> {
        let var = |name| std::env::var(name).ok();
        let data_dir_set = std::env::var_os(DATA_DIR_VAR).is_some_and(|d| !d.is_empty());
        Launch::from_vars(
            var(SHOW_VAR).as_deref(),
            var(APPEARANCE_VAR).as_deref(),
            var(SYSTEM_PREFS_VAR).as_deref(),
            data_dir_set,
        )
    }

    /// [`Launch::from_env`] with the variables passed in.
    pub fn from_vars(
        show: Option<&str>,
        appearance: Option<&str>,
        system_prefs: Option<&str>,
        data_dir_set: bool,
    ) -> Result<Launch, LaunchError> {
        for (var, value) in [
            (SHOW_VAR, show),
            (APPEARANCE_VAR, appearance),
            (SYSTEM_PREFS_VAR, system_prefs),
        ] {
            if value.is_some() && !data_dir_set {
                return Err(LaunchError::NeedsDataDir(var));
            }
        }
        Ok(Launch {
            view: show.map(parse_view).transpose()?,
            appearance: appearance.map(parse_appearance).transpose()?,
            system_prefs: system_prefs.map(parse_system_prefs).transpose()?,
        })
    }
}

fn parse_view(value: &str) -> Result<View, LaunchError> {
    let bad = || LaunchError::View(value.to_owned());
    let trimmed = value.trim();
    match trimmed {
        "about" => return Ok(View::About),
        "explainer" => return Ok(View::Explainer(None)),
        "summaries" => return Ok(View::Summaries(None)),
        "queue" => return Ok(View::Queue(None)),
        "quit" => return Ok(View::Quit),
        _ => {}
    }
    if let Some(query) = trimmed.strip_prefix("search:") {
        if query.trim().is_empty() {
            return Err(bad());
        }
        return Ok(View::Search(query.trim().to_owned()));
    }
    if let Some(state) = trimmed.strip_prefix("queue:") {
        return fixtures::queue_state(state)
            .map(|_| View::Queue(Some(state.to_owned())))
            .ok_or_else(bad);
    }
    if let Some(step) = trimmed.strip_prefix("summaries:") {
        return fixtures::summaries_step(step)
            .map(|_| View::Summaries(Some(step.to_ascii_uppercase())))
            .ok_or_else(bad);
    }
    if let Some(name) = trimmed.strip_prefix("explainer:") {
        if name.is_empty() {
            return Err(bad());
        }
        return Ok(View::Explainer(Some(name.to_owned())));
    }
    if let Some(rest) = trimmed.strip_prefix("settings:") {
        let (rest, min_height) = match rest.strip_suffix(":min-height") {
            Some(rest) => (rest, true),
            None => (rest, false),
        };
        let (pane, languages_open, theme) = match rest.split_once(':') {
            Some(("transcription", "languages-open")) => (Pane::Transcription, true, false),
            Some(("general", "theme")) => (Pane::General, false, true),
            Some(_) => return Err(bad()),
            None => (Pane::parse(rest).ok_or_else(bad)?, false, false),
        };
        return Ok(View::Settings {
            pane,
            languages_open,
            theme,
            min_height,
        });
    }
    if let Some(rest) = trimmed.strip_prefix("recording:") {
        // A title may hold colons ("1:1 with Sam"), so only a known trailing word is a tab.
        let (title, tab, translate_open, translate_with) =
            if let Some((title, model)) = rest.split_once(":translate-with:") {
                let model = model.trim().parse().map_err(|_| bad())?;
                (title, DetailTab::Summary, true, Some(model))
            } else if let Some(title) = rest.strip_suffix(":transcript") {
                (title, DetailTab::Transcript, false, None)
            } else if let Some(title) = rest.strip_suffix(":translate-open") {
                (title, DetailTab::Summary, true, None)
            } else {
                (rest, DetailTab::Summary, false, None)
            };
        if title.is_empty() {
            return Err(bad());
        }
        return Ok(View::Recording {
            title: title.to_owned(),
            tab,
            translate_open,
            translate_with,
        });
    }
    if let Some(rest) = trimmed.strip_prefix("phase:") {
        let (name, title) = match rest.split_once(':') {
            Some((_, "")) => return Err(bad()),
            Some((name, title)) => (name, Some(title.to_owned())),
            None => (rest, None),
        };
        return fixtures::card_phases()
            .iter()
            .position(|(label, _, _)| {
                label
                    .split_whitespace()
                    .next()
                    .is_some_and(|key| key.eq_ignore_ascii_case(name))
            })
            .map(|ix| View::Phase { ix, title })
            .ok_or_else(bad);
    }
    Err(bad())
}

fn parse_appearance(value: &str) -> Result<Appearance, LaunchError> {
    match value.trim() {
        "light" => Ok(Appearance::Light),
        "dark" => Ok(Appearance::Dark),
        _ => Err(LaunchError::Appearance(value.to_owned())),
    }
}

fn parse_system_prefs(value: &str) -> Result<SystemPrefs, LaunchError> {
    let mut prefs = SystemPrefs::default();
    for word in value.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        match word {
            "increase-contrast" => prefs.increase_contrast = true,
            "reduce-transparency" => prefs.reduce_transparency = true,
            "reduce-motion" => prefs.reduce_motion = true,
            "differentiate-without-color" => prefs.differentiate_without_color = true,
            _ => return Err(LaunchError::SystemPrefs(value.to_owned())),
        }
    }
    Ok(prefs)
}

fn ready(view: &View) {
    eprintln!("show: ready {view:?}");
}

/// Puts `view` on screen over the main window `window`. Views that need the library wait for
/// its first rows.
pub fn run(view: View, app: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    match view.clone() {
        View::About => {
            about::open(cx);
            ready(&view);
        }
        View::Queue(state) => {
            if let Some(queue) = state.as_deref().and_then(fixtures::queue_state) {
                app.update(cx, |app, cx| app.preview_queue(Some(queue), cx));
            }
            processing_window::open(app, cx);
            ready(&view);
        }
        View::Summaries(step) => {
            // Deferred: the dialog layer is the main window's root view, still being built.
            window.defer(cx, move |window, cx| {
                match step.as_deref().map(fixtures::summaries_step) {
                    Some(Some(step)) => set_up_summaries::preview(step, window, cx),
                    Some(None) => {}
                    None => set_up_summaries::sheet(window, cx),
                }
                ready(&view);
            });
        }
        View::Quit => {
            let app = app.clone();
            window.defer(cx, move |window, cx| {
                let prompt = quit::prompt(&fixtures::busy()).expect("the sample is busy");
                quit::ask(prompt, app, window, cx);
                ready(&view);
            });
        }
        View::Settings {
            pane,
            languages_open,
            theme,
            min_height,
        } => {
            settings_window::open_at(app, pane, cx);
            let (Some(handle), Some(settings)) = (
                settings_window::current_window(cx),
                settings_window::current(cx),
            ) else {
                return;
            };
            let _ = handle.update(cx, |_, window, cx| {
                if min_height {
                    window.resize(size(
                        px(settings_window::WIDTH),
                        px(settings_window::MIN_HEIGHT),
                    ));
                }
                if languages_open {
                    settings.update(cx, |settings, cx| settings.open_languages(window, cx));
                }
                if let Some(entry) = theme
                    .then(|| settings_window::ENTRIES.iter().find(|e| e.label == "Theme"))
                    .flatten()
                {
                    settings.update(cx, |settings, cx| settings.jump(*entry, cx));
                }
            });
            ready(&view);
        }
        View::Explainer(Some(name)) => {
            settings_window::open_at(app, Pane::Transcription, cx);
            let (Some(settings), Some(models)) =
                (settings_window::current(cx), models_view::shared(cx))
            else {
                return;
            };
            let open = move |cx: &mut App| settings.update(cx, |s, cx| s.open_explainer(&name, cx));
            if open(cx) {
                ready(&view);
                return;
            }
            let done = Rc::new(Cell::new(false));
            cx.observe(&models, move |_, cx| {
                if !done.get() && open(cx) {
                    done.set(true);
                    ready(&view);
                }
            })
            .detach();
        }
        View::Explainer(None) => {
            settings_window::open_at(app, Pane::Transcription, cx);
            let (Some(settings), Some(models)) =
                (settings_window::current(cx), models_view::shared(cx))
            else {
                return;
            };
            let open = move |cx: &mut App| settings.update(cx, |s, cx| s.open_first_explainer(cx));
            if open(cx) {
                ready(&view);
                return;
            }
            let done = Rc::new(Cell::new(false));
            cx.observe(&models, move |_, cx| {
                if !done.get() && open(cx) {
                    done.set(true);
                    ready(&view);
                }
            })
            .detach();
        }
        View::Search(query) => {
            let filled = Rc::new(Cell::new(false));
            let done = Rc::new(Cell::new(false));
            // Deferred, so the launch selection rule in `prefs::watch` has run first.
            let step = move |app: Entity<AppState>, cx: &mut App| {
                let (view, query, filled, done) =
                    (view.clone(), query.clone(), filled.clone(), done.clone());
                cx.defer(move |cx| {
                    if done.get() || !app.read(cx).is_loaded() {
                        return;
                    }
                    if !filled.get() {
                        filled.set(true);
                        app.update(cx, |app, cx| app.fill_search(&query, cx));
                        return;
                    }
                    let state = app.read(cx);
                    if state.typed.text() != query {
                        return;
                    }
                    let Some(results) = state.search.as_ref() else {
                        return;
                    };
                    done.set(true);
                    match results.hits.first().map(|hit| hit.row.recording.id.clone()) {
                        Some(id) => {
                            app.update(cx, |app, cx| app.select(Selection::One(id), cx));
                            ready(&view);
                        }
                        None => eprintln!("show: no hit for {view:?}"),
                    }
                });
            };
            step(app.clone(), cx);
            cx.subscribe(app, move |app, event: &AppEvent, cx| {
                if matches!(event, AppEvent::RowsReloaded) {
                    step(app, cx);
                }
            })
            .detach();
        }
        View::Recording { .. } | View::Phase { .. } => {
            let done = Rc::new(Cell::new(false));
            let handle = window.window_handle();
            // Deferred, so the launch selection rule in `prefs::watch` has run first.
            let try_select = move |app: Entity<AppState>, cx: &mut App| {
                let (view, done) = (view.clone(), done.clone());
                cx.defer(move |cx| {
                    if !done.get() && app.read(cx).is_loaded() {
                        done.set(select(&view, &app, cx));
                        if done.get()
                            && let View::Recording {
                                translate_open: true,
                                translate_with,
                                ..
                            } = &view
                        {
                            // After the shell has made the detail pane for the selection.
                            let llm = translate_with.clone();
                            cx.defer(move |cx| open_translate(handle, llm, cx));
                        }
                    }
                });
            };
            try_select(app.clone(), cx);
            cx.subscribe(app, move |app, event: &AppEvent, cx| {
                if matches!(event, AppEvent::RowsReloaded) {
                    try_select(app, cx);
                }
            })
            .detach();
        }
    }
}

/// Opens **Translate…** in the main window's detail pane, for `llm` when it is set.
fn open_translate(handle: gpui_kit::AnyWindowHandle, llm: Option<ModelId<Llm>>, cx: &mut App) {
    let _ = handle.update(cx, |_, window, cx| {
        let detail = gpui_kit::base::Root::read(window, cx)
            .view()
            .clone()
            .downcast::<crate::shell::Shell>()
            .ok()
            .and_then(|shell| shell.read(cx).detail().cloned());
        match detail {
            Some(detail) => {
                detail.update(cx, |detail, cx| detail.open_translate_with(llm, window, cx))
            }
            None => eprintln!("show: no detail pane to open Translate… in"),
        }
    });
}

/// Selects the recording `view` names. False when the rows do not list it yet.
fn select(view: &View, app: &Entity<AppState>, cx: &mut App) -> bool {
    let picked = app.update(cx, |app, cx| {
        let title = match view {
            View::Recording { title, .. } => Some(title),
            View::Phase { title, .. } => title.as_ref(),
            _ => None,
        };
        let id = app
            .recordings
            .iter()
            .find(|row| title.is_none_or(|t| row.recording.title.as_deref() == Some(t)))
            .map(|row| row.recording.id.clone())?;
        app.select(Selection::One(id.clone()), cx);
        match view {
            View::Recording { tab, .. } => app.set_tab(*tab, cx),
            View::Phase { ix, .. } => {
                let (_, phase, lines) = fixtures::card_phases().swap_remove(*ix);
                app.preview_phase(id, phase, lines, cx);
            }
            _ => {}
        }
        Some(())
    });
    if picked.is_some() {
        ready(view);
    } else {
        eprintln!("show: no recording for {view:?}");
    }
    picked.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(show: &str) -> Result<Option<View>, LaunchError> {
        Launch::from_vars(Some(show), None, None, true).map(|launch| launch.view)
    }

    #[test]
    fn settings_general_theme_scrolls_to_the_theme_section() {
        assert_eq!(
            view("settings:general:theme"),
            Ok(Some(View::Settings {
                pane: Pane::General,
                languages_open: false,
                theme: true,
                min_height: false
            }))
        );
    }

    #[test]
    fn every_view_parses() {
        for pane in Pane::ALL {
            assert_eq!(
                view(&format!("settings:{}", pane.slug())),
                Ok(Some(View::Settings {
                    pane,
                    languages_open: false,
                    theme: false,
                    min_height: false
                }))
            );
        }
        assert_eq!(
            view("settings:transcription:languages-open:min-height"),
            Ok(Some(View::Settings {
                pane: Pane::Transcription,
                languages_open: true,
                theme: false,
                min_height: true
            }))
        );
        assert_eq!(view("about"), Ok(Some(View::About)));
        assert_eq!(view("explainer"), Ok(Some(View::Explainer(None))));
        assert_eq!(
            view("explainer:Apple Intelligence"),
            Ok(Some(View::Explainer(Some("Apple Intelligence".into()))))
        );
        assert_eq!(view("summaries"), Ok(Some(View::Summaries(None))));
        assert_eq!(view("queue"), Ok(Some(View::Queue(None))));
        for state in fixtures::QUEUE_STATES {
            assert_eq!(
                view(&format!("queue:{state}")),
                Ok(Some(View::Queue(Some(state.into()))))
            );
        }
        for step in ["U1", "U2", "U3", "u2"] {
            assert_eq!(
                view(&format!("summaries:{step}")),
                Ok(Some(View::Summaries(Some(step.to_ascii_uppercase()))))
            );
        }
        assert_eq!(view("quit"), Ok(Some(View::Quit)));
        assert_eq!(
            view("search: hoja de ruta "),
            Ok(Some(View::Search("hoja de ruta".into())))
        );
        assert!(view("search: ").is_err());
        assert_eq!(
            view("recording:1:1 with Sam"),
            Ok(Some(View::Recording {
                title: "1:1 with Sam".into(),
                tab: DetailTab::Summary,
                translate_open: false,
                translate_with: None,
            }))
        );
        assert_eq!(
            view("recording:1:1 with Sam:translate-with:ollama/gemma4:e4b"),
            Ok(Some(View::Recording {
                title: "1:1 with Sam".into(),
                tab: DetailTab::Summary,
                translate_open: true,
                translate_with: Some("ollama/gemma4:e4b".parse().unwrap()),
            }))
        );
        assert_eq!(
            view("recording:Weekly sync:translate-open"),
            Ok(Some(View::Recording {
                title: "Weekly sync".into(),
                tab: DetailTab::Summary,
                translate_open: true,
                translate_with: None,
            }))
        );
        assert_eq!(
            view("recording:Weekly sync:transcript"),
            Ok(Some(View::Recording {
                title: "Weekly sync".into(),
                tab: DetailTab::Transcript,
                translate_open: false,
                translate_with: None,
            }))
        );
        let labels: Vec<&str> = fixtures::card_phases()
            .iter()
            .map(|(label, _, _)| label.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "P1", "P2", "P3a", "P3b", "P4", "P5", "P5f", "P6", "P7", "P8"
            ]
        );
        for (ix, label) in labels.iter().enumerate() {
            assert_eq!(
                view(&format!("phase:{label}")),
                Ok(Some(View::Phase { ix, title: None }))
            );
        }
        assert_eq!(
            view("phase:p8:1:1 with Sam"),
            Ok(Some(View::Phase {
                ix: 9,
                title: Some("1:1 with Sam".into())
            }))
        );
    }

    #[test]
    fn unknown_views_are_refused() {
        for bad in [
            "",
            "settings",
            "settings:audio",
            "settings:general:languages-open",
            "settings:transcription:theme",
            "recording:",
            "recording::transcript",
            "phase:P9",
            "phase:",
            "phase:P4:",
            "queue:open",
            "summaries:U4",
            "explainer:",
        ] {
            assert_eq!(view(bad), Err(LaunchError::View(bad.into())), "{bad:?}");
        }
    }

    #[test]
    fn appearance_and_system_prefs_parse() {
        let launch = |appearance, prefs| Launch::from_vars(None, appearance, prefs, true);
        assert_eq!(
            launch(Some("dark"), None).map(|l| l.appearance),
            Ok(Some(Appearance::Dark))
        );
        assert_eq!(
            launch(Some("light"), None).map(|l| l.appearance),
            Ok(Some(Appearance::Light))
        );
        assert_eq!(
            launch(Some("sepia"), None),
            Err(LaunchError::Appearance("sepia".into()))
        );
        assert_eq!(
            launch(None, Some("increase-contrast, reduce-motion")).map(|l| l.system_prefs),
            Ok(Some(SystemPrefs {
                increase_contrast: true,
                reduce_motion: true,
                ..SystemPrefs::default()
            }))
        );
        assert_eq!(
            launch(
                None,
                Some("reduce-transparency,differentiate-without-color")
            )
            .map(|l| l.system_prefs),
            Ok(Some(SystemPrefs {
                reduce_transparency: true,
                differentiate_without_color: true,
                ..SystemPrefs::default()
            }))
        );
        assert_eq!(
            launch(None, Some("")).map(|l| l.system_prefs),
            Ok(Some(SystemPrefs::default()))
        );
        assert_eq!(
            launch(None, Some("bold-text")),
            Err(LaunchError::SystemPrefs("bold-text".into()))
        );
    }

    #[test]
    fn each_variable_needs_a_data_folder() {
        assert_eq!(
            Launch::from_vars(Some("about"), None, None, false),
            Err(LaunchError::NeedsDataDir(SHOW_VAR))
        );
        assert_eq!(
            Launch::from_vars(None, Some("dark"), None, false),
            Err(LaunchError::NeedsDataDir(APPEARANCE_VAR))
        );
        assert_eq!(
            Launch::from_vars(None, None, Some("reduce-motion"), false),
            Err(LaunchError::NeedsDataDir(SYSTEM_PREFS_VAR))
        );
        assert_eq!(
            Launch::from_vars(None, None, None, false),
            Ok(Launch::default())
        );
    }
}
