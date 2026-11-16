//! Window state that survives a relaunch (UX-24), and the first-run step (UX-16).
//!
//! It lives in a versioned JSON file at `<data_dir>/ui-prefs.json`, not in the macOS config
//! folder. `DORY_DATA_DIR` then isolates it, so a test lane or a copied library never touches the
//! real window state, and one process owns the folder anyway. A missing, unreadable, or newer
//! file gives `Prefs::default()`, never an error.
//!
//! `watch` restores the selection on the first list of rows (only after a short absence, see
//! [`RESTORE_WITHIN`]) and saves with a debounce whenever the window size, the sidebar width, or
//! the selection or the onboarding step changes, and once more on quit with the quit time.

use std::cell::RefCell;
use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dory_core::domain::{RecordingId, Timestamp};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, CursorStyle, Entity, FocusHandle, Global,
    InteractiveElement as _, IntoElement as _, KeyDownEvent, Render, Role,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use serde::{Deserialize, Serialize};

use crate::a11y;
use crate::app::{AppEvent, AppState};
use crate::library::Selection;
use crate::onboarding::OnboardingStep;

pub const SIDEBAR_WIDTH: RangeInclusive<f32> = 240.0..=380.0;
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 300.0;
pub const DEFAULT_WINDOW_SIZE: (f32, f32) = (1100.0, 720.0);
/// UX-24's proposed rule: restore the selection only after a short absence.
pub const RESTORE_WITHIN: Duration = Duration::from_secs(4 * 3600);
/// How long the state sits still before it is written.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

const FILE_NAME: &str = "ui-prefs.json";
const VERSION: u32 = 1;
const WINDOW_SIDE: RangeInclusive<f32> = 400.0..=10_000.0;
const KEY_STEP: f32 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Points, width then height.
    pub window_size: (f32, f32),
    /// Always within [`SIDEBAR_WIDTH`].
    pub sidebar_width: f32,
    pub last_selection: Option<RecordingId>,
    pub quit_at: Option<Timestamp>,
    /// `None` in a file from before onboarding, until `AppState` resolves it against the library.
    pub onboarding: Option<OnboardingStep>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            window_size: DEFAULT_WINDOW_SIZE,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            last_selection: None,
            quit_at: None,
            onboarding: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Disk {
    version: u32,
    window_width: f32,
    window_height: f32,
    sidebar_width: f32,
    last_selection: Option<String>,
    quit_at_ms: Option<i64>,
    onboarding: Option<String>,
}

impl Default for Disk {
    fn default() -> Self {
        Disk::from(&Prefs::default())
    }
}

impl From<&Prefs> for Disk {
    fn from(prefs: &Prefs) -> Self {
        Disk {
            version: VERSION,
            window_width: prefs.window_size.0,
            window_height: prefs.window_size.1,
            sidebar_width: prefs.sidebar_width,
            last_selection: prefs.last_selection.as_ref().map(ToString::to_string),
            quit_at_ms: prefs.quit_at.map(|t| t.0),
            onboarding: prefs.onboarding.map(|step| step.as_str().to_owned()),
        }
    }
}

impl Prefs {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    pub fn read(dir: &Path) -> Prefs {
        let Ok(bytes) = std::fs::read(Self::path(dir)) else {
            return Prefs::default();
        };
        let Ok(disk) = serde_json::from_slice::<Disk>(&bytes) else {
            return Prefs::default();
        };
        if disk.version != VERSION {
            return Prefs::default();
        }
        let side = |v: f32, default: f32| {
            if v.is_finite() && WINDOW_SIDE.contains(&v) {
                v
            } else {
                default
            }
        };
        Prefs {
            window_size: (
                side(disk.window_width, DEFAULT_WINDOW_SIZE.0),
                side(disk.window_height, DEFAULT_WINDOW_SIZE.1),
            ),
            sidebar_width: clamp_sidebar_width(disk.sidebar_width),
            last_selection: disk
                .last_selection
                .and_then(|id| RecordingId::parse(&id).ok()),
            quit_at: disk.quit_at_ms.map(Timestamp),
            onboarding: disk.onboarding.as_deref().and_then(OnboardingStep::parse),
        }
    }

