//! The Settings window (UX-19, UX-20) in the System Settings layout of
//! `docs/design/settings-icons-blur.md` section 1: a sidebar panel with a search field and the
//! five panes, then a 52 pt toolbar with the pane's title over one scrolling content area. The
//! window is 860 wide, fixed, and at least 560 tall. It reopens on the pane it showed last.
//!
//! - General: process recordings automatically, and the appearance note.
//! - Recording: the Mac's inputs and system audio, as radio groups. No setting carries a
//!   microphone or a layout yet; every take starts with `StartOptions::default()`, the system
//!   default input plus system audio, so only those options are enabled.
//! - Transcription: Language (UX-26), computed from the meeting languages by
//!   `languages::choices`; the meeting-languages combobox (UX-29); the speech models (UX-28);
//!   Add model file, which stays disabled because no speech provider accepts a user's file today;
//!   and Processing (UX-30).
//! - Summaries: the summary models, and the Ollama steps when Ollama does not answer.
//! - Storage: the data folder, its size, and Show in Finder, then the old app's WebM files and
//!   **Move original WebM files to Trash** with Undo.
//!
//! Content has exactly one scroll area (UX-31). The combobox's list is the only inner one, and it
//! sits in a popover, not in the page.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use dory_core::domain::LanguagePref;
use dory_core::models::{Availability, Llm, Stt, UnavailableReason};
use dory_core::ports::capture::InputDevice;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle,
    FontWeight, Global, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TestSupportExt as _, TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowOptions,
    canvas, div, point, prelude::FluentBuilder as _, px, relative, size,
};

use crate::a11y;
use crate::app::{AppState, Originals};
use crate::combobox::LanguageCombobox;
use crate::icons::{self, Icon};
use crate::languages;
use crate::models_view::{self, ModelList, Models, button, note, size_text};
use crate::processing::PACE_CHOICES;
use crate::radio::{self, Radio, RadioGroup};
use crate::set_up_summaries::{COMMANDS, ollama_steps};
use crate::theme::Theme;

pub const WIDTH: f32 = 860.;
pub const HEIGHT: f32 = 720.;
pub const MIN_HEIGHT: f32 = 560.;
const SIDEBAR: f32 = 220.;
const INSET: f32 = 8.;
/// How long a search result's row stays highlighted after the jump.
pub const HIGHLIGHT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pane {
    #[default]
    General,
    Recording,
    Transcription,
    Summaries,
    Storage,
}

impl Pane {
    pub const ALL: [Pane; 5] = [
        Pane::General,
        Pane::Recording,
        Pane::Transcription,
        Pane::Summaries,
        Pane::Storage,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Recording => "Recording",
            Pane::Transcription => "Transcription",
            Pane::Summaries => "Summaries",
            Pane::Storage => "Storage",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Pane::General => "general",
            Pane::Recording => "recording",
            Pane::Transcription => "transcription",
            Pane::Summaries => "summaries",
            Pane::Storage => "storage",
        }
    }

    pub fn parse(slug: &str) -> Option<Pane> {
        Pane::ALL.into_iter().find(|p| p.slug() == slug)
    }

    pub fn id(self) -> String {
        format!("settings-pane-{}", self.slug())
    }

    /// The icon on the pane's tile in the sidebar.
    fn icon(self) -> Icon {
        match self {
            Pane::General => Icon::Settings2,
            Pane::Recording => Icon::Mic,
            Pane::Transcription => Icon::Captions,
            Pane::Summaries => Icon::FileText,
            Pane::Storage => Icon::HardDrive,
        }
    }
}

/// One setting the sidebar search can find, and the element it jumps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub pane: Pane,
    pub label: &'static str,
    pub row: &'static str,
}

const fn entry(pane: Pane, label: &'static str, row: &'static str) -> Entry {
    Entry { pane, label, row }
}

pub const ENTRIES: &[Entry] = &[
    entry(
        Pane::General,
        "Process recordings automatically",
        "row-auto-process",
    ),
    entry(Pane::General, "Appearance", "row-appearance"),
    entry(Pane::Recording, "Microphone", "row-microphone"),
    entry(Pane::Recording, "System audio", "row-system-audio"),
    entry(Pane::Transcription, "Language", "row-language"),
    entry(
        Pane::Transcription,
        "Meeting languages",
        "row-meeting-languages",
    ),
    entry(Pane::Transcription, "Speech model", "row-speech-model"),
    entry(Pane::Transcription, "Add model file", "row-add-model-file"),
    entry(Pane::Transcription, "Processing", "row-processing"),
    entry(Pane::Summaries, "Summary model", "row-summary-model"),
    entry(Pane::Storage, "Data folder", "row-data-folder"),
    entry(Pane::Storage, "Show in Finder", "row-data-folder"),
    entry(Pane::Storage, "Files from the old app", "row-originals"),
];

