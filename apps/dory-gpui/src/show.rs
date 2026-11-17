//! Launch state for the visual pass (`scripts/visual-pass.sh`): `DORY_SHOW` opens one view,
//! `DORY_APPEARANCE` forces light or dark for Dory only, and `DORY_SYSTEM_PREFS` stands in for the
//! macOS display settings. The script reaches every view this way, without sending the app any
//! input. All three are refused unless `DORY_DATA_DIR` is set, like `DORY_MACHINE` and the
//! `--fake-*` flags, so they never touch the real library.
//!
//! `DORY_SHOW` takes one of:
//! - `settings:<pane>`, with `:languages-open` on Transcription for the combobox popover, and an
//!   optional trailing `:min-height` for Settings at its shortest
//! - `about`
//! - `recording:<title>` and `recording:<title>:transcript`
//! - `phase:<P1, P2, P3a, P3b, P4, P5, P6, P7, or P8>[:<title>]`, on that recording or the
//!   newest
//! - `explainer`, the first speech model's info popover in Settings › Transcription
//! - `queue`, the Processing Queue dialog
//!
//! The app prints `show: ready` once the view is in place.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use gpui_kit::{App, Entity, Window, px, size};

use crate::app::{AppEvent, AppState, DetailTab};
use crate::library::Selection;
use crate::settings_window::{self, Pane};
use crate::system_prefs::SystemPrefs;
use crate::{about, fixtures, models_view, queue_popover};

pub const SHOW_VAR: &str = "DORY_SHOW";
pub const APPEARANCE_VAR: &str = "DORY_APPEARANCE";
pub const SYSTEM_PREFS_VAR: &str = "DORY_SYSTEM_PREFS";
const DATA_DIR_VAR: &str = "DORY_DATA_DIR";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Settings {
        pane: Pane,
        languages_open: bool,
        min_height: bool,
    },
    About,
    Recording {
        title: String,
        tab: DetailTab,
    },
    /// `ix` indexes [`fixtures::card_phases`]. No title means the newest recording.
    Phase {
        ix: usize,
        title: Option<String>,
    },
    Explainer,
    Queue,
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
                "{SHOW_VAR} takes settings:<pane>, settings:transcription:languages-open, about, recording:<title>[:transcript], phase:<P1..P8>[:<title>], explainer, or queue, not {value:?}"
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
        "explainer" => return Ok(View::Explainer),
        "queue" => return Ok(View::Queue),
        _ => {}
    }
    if let Some(rest) = trimmed.strip_prefix("settings:") {
        let (rest, min_height) = match rest.strip_suffix(":min-height") {
            Some(rest) => (rest, true),
            None => (rest, false),
        };
        let (pane, languages_open) = match rest.split_once(':') {
            Some(("transcription", "languages-open")) => (Pane::Transcription, true),
            Some(_) => return Err(bad()),
            None => (Pane::parse(rest).ok_or_else(bad)?, false),
        };
        return Ok(View::Settings {
            pane,
            languages_open,
            min_height,
        });
    }
    if let Some(rest) = trimmed.strip_prefix("recording:") {
        // A title may hold colons ("1:1 with Sam"), so only a trailing `:transcript` is a tab.
        let (title, tab) = match rest.strip_suffix(":transcript") {
            Some(title) => (title, DetailTab::Transcript),
            None => (rest, DetailTab::Summary),
        };
        if title.is_empty() {
            return Err(bad());
        }
        return Ok(View::Recording {
            title: title.to_owned(),
            tab,
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
        View::Queue => {
            // Deferred: a dialog needs the window's root view, which the caller is still building.
            let app = app.clone();
            window.defer(cx, move |window, cx| {
                queue_popover::open(&app, window, cx);
                ready(&view);
            });
        }
        View::Settings {
            pane,
            languages_open,
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
            });
            ready(&view);
        }
        View::Explainer => {
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
        View::Recording { .. } | View::Phase { .. } => {
            let done = Rc::new(Cell::new(false));
            // Deferred, so the launch selection rule in `prefs::watch` has run first.
            let try_select = move |app: Entity<AppState>, cx: &mut App| {
                let (view, done) = (view.clone(), done.clone());
                cx.defer(move |cx| {
                    if !done.get() && app.read(cx).is_loaded() {
                        done.set(select(&view, &app, cx));
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
    fn every_view_parses() {
        for pane in Pane::ALL {
            assert_eq!(
                view(&format!("settings:{}", pane.slug())),
                Ok(Some(View::Settings {
                    pane,
                    languages_open: false,
                    min_height: false
                }))
            );
        }
        assert_eq!(
            view("settings:transcription:languages-open:min-height"),
            Ok(Some(View::Settings {
                pane: Pane::Transcription,
                languages_open: true,
                min_height: true
            }))
        );
        assert_eq!(view("about"), Ok(Some(View::About)));
        assert_eq!(view("explainer"), Ok(Some(View::Explainer)));
        assert_eq!(view("queue"), Ok(Some(View::Queue)));
        assert_eq!(
            view("recording:1:1 with Sam"),
            Ok(Some(View::Recording {
                title: "1:1 with Sam".into(),
                tab: DetailTab::Summary
            }))
        );
        assert_eq!(
            view("recording:Weekly sync:transcript"),
            Ok(Some(View::Recording {
                title: "Weekly sync".into(),
                tab: DetailTab::Transcript
            }))
        );
        let labels: Vec<&str> = fixtures::card_phases()
            .iter()
            .map(|(label, _, _)| label.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            labels,
            ["P1", "P2", "P3a", "P3b", "P4", "P5", "P6", "P7", "P8"]
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
                ix: 8,
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
            "recording:",
            "recording::transcript",
            "phase:P9",
            "phase:",
            "phase:P4:",
            "queue:open",
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