    /// Writes through a temporary file, so a crash never leaves half a file.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path(dir);
        let temp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(&Disk::from(self)).map_err(io::Error::other)?;
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, &path)
    }

    /// Everything but the quit time, which only a real quit sets.
    fn same_saved_state(&self, other: &Prefs) -> bool {
        self.window_size == other.window_size
            && self.sidebar_width == other.sidebar_width
            && self.last_selection == other.last_selection
            && self.onboarding == other.onboarding
    }
}

pub fn clamp_sidebar_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(*SIDEBAR_WIDTH.start(), *SIDEBAR_WIDTH.end())
    } else {
        DEFAULT_SIDEBAR_WIDTH
    }
}

pub fn load(config: &dory::Config) -> Prefs {
    Prefs::read(&config.data_dir)
}

/// The selection to open on. A quit time in the future means the clock moved, so it does not
/// count as a short absence.
pub fn restore_selection(prefs: &Prefs, now: Timestamp) -> Selection {
    let (Some(id), Some(quit_at)) = (&prefs.last_selection, prefs.quit_at) else {
        return Selection::None;
    };
    let away = now.0 - quit_at.0;
    if (0..RESTORE_WITHIN.as_millis() as i64).contains(&away) {
        Selection::One(id.clone())
    } else {
        Selection::None
    }
}

fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    Timestamp(ms)
}

struct Persist {
    dir: PathBuf,
    saved: Prefs,
    generation: u64,
    /// True once the launch selection is decided, by the restore rule or by the user first.
    decided: bool,
    _bounds: Option<Entity<BoundsWatcher>>,
    _subscriptions: Vec<Subscription>,
}

/// `observe_window_bounds` lives on `Context`, so a tiny entity holds it.
struct BoundsWatcher {
    _subscription: Subscription,
}

impl Render for BoundsWatcher {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui_kit::IntoElement {
        div()
    }
}

/// Called once by `main` after the window opens.
pub fn watch(app: &Entity<AppState>, config: &dory::Config, window: &mut Window, cx: &mut App) {
    // Compared with the file, not the launch prefs, so a step resolved at launch is saved at once.
    let persist = Rc::new(RefCell::new(Persist {
        dir: config.data_dir.clone(),
        saved: Prefs::read(&config.data_dir),
        generation: 0,
        decided: false,
        _bounds: None,
        _subscriptions: Vec::new(),
    }));

    let on_event = cx.subscribe(app, {
        let persist = persist.clone();
        move |app, event: &AppEvent, cx| match event {
            AppEvent::SelectionChanged => {
                persist.borrow_mut().decided = true;
                app.update(cx, |app, cx| {
                    app.prefs.last_selection = app.selection.id().cloned();
                    cx.notify();
                });
            }
            AppEvent::RowsReloaded => restore(&app, &persist, cx),
        }
    });

    let on_change = cx.observe(app, {
        let persist = persist.clone();
        move |app, cx| {
            let current = app.read(cx).prefs.clone();
            let mut state = persist.borrow_mut();
            if current.same_saved_state(&state.saved) {
                return;
            }
            state.generation += 1;
            let generation = state.generation;
            drop(state);
            let persist = persist.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(SAVE_DEBOUNCE).await;
                let current = app.read_with(cx, |app, _| app.prefs.clone());
                let dir = {
                    let state = persist.borrow();
                    if state.generation != generation {
                        return;
                    }
                    state.dir.clone()
                };
                let mut written = current.clone();
                written.quit_at = None;
                let saved = written.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { written.write(&dir) })
                    .await;
                match result {
                    Ok(()) => persist.borrow_mut().saved = saved,
                    Err(error) => eprintln!("dory: could not save window state: {error}"),
                }
            })
            .detach();
        }
    });

    let bounds = {
        let app = app.clone();
        cx.new(|cx| BoundsWatcher {
            _subscription: cx.observe_window_bounds(window, move |_, window, cx| {
                let size = window.viewport_size();
                let size = (f32::from(size.width), f32::from(size.height));
                app.update(cx, |app, cx| {
                    if WINDOW_SIDE.contains(&size.0)
                        && WINDOW_SIDE.contains(&size.1)
                        && app.prefs.window_size != size
                    {
                        app.prefs.window_size = size;
                        cx.notify();
                    }
                });
            }),
        })
    };

    let on_quit = cx.on_app_quit({
        let app = app.clone();
        let persist = persist.clone();
        move |cx| {
            let mut prefs = app.read(cx).prefs.clone();
            prefs.quit_at = Some(now());
            if let Err(error) = prefs.write(&persist.borrow().dir) {
                eprintln!("dory: could not save window state: {error}");
            }
            async {}
        }
    });

    {
        let mut state = persist.borrow_mut();
        state._bounds = Some(bounds);
        state._subscriptions = vec![on_event, on_change, on_quit];
    }
    // `persist` holds the subscriptions whose callbacks hold `persist`: a cycle that keeps the
    // watcher alive for the life of the process, which is the point.
    if app.read(cx).is_loaded() {
        restore(app, &persist, cx);
    }
}