/// What the sidebar shows for a search: the panes whose name or settings match, then each
/// matching setting. Case and accents do not matter.
pub fn search(query: &str) -> (Vec<Pane>, Vec<Entry>) {
    let query = languages::fold(query.trim());
    if query.is_empty() {
        return (Pane::ALL.to_vec(), Vec::new());
    }
    let hit = |text: &str| languages::fold(text).contains(&query);
    let entries: Vec<Entry> = ENTRIES.iter().copied().filter(|e| hit(e.label)).collect();
    let panes = Pane::ALL
        .into_iter()
        .filter(|p| hit(p.title()) || entries.iter().any(|e| e.pane == *p))
        .collect();
    (panes, entries)
}

pub const ADD_MODEL_FILE_REASON: &str =
    "No speech engine in Dory can load a model file yet, so models come from the list above.";

/// Language > a choice from the popup.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = dory, no_json)]
pub struct ChooseLanguage {
    pub language: LanguagePref,
}

struct Open(AnyWindowHandle, Entity<SettingsWindow>);

impl Global for Open {}

/// The open Settings window's view, for a fix button and the tests.
pub fn current(cx: &App) -> Option<Entity<SettingsWindow>> {
    cx.try_global::<Open>().map(|open| open.1.clone())
}

/// Opens the Settings window on the pane it showed last, or brings it forward when it is open.
pub fn open(app: &Entity<AppState>, cx: &mut App) {
    if let Some(handle) = cx.try_global::<Open>().map(|open| open.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(20.), px(20.))),
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(WIDTH), px(HEIGHT)),
            cx,
        ))),
        window_min_size: Some(size(px(WIDTH), px(MIN_HEIGHT))),
        window_background: crate::glass::window_background(cx),
        ..Default::default()
    };
    let owner = app.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| {
        crate::glass::follow(window, cx);
        cx.new(|cx| SettingsWindow::new(owner, window, cx))
    }) {
        Ok((handle, view)) => cx.set_global(Open(handle, view)),
        Err(error) => app.update(cx, |app, cx| {
            app.say(format!("Could not open Settings: {error}"), cx)
        }),
    }
}

/// Opens Settings on `pane`: a fix button that sends the user to the setting that fixes it.
pub fn open_at(app: &Entity<AppState>, pane: Pane, cx: &mut App) {
    open(app, cx);
    if let Some(view) = current(cx) {
        view.update(cx, |view, cx| view.select(pane, cx));
    }
}

/// The data folder's path and size, read when the Storage pane first shows.
enum Storage {
    Unread,
    Reading(PathBuf),
    Read { path: PathBuf, bytes: u64 },
    Failed(SharedString),
}

/// Every byte under `path`, without following links.
pub fn folder_size(path: &Path) -> std::io::Result<u64> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        total += folder_size(&entry?.path())?;
    }
    Ok(total)
}