fn restore(app: &Entity<AppState>, persist: &Rc<RefCell<Persist>>, cx: &mut App) {
    if std::mem::replace(&mut persist.borrow_mut().decided, true) {
        return;
    }
    app.update(cx, |app, cx| {
        match restore_selection(&app.prefs, now()) {
            Selection::None => app.prefs.last_selection = None,
            selection => app.select(selection, cx),
        }
        cx.notify();
    });
}

struct ResizeDrag;

struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui_kit::IntoElement {
        div()
    }
}

struct ResizeFocus(FocusHandle);

impl Global for ResizeFocus {}

fn set_sidebar_width(app: &Entity<AppState>, width: f32, cx: &mut App) {
    let width = clamp_sidebar_width(width);
    app.update(cx, |app, cx| {
        if app.prefs.sidebar_width != width {
            app.prefs.sidebar_width = width;
            cx.notify();
        }
    });
}

/// A drag handle on the sidebar's trailing edge that clamps to [`SIDEBAR_WIDTH`], also moved by
/// Left and Right when it has focus.
pub fn sidebar_resize_handle(app: &Entity<AppState>, cx: &mut App) -> AnyElement {
    if cx.try_global::<ResizeFocus>().is_none() {
        let focus = cx.focus_handle();
        cx.set_global(ResizeFocus(focus));
    }
    let focus = cx.global::<ResizeFocus>().0.clone();
    let width = app.read(cx).prefs.sidebar_width;
    let drag = app.clone();
    let key = app.clone();
    a11y::node("sidebar-resize", Role::Splitter, "Sidebar width")
        .aria_value(format!("{} points", width.round() as i32))
        .track_focus(&focus.tab_stop(true))
        .w(px(8.))
        .h_full()
        .flex_none()
        .cursor(CursorStyle::ResizeLeftRight)
        .on_drag(ResizeDrag, |_, _, _, cx| cx.new(|_| NoGhost))
        .on_drag_move::<ResizeDrag>(move |event, _, cx| {
            let center = event.bounds.origin.x + event.bounds.size.width / 2.;
            let delta = f32::from(event.event.position.x - center);
            let current = drag.read(cx).prefs.sidebar_width;
            set_sidebar_width(&drag, current + delta, cx);
        })
        .on_key_down(move |event: &KeyDownEvent, _, cx| {
            let delta = match event.keystroke.key.as_str() {
                "left" => -KEY_STEP,
                "right" => KEY_STEP,
                _ => return,
            };
            let current = key.read(cx).prefs.sidebar_width;
            set_sidebar_width(&key, current + delta, cx);
            cx.stop_propagation();
        })
        .into_any_element()
}