pub struct SettingsWindow {
    app: Entity<AppState>,
    models: Entity<Models>,
    pane: Pane,
    root: FocusHandle,
    pane_focus: HashMap<Pane, FocusHandle>,
    hit_focus: HashMap<&'static str, FocusHandle>,
    focus: HashMap<String, FocusHandle>,
    search: Entity<InputState>,
    query: SharedString,
    scroll: ScrollHandle,
    highlight: Option<&'static str>,
    highlight_generation: u64,
    /// Set by a search jump until the row's place is known and the page has scrolled to it.
    jump: Option<Rc<Cell<Option<Bounds<Pixels>>>>>,
    speech: Entity<ModelList<Stt>>,
    summaries: Entity<ModelList<Llm>>,
    languages: Entity<LanguageCombobox>,
    devices: Option<Result<Vec<InputDevice>, String>>,
    storage: Storage,
    copy_focus: Vec<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let models = models_view::install(&app, cx);
        models.update(cx, |m, cx| m.refresh(cx));
        let observe = cx.observe(&models, |_, _, cx| cx.notify());
        let devices = app.read(cx).input_devices(cx);
        cx.spawn(async move |this, cx| {
            let devices = devices.await;
            let _ = this.update(cx, |this, cx| {
                this.devices = Some(devices);
                cx.notify();
            });
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        a11y::register_text_field(&search, cx);
        let on_type = cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = search.read(cx).value();
                cx.notify();
            }
        });
        let width = cx.observe_window_bounds(window, |_, window, _| {
            let now = window.viewport_size();
            if (f32::from(now.width) - WIDTH).abs() > 0.5 {
                window.resize(size(px(WIDTH), now.height));
            }
        });
        let pane = app.read(cx).prefs.settings_pane;
        Self {
            speech: cx.new(|cx| ModelList::new(models.clone(), cx)),
            summaries: cx.new(|cx| ModelList::new(models.clone(), cx)),
            languages: cx.new(|cx| LanguageCombobox::new(models.clone(), window, cx)),
            root: cx.focus_handle(),
            pane_focus: Pane::ALL.iter().map(|p| (*p, cx.focus_handle())).collect(),
            hit_focus: HashMap::new(),
            focus: HashMap::new(),
            copy_focus: COMMANDS.iter().map(|_| cx.focus_handle()).collect(),
            search,
            query: SharedString::default(),
            scroll: ScrollHandle::new(),
            highlight: None,
            highlight_generation: 0,
            jump: None,
            app,
            models,
            pane,
            devices: None,
            storage: Storage::Unread,
            _subscriptions: vec![observe, on_type, width],
        }
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// The row a search result last jumped to, while it is still highlighted.
    pub fn highlighted(&self) -> Option<&'static str> {
        self.highlight
    }

    pub fn select(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if pane == Pane::Storage && self.pane != pane {
            self.storage = Storage::Unread;
        }
        if self.pane != pane {
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        self.pane = pane;
        self.app.update(cx, |app, cx| {
            if app.prefs.settings_pane != pane {
                app.prefs.settings_pane = pane;
                cx.notify();
            }
        });
        cx.notify();
    }

    /// A search result: shows its pane, scrolls to the row, and highlights it for a second.
    pub fn jump(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.select(entry.pane, cx);
        self.highlight = Some(entry.row);
        self.jump = Some(Rc::default());
        self.highlight_generation += 1;
        let generation = self.highlight_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIGHLIGHT).await;
            let _ = this.update(cx, |this, cx| {
                if this.highlight_generation == generation {
                    this.highlight = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn read_storage(&mut self, cx: &mut Context<Self>) {
        let path = match dory::Config::from_env() {
            Ok(config) => config.data_dir,
            Err(error) => {
                self.storage = Storage::Failed(error.to_string().into());
                return;
            }
        };
        self.storage = Storage::Reading(path.clone());
        let read = cx.background_executor().spawn({
            let path = path.clone();
            async move { folder_size(&path) }
        });
        cx.spawn(async move |this, cx| {
            let bytes = read.await;
            let _ = this.update(cx, |this, cx| {
                this.storage = match bytes {
                    Ok(bytes) => Storage::Read { path, bytes },
                    Err(error) => Storage::Failed(
                        format!("Could not read {}: {error}", path.display()).into(),
                    ),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn focus(&mut self, key: impl Into<String>, cx: &mut App) -> FocusHandle {
        self.focus
            .entry(key.into())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// Wraps a row so a search can scroll to it and highlight it: a 12% accent fill, or a 2 pt
    /// accent outline under Reduce Motion.
    fn mark(&self, row: &'static str, el: impl IntoElement, theme: &Theme, cx: &App) -> AnyElement {
        let lit = self.highlight == Some(row);
        let still = cx.reduce_motion();
        div()
            .id(row)
            .test_support()
            .relative()
            .when_some(self.jump.clone().filter(|_| lit), |d, place| {
                d.child(
                    canvas(move |bounds, _, _| place.set(Some(bounds)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
            })
            .when(lit && !still, |d| d.bg(theme.accent.opacity(0.12)))
            .child(el)
            .when(lit && still, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded(px(10.))
                        .border_2()
                        .border_color(theme.accent),
                )
            })
            .into_any_element()
    }

    fn general(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let on = self
            .models
            .read(cx)
            .settings()
            .map_or_else(|| self.app.read(cx).auto_process(), |s| s.auto_process);
        let focus = self.focus("auto-process", cx);
        let models = self.models.clone();
        let auto = radio::checkbox(
            radio::Radio::new("setting-auto-process", "Process recordings automatically")
                .description("Transcribes and summarizes each recording as soon as it is saved."),
            on,
            &focus,
            theme,
            window,
            move |_, cx| models.update(cx, |m, cx| m.edit(move |s| s.auto_process = !on, cx)),
        );
        vec![
            section(
                "settings-processing",
                "Processing",
                theme,
                vec![self.mark("row-auto-process", auto, theme, cx)],
            ),
            section(
                "settings-appearance",
                "Appearance",
                theme,
                vec![self.mark(
                    "row-appearance",
                    row(
                        "setting-appearance",
                        "Follows the system",
                        "Dory follows the system's light or dark appearance and its accent color.",
                        None,
                        theme,
                    ),
                    theme,
                    cx,
                )],
            ),
        ]
    }

    fn recording(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let microphone = match &self.devices {
            None => row(
                "microphones-loading",
                "Looking for microphones",
                "",
                None,
                theme,
            )
            .into_any_element(),
            Some(Err(error)) => row(
                "microphones-failed",
                "Could not list microphones",
                error.clone(),
                None,
                theme,
            )
            .into_any_element(),
            Some(Ok(devices)) => {
                let default = devices
                    .iter()
                    .find(|d| d.is_default)
                    .map(|d| d.name.clone());
                let mut group = RadioGroup::new("microphone", "Microphone")
                    .option(Radio::new("microphone-automatic", "Automatic").description(
                        match default {
                            Some(name) => {
                                format!("Follows the system's default input, now {name}.")
                            }
                            None => "Follows the system's default input.".into(),
                        },
                    ))
                    .selected(Some(0));
                for device in devices {
                    group = group.option(
                        Radio::new(format!("microphone-{}", device.uid), device.name.clone())
                            .description("Choose it in System Settings > Sound to record from it.")
                            .enabled(false),
                    );
                }
                let focus = self.focus("microphone", cx);
                group
                    .render(&focus, theme, window, |_, _, _| {})
                    .into_any_element()
            }
        };
        let focus = self.focus("system-audio", cx);
        let system = RadioGroup::new("system-audio", "System audio")
            .option(
                Radio::new("system-audio-separate", "On a separate channel").description(
                    "Records the Mac's audio apart from your microphone, so Dory can tell you from the people on the call.",
                ),
            )
            .option(
                Radio::new("system-audio-off", "Off")
                    .description("Not available yet: every recording includes the Mac's audio.")
                    .enabled(false),
            )
            .selected(Some(0))
            .render(&focus, theme, window, |_, _, _| {});
        vec![
            section(
                "settings-microphone",
                "Microphone",
                theme,
                vec![self.mark("row-microphone", microphone, theme, cx)],
            ),
            section(
                "settings-system-audio",
                "System audio",
                theme,
                vec![self.mark("row-system-audio", system, theme, cx)],
            ),
        ]
    }

    fn language_row(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let models = self.models.read(cx);
        let Some(settings) = models.settings().cloned() else {
            return row("setting-language", "Language", "Loading", None, theme).into_any_element();
        };
        let choices = languages::choices(&settings.meeting_languages, &settings.language);
        let note = models
            .language_note()
            .map(|n| n.to_string())
            .or(choices.note.clone());
        let current = languages::label(&choices.effective);
        let options = choices.options.clone();
        let effective = choices.effective.clone();
        let root = self.root.clone();
        let popup = Button::new("language-popup")
            .outline()
            .label(current.clone())
            .accessibility_label(format!("Language, {current}"))
            .child(Icon::ChevronsUpDown.view(12., theme.text_secondary))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.action_context(root.clone());
                for (ix, option) in options.iter().enumerate() {
                    if ix == 1 {
                        menu = menu.separator();
                    }
                    menu = menu.menu_with_check(
                        languages::label(option),
                        *option == effective,
                        Box::new(ChooseLanguage {
                            language: option.clone(),
                        }),
                    );
                }
                menu
            });
        let line = row(
            "setting-language",
            "Language",
            languages::description(&settings.meeting_languages, &choices.effective),
            Some(popup.into_any_element()),
            theme,
        );
        div()
            .child(line)
            .children(note.map(|note| {
                a11y::node("language-note", Role::Status, note.clone())
                    .px(px(14.))
                    .pb(px(11.))
                    .mt(px(-6.))
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child(note)
            }))
            .into_any_element()
    }

    fn transcription(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let language = self.language_row(theme, cx);
        let meeting = row(
            "setting-meeting-languages",
            "Meeting languages",
            "With Language on Automatic, Dory only picks a speech model that covers every language listed here.",
            None,
            theme,
        )
        .child(div().pt(px(8.)).child(self.languages.clone()));
        let add_file = a11y::node(
            "add-model-file",
            Role::Button,
            "Add model file..., unavailable",
        )
        .aria_description(ADD_MODEL_FILE_REASON)
        .min_h(px(28.))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(14.))
        .border_1()
        .border_color(theme.divider)
        .text_size(theme.text(13.))
        .text_color(theme.text_secondary)
        .child("Add model file...");
        let add_file = row(
            "setting-add-model-file",
            "Add model file",
            ADD_MODEL_FILE_REASON,
            Some(add_file.into_any_element()),
            theme,
        );
        let settings = self.models.read(cx).settings().cloned();
        let paces = settings.map(|settings| {
            let selected = PACE_CHOICES
                .iter()
                .position(|c| c.setting == settings.processing);
            let mut group = RadioGroup::new("processing", "Processing").selected(selected);
            for choice in &PACE_CHOICES {
                group =
                    group.option(Radio::new(choice.id, choice.label).description(choice.effect));
            }
            let focus = self.focus("processing", cx);
            let models = self.models.clone();
            group.render(&focus, theme, window, move |ix, _, cx| {
                let setting = PACE_CHOICES[ix].setting;
                models.update(cx, |m, cx| m.edit(move |s| s.processing = setting, cx))
            })
        });
        vec![
            section(
                "settings-language",
                "Language",
                theme,
                vec![
                    self.mark("row-language", language, theme, cx),
                    self.mark("row-meeting-languages", meeting, theme, cx),
                ],
            ),
            section(
                "settings-speech-models",
                "Speech model",
                theme,
                vec![
                    self.mark("row-speech-model", self.speech.clone(), theme, cx),
                    self.mark("row-add-model-file", add_file, theme, cx),
                ],
            ),
            section(
                "settings-pace",
                "Processing",
                theme,
                paces
                    .map(|paces| vec![self.mark("row-processing", paces, theme, cx)])
                    .unwrap_or_default(),
            ),
            footnote("Light limits both transcription and summaries.", theme),
        ]
    }

    fn summaries(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let models = self.models.read(cx);
        let unreachable = models.rows::<Llm>().is_some_and(|rows| {
            rows.iter().any(|r| {
                matches!(
                    &r.status.availability,
                    Availability::Unavailable(u)
                        if matches!(u.reason, UnavailableReason::ServerUnreachable { .. })
                )
            })
        });
        let checking = models.is_checking();
        let mut page = Vec::new();
        if unreachable {
            let focus = self.focus("check-again", cx);
            let models = self.models.clone();
            let steps = div()
                .px(px(14.))
                .py(px(11.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(note(
                    "Summaries run on this Mac through Ollama. Install and start it in Terminal, then choose Check again.",
                    theme,
                ))
                .child(ollama_steps(&self.copy_focus, theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(button(
                            "settings-check-again",
                            "Check again",
                            "Check again",
                            true,
                            &focus,
                            theme,
                            move |_, cx| models.update(cx, |m, cx| m.refresh(cx)),
                        ))
                        .children(checking.then(|| note("Checking for Ollama", theme))),
                );
            page.push(section(
                "settings-ollama",
                "Ollama is not running",
                theme,
                vec![steps.into_any_element()],
            ));
        }
        page.push(section(
            "settings-summary-models",
            "Summary model",
            theme,
            vec![self.mark("row-summary-model", self.summaries.clone(), theme, cx)],
        ));
        page
    }

    fn storage(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if matches!(self.storage, Storage::Unread) {
            self.read_storage(cx);
        }
        let (path, size) = match &self.storage {
            Storage::Unread => (None, "Measuring".to_owned()),
            Storage::Reading(path) => (Some(path.clone()), "Measuring".to_owned()),
            Storage::Read { path, bytes } => {
                (Some(path.clone()), format!("Uses {}", size_text(*bytes)))
            }
            Storage::Failed(error) => (None, error.to_string()),
        };
        let folder = match path {
            Some(path) => {
                let shown = path.display().to_string();
                let focus = self.focus("show-in-finder", cx);
                let finder = button(
                    "show-in-finder",
                    "Show in Finder, the data folder",
                    "Show in Finder",
                    false,
                    &focus,
                    theme,
                    move |_, cx| cx.reveal_path(&path),
                );
                row_with(
                    "setting-data-folder",
                    "Data folder",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            a11y::node("data-folder", Role::Label, format!("Data folder: {shown}"))
                                .text_size(theme.text(12.))
                                .font_family("Menlo")
                                .child(shown),
                        )
                        .child(
                            a11y::node("data-folder-size", Role::Label, size.clone())
                                .text_size(theme.text(12.))
                                .text_color(theme.text_secondary)
                                .child(size),
                        )
                        .into_any_element(),
                    Some(finder.into_any_element()),
                    theme,
                )
            }
            None => row("setting-data-folder", "Data folder", size, None, theme),
        };
        let mut page = vec![
            section(
                "settings-storage",
                "Data folder",
                theme,
                vec![self.mark("row-data-folder", folder, theme, cx)],
            ),
            footnote(
                "Recordings, transcripts, summaries, and downloaded speech models live here.",
                theme,
            ),
        ];
        if let Some(originals) = self.originals(theme, cx) {
            page.push(originals);
        }
        page
    }

    /// The old app's WebM files: what they cost, the one action that moves them to the Trash,
    /// and Undo. Hidden when there are none.
    fn originals(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if matches!(self.app.read(cx).originals(), Originals::Unread) {
            self.app.update(cx, |app, cx| app.read_originals(cx));
        }
        let app = self.app.clone();
        let originals = self.app.read(cx).originals().clone();
        let (line, action): (String, Option<a11y::Node>) = match &originals {
            Originals::Unread | Originals::Reading => return None,
            Originals::Listed { count: 0, .. } => return None,
            Originals::Listed { count, bytes } => {
                let focus = self.focus("trash-originals", cx);
                (
                    format!(
                        "{} from the old Dory app, {}. Dory copies each recording in first, so playback and transcripts keep working.",
                        crate::app::count(*count, "original WebM file"),
                        size_text(*bytes)
                    ),
                    Some(button(
                        "trash-originals",
                        "Move original WebM files to Trash",
                        "Move Original WebM Files to Trash",
                        false,
                        &focus,
                        theme,
                        move |_, cx| app.update(cx, |app, cx| app.trash_originals(cx)),
                    )),
                )
            }
            Originals::Moving => (
                "Copying the recordings in, then moving the files…".into(),
                None,
            ),
            Originals::Moved(trashed) => {
                let focus = self.focus("undo-trash-originals", cx);
                (
                    format!(
                        "Moved {} to the Trash.",
                        crate::app::count(trashed.count(), "original WebM file")
                    ),
                    Some(button(
                        "undo-trash-originals",
                        "Undo, put the WebM files back",
                        "Undo",
                        false,
                        &focus,
                        theme,
                        move |_, cx| app.update(cx, |app, cx| app.undo_trash_originals(cx)),
                    )),
                )
            }
            Originals::Restoring => ("Putting the files back…".into(), None),
            Originals::Failed(error) => (error.to_string(), None),
        };
        let body = div()
            .px(px(14.))
            .py(px(11.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                a11y::node("originals-status", Role::Status, line.clone())
                    .text_size(theme.text(13.))
                    .map(|status| {
                        if matches!(originals, Originals::Failed(_)) {
                            status.child(icons::beside(
                                Icon::TriangleAlert,
                                12.,
                                theme.attention,
                                theme.text(13.) * 1.4,
                                line,
                            ))
                        } else {
                            status.child(line)
                        }
                    }),
            )
            .children(action.map(|action| div().flex().child(action)))
            .child(note(
                "Nothing is deleted. The Trash keeps the files until you empty it.",
                theme,
            ));
        Some(section(
            "settings-originals",
            "Files from the old app",
            theme,
            vec![self.mark("row-originals", body, theme, cx)],
        ))
    }

    fn sidebar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let (panes, hits) = search(&self.query);
        let mut list = a11y::node("settings-panes", Role::TabList, "Settings")
            .flex()
            .flex_col()
            .gap(px(2.));
        for pane in panes {
            let selected = pane == self.pane;
            let this = this.clone();
            let tile_bg = if selected {
                theme.selected_tile()
            } else {
                theme.accent
            };
            list = list.child(
                a11y::focusable(
                    a11y::node(pane.id(), Role::Tab, pane.title()),
                    &self.pane_focus[&pane],
                    theme,
                    selected,
                )
                .aria_selected(selected)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |w, cx| w.select(pane, cx));
                })
                .cursor_pointer()
                .min_h(px(32.))
                .px(px(6.))
                .py(px(3.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(8.))
                .text_size(theme.text(13.))
                .when(selected, |row| {
                    row.bg(theme.accent).text_color(theme.on_accent)
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(22.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(tile_bg)
                        .child(pane.icon().view(14., theme.on_accent)),
                )
                .child(pane.title()),
            );
        }
        let results = (!hits.is_empty()).then(|| {
            let mut found = a11y::node("settings-results", Role::List, "Settings found")
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .px(px(6.))
                        .pt(px(8.))
                        .text_size(theme.text(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_secondary)
                        .child("Settings"),
                );
            for hit in hits {
                let focus = self
                    .hit_focus
                    .entry(hit.label)
                    .or_insert_with(|| cx.focus_handle())
                    .clone();
                let this = this.clone();
                let label = format!("{}, in {}", hit.label, hit.pane.title());
                found = found.child(
                    a11y::focusable(
                        a11y::node(
                            format!(
                                "settings-hit-{}",
                                hit.label.to_lowercase().replace(' ', "-")
                            ),
                            Role::Button,
                            label,
                        ),
                        &focus,
                        theme,
                        false,
                    )
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |w, cx| w.jump(hit, cx));
                    })
                    .cursor_pointer()
                    .min_h(px(28.))
                    .px(px(6.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .rounded(px(8.))
                    .child(div().text_size(theme.text(13.)).child(hit.label))
                    .child(
                        div()
                            .text_size(theme.text(11.))
                            .text_color(theme.text_secondary)
                            .child(hit.pane.title()),
                    ),
                );
            }
            found
        });
        div()
            .flex_none()
            .w(px(SIDEBAR + 2. * INSET))
            .h_full()
            .p(px(INSET))
            .child(
                a11y::node("settings-sidebar", Role::Navigation, "Settings panes")
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(44.))
                    .px(px(8.))
                    .rounded(px(12.))
                    .bg(crate::glass::sidebar_panel(theme, cx))
                    .border_1()
                    .border_color(theme.panel_border)
                    .when(theme.shadows, |panel| panel.shadow_sm())
                    .child(
                        Input::new(&self.search)
                            .id("settings-search")
                            .aria_label("Search settings")
                            .small()
                            .cleanable(true),
                    )
                    .child(
                        div()
                            .id("settings-sidebar-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(list)
                            .children(results),
                    ),
            )
            .into_any_element()
    }
}

/// A section: a 13 pt semibold header in `text_secondary`, then its rows in a group box with
/// full-width dividers between them.
fn section(
    id: &'static str,
    title: &'static str,
    theme: &Theme,
    rows: Vec<AnyElement>,
) -> AnyElement {
    let mut group = div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .bg(theme.group_bg)
        .border_1()
        .border_color(theme.group_border)
        .overflow_hidden();
    for (ix, row) in rows.into_iter().enumerate() {
        group = group.child(
            div()
                .when(ix > 0, |d| d.border_t_1().border_color(theme.group_border))
                .child(row),
        );
    }
    a11y::node(id, Role::Group, title)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            a11y::node(format!("{id}-title"), Role::Heading, title)
                .pl(px(4.))
                .text_size(theme.text(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_secondary)
                .child(title),
        )
        .child(group)
        .into_any_element()
}

/// A plain row: the label (13 medium) with its one-line effect below (12 secondary), and a
/// control on the right aligned to the label's first line.
fn row(
    id: &'static str,
    label: &'static str,
    description: impl Into<SharedString>,
    control: Option<AnyElement>,
    theme: &Theme,
) -> a11y::Node {
    let description: SharedString = description.into();
    let text = (!description.is_empty()).then(|| {
        div()
            .text_size(theme.text(12.))
            .line_height(relative(1.4))
            .text_color(theme.text_secondary)
            .child(description)
            .into_any_element()
    });
    row_with(
        id,
        label,
        text.unwrap_or_else(|| div().into_any_element()),
        control,
        theme,
    )
}

fn row_with(
    id: &'static str,
    label: &'static str,
    below: AnyElement,
    control: Option<AnyElement>,
    theme: &Theme,
) -> a11y::Node {
    let line = theme.text(radio::LINE);
    a11y::node(id, Role::Group, label)
        .px(px(14.))
        .py(px(11.))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(theme.text(13.))
                                .line_height(line)
                                .font_weight(FontWeight::MEDIUM)
                                .child(label),
                        )
                        .child(below),
                )
                .children(control.map(|control| {
                    let tall = line.max(px(28.));
                    div()
                        .flex_none()
                        .min_h(tall)
                        .mt((line - tall) / 2.)
                        .flex()
                        .items_center()
                        .child(control)
                })),
        )
}

fn footnote(text: &'static str, theme: &Theme) -> AnyElement {
    div()
        .mt(px(-12.))
        .pl(px(4.))
        .text_size(theme.text(12.))
        .text_color(theme.text_secondary)
        .child(text)
        .into_any_element()
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let page = match self.pane {
            Pane::General => self.general(&theme, window, cx),
            Pane::Recording => self.recording(&theme, window, cx),
            Pane::Transcription => self.transcription(&theme, window, cx),
            Pane::Summaries => self.summaries(&theme, cx),
            Pane::Storage => self.storage(&theme, cx),
        };
        if let Some(place) = self.jump.clone() {
            match place.get() {
                Some(row) => {
                    let view = self.scroll.bounds();
                    let offset = self.scroll.offset();
                    let by = view.top() + px(20.) - row.top();
                    self.scroll
                        .set_offset(point(offset.x, (offset.y + by).min(px(0.))));
                    self.jump = None;
                }
                None => cx.notify(),
            }
        }
        let error = self.models.read(cx).error().cloned();
        let sidebar = self.sidebar(&theme, cx);
        div()
            .track_focus(&self.root)
            .on_action(cx.listener(|this, action: &ChooseLanguage, _, cx| {
                let language = action.language.clone();
                this.models.update(cx, |m, cx| {
                    m.edit(move |s| s.language = language.clone(), cx)
                });
            }))
            .size_full()
            .flex()
            .bg(crate::glass::sidebar_column(&theme, cx))
            .text_color(theme.text)
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(theme.window_bg)
                    .child(
                        div()
                            .flex_none()
                            .h(px(52.))
                            .px(px(28.))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(theme.divider)
                            .window_control_area(WindowControlArea::Drag)
                            .child(
                                a11y::node("settings-title", Role::Heading, self.pane.title())
                                    .text_size(theme.text(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.pane.title()),
                            ),
                    )
                    .child(
                        a11y::node("settings-panel", Role::TabPanel, self.pane.title())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .pt(px(20.))
                            .px(px(28.))
                            .pb(px(28.))
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .when_some(error, |panel, error| {
                                panel.child(
                                    a11y::node("settings-error", Role::Status, error.clone())
                                        .text_size(theme.text(12.))
                                        .text_color(theme.attention)
                                        .child(icons::beside(
                                            Icon::TriangleAlert,
                                            12.,
                                            theme.attention,
                                            theme.text(12.) * 1.4,
                                            error,
                                        )),
                                )
                            })
                            .children(page),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_size_adds_every_file_below() {
        let dir = std::env::temp_dir().join(format!("dory-folder-size-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("takes")).unwrap();
        std::fs::write(dir.join("dory.sqlite"), vec![0u8; 1_000]).unwrap();
        std::fs::write(dir.join("takes/a.wav"), vec![0u8; 2_500]).unwrap();
        assert_eq!(folder_size(&dir).unwrap(), 3_500);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_finds_panes_and_settings_by_name() {
        let (panes, hits) = search("lang");
        assert_eq!(panes, [Pane::Transcription]);
        let labels: Vec<&str> = hits.iter().map(|h| h.label).collect();
        assert_eq!(labels, ["Language", "Meeting languages"]);
        let (panes, hits) = search("STORAGE");
        assert_eq!(panes, [Pane::Storage]);
        assert!(hits.is_empty());
        assert_eq!(search("").0, Pane::ALL);
        assert_eq!(search("zzz"), (Vec::new(), Vec::new()));
    }

    #[test]
    fn panes_round_trip_through_their_slugs() {
        for pane in Pane::ALL {
            assert_eq!(Pane::parse(pane.slug()), Some(pane));
        }
        assert_eq!(Pane::parse("tabs"), None);
    }
}
