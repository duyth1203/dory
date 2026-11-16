//! The Settings window (UX-19, UX-20): a separate native window with five tabs. Each option
//! names its effect in one line. Appearance follows the system, so there is no theme option.
//!
//! - General: process recordings automatically.
//! - Recording: the Mac's inputs, read-only. No setting carries a microphone or a layout yet;
//!   every take starts with `StartOptions::default()`, the system default input plus system audio.
//! - Transcription: language (UX-26), meeting languages (UX-29), the speech models (UX-28), and
//!   Processing (UX-30). **Add model file...** stays disabled: no speech provider accepts a
//!   user's file today.
//! - Summaries: the summary models, and the Ollama steps when Ollama does not answer.
//! - Storage: the data folder, its size, and Show in Finder, then the old app's WebM files and
//!   **Move original WebM files to Trash** with Undo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dory_core::domain::{LangTag, LanguagePref};
use dory_core::models::{Availability, Llm, Stt, UnavailableReason};
use dory_core::ports::capture::InputDevice;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, FontWeight,
    Global, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};

use crate::a11y;
use crate::app::{AppState, Originals};
use crate::models_view::{
    self, LanguagePicker, Mark, ModelList, Models, button, language_name, note, option, size_text,
};
use crate::processing::PACE_CHOICES;
use crate::set_up_summaries::{COMMANDS, ollama_steps};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    General,
    Recording,
    Transcription,
    Summaries,
    Storage,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::General,
        Tab::Recording,
        Tab::Transcription,
        Tab::Summaries,
        Tab::Storage,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Recording => "Recording",
            Tab::Transcription => "Transcription",
            Tab::Summaries => "Summaries",
            Tab::Storage => "Storage",
        }
    }

    pub fn id(self) -> String {
        format!("settings-tab-{}", self.title().to_lowercase())
    }
}

pub const ADD_MODEL_FILE_REASON: &str =
    "No speech engine in Dory can load a model file yet, so models come from the list above.";

struct Open(AnyWindowHandle);

impl Global for Open {}

/// Opens the Settings window, or brings it forward when it is already open.
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
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(640.), px(620.)),
            cx,
        ))),
        ..Default::default()
    };
    let owner = app.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| SettingsWindow::new(owner, window, cx))
    }) {
        Ok((handle, _)) => cx.set_global(Open(handle)),
        Err(error) => app.update(cx, |app, cx| {
            app.say(format!("Could not open Settings: {error}"), cx)
        }),
    }
}

/// The data folder's path and size, read when the Storage tab first shows.
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
    tab: Tab,
    tab_focus: HashMap<Tab, FocusHandle>,
    focus: HashMap<String, FocusHandle>,
    speech: Entity<ModelList<Stt>>,
    summaries: Entity<ModelList<Llm>>,
    languages: Entity<LanguagePicker>,
    devices: Option<Result<Vec<InputDevice>, String>>,
    storage: Storage,
    copy_focus: Vec<FocusHandle>,
    _observe: Subscription,
}

impl SettingsWindow {
    fn new(app: Entity<AppState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
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
        Self {
            speech: cx.new(|cx| ModelList::new(models.clone(), cx)),
            summaries: cx.new(|cx| ModelList::new(models.clone(), cx)),
            languages: cx.new(|cx| LanguagePicker::new(models.clone(), cx)),
            tab_focus: Tab::ALL.iter().map(|t| (*t, cx.focus_handle())).collect(),
            focus: HashMap::new(),
            copy_focus: COMMANDS.iter().map(|_| cx.focus_handle()).collect(),
            app,
            models,
            tab: Tab::General,
            devices: None,
            storage: Storage::Unread,
            _observe: observe,
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn select(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if tab == Tab::Storage && self.tab != tab {
            self.storage = Storage::Unread;
        }
        self.tab = tab;
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

    fn general(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::Div {
        let on = self
            .models
            .read(cx)
            .settings()
            .map_or_else(|| self.app.read(cx).auto_process(), |s| s.auto_process);
        let focus = self.focus("auto-process", cx);
        let models = self.models.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section(
                "settings-processing",
                "Processing",
                theme,
                div().child(option(
                    "setting-auto-process",
                    Mark::Check(on),
                    "Process recordings automatically",
                    "Transcribes and summarizes each recording as soon as it is saved.",
                    &focus,
                    theme,
                    move |_, cx| {
                        models.update(cx, |m, cx| m.edit(move |s| s.auto_process = !on, cx))
                    },
                )),
            ))
            .child(section(
                "settings-appearance",
                "Appearance",
                theme,
                note(
                    "Dory follows the system's light or dark appearance and its accent color.",
                    theme,
                ),
            ))
    }

    fn recording(&mut self, theme: &Theme) -> gpui_kit::Div {
        let devices = match &self.devices {
            None => note("Looking for microphones", theme),
            Some(Err(error)) => note(format!("Could not list microphones: {error}"), theme),
            Some(Ok(devices)) => {
                let mut list = a11y::node("settings-microphones", Role::List, "Microphones")
                    .flex()
                    .flex_col()
                    .gap(px(2.));
                for device in devices {
                    let text = if device.is_default {
                        format!("{}, system default", device.name)
                    } else {
                        device.name.clone()
                    };
                    list = list.child(
                        a11y::node(
                            format!("microphone-{}", device.uid),
                            Role::ListItem,
                            text.clone(),
                        )
                        .text_size(theme.text(13.))
                        .child(if device.is_default {
                            format!("● {text}")
                        } else {
                            format!("○ {text}")
                        }),
                    );
                }
                div().child(list)
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section(
                "settings-microphone",
                "Microphone",
                theme,
                div().flex().flex_col().gap(px(6.)).child(devices).child(note(
                    "Dory records from the system's default input and follows it when it changes. Pick another one in System Settings > Sound.",
                    theme,
                )),
            ))
            .child(section(
                "settings-layout",
                "Channels",
                theme,
                note(
                    "Dory records your microphone and the Mac's audio on separate channels, so it can tell you apart from the people on the call.",
                    theme,
                ),
            ))
    }

    fn transcription(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::Div {
        let settings = self.models.read(cx).settings().cloned();
        let mut languages = div().flex().flex_col().gap(px(2.));
        if let Some(settings) = settings {
            let mut pinnable: Vec<LangTag> = settings.meeting_languages.clone();
            if let LanguagePref::Pinned(tag) = &settings.language
                && !pinnable.contains(tag)
            {
                pinnable.push(tag.clone());
            }
            let choices = std::iter::once(LanguagePref::Auto)
                .chain(pinnable.into_iter().map(LanguagePref::Pinned));
            for choice in choices {
                let (id, label, effect) = match &choice {
                    LanguagePref::Auto => (
                        "language-auto".to_owned(),
                        "Automatic".to_owned(),
                        "Detects the language and follows switches mid-meeting.".to_owned(),
                    ),
                    LanguagePref::Pinned(tag) => {
                        let name = language_name(tag);
                        (
                            format!("language-{tag}"),
                            format!("Always {name}"),
                            format!("Transcribes every meeting as {name}, with no detection."),
                        )
                    }
                };
                let focus = self.focus(id.clone(), cx);
                let models = self.models.clone();
                let on = settings.language == choice;
                languages = languages.child(option(
                    id,
                    Mark::Radio(on),
                    label,
                    effect,
                    &focus,
                    theme,
                    move |_, cx| {
                        let choice = choice.clone();
                        models.update(cx, |m, cx| m.edit(move |s| s.language = choice.clone(), cx))
                    },
                ));
            }
            if settings.meeting_languages.is_empty() {
                languages = languages.child(note(
                    "Add meeting languages below to pin one of them.",
                    theme,
                ));
            }
        }
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
        let mut paces = div().flex().flex_col().gap(px(2.));
        if let Some(settings) = self.models.read(cx).settings().cloned() {
            for choice in &PACE_CHOICES {
                let focus = self.focus(choice.id, cx);
                let models = self.models.clone();
                let setting = choice.setting;
                paces = paces.child(
                    option(
                        choice.id,
                        Mark::Radio(settings.processing == setting),
                        choice.label,
                        choice.effect,
                        &focus,
                        theme,
                        move |_, cx| {
                            models.update(cx, |m, cx| m.edit(move |s| s.processing = setting, cx))
                        },
                    )
                    .aria_label(format!("{}. {}", choice.label, choice.effect)),
                );
            }
            paces = paces.child(note(
                "Light limits both transcription and summaries.",
                theme,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section("settings-language", "Language", theme, languages))
            .child(section(
                "settings-meeting-languages",
                "Meeting languages",
                theme,
                div().child(self.languages.clone()),
            ))
            .child(section(
                "settings-speech-models",
                "Speech model",
                theme,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(self.speech.clone())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(add_file)
                            .child(note(ADD_MODEL_FILE_REASON, theme).flex_1()),
                    ),
            ))
            .child(section("settings-pace", "Processing", theme, paces))
    }

    fn summaries(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::Div {
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
        let mut page = div().flex().flex_col().gap(px(16.));
        if unreachable {
            let focus = self.focus("check-again", cx);
            let models = self.models.clone();
            page = page.child(section(
                "settings-ollama",
                "Ollama is not running",
                theme,
                div()
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
                    ),
            ));
        }
        page.child(section(
            "settings-summary-models",
            "Summary model",
            theme,
            div().child(self.summaries.clone()),
        ))
    }

    fn storage(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::Div {
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
        let mut body = div().flex().flex_col().gap(px(8.));
        if let Some(path) = path {
            let shown = path.display().to_string();
            let focus = self.focus("show-in-finder", cx);
            body = body
                .child(
                    a11y::node("data-folder", Role::Label, format!("Data folder: {shown}"))
                        .text_size(theme.text(13.))
                        .font_family("Menlo")
                        .child(shown),
                )
                .child(
                    a11y::node("data-folder-size", Role::Label, size.clone())
                        .text_size(theme.text(12.))
                        .text_color(theme.text_secondary)
                        .child(size),
                )
                .child(div().flex().child(button(
                    "show-in-finder",
                    "Show in Finder, the data folder",
                    "Show in Finder",
                    false,
                    &focus,
                    theme,
                    move |_, cx| cx.reveal_path(&path),
                )));
        } else {
            body = body.child(note(size, theme));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section(
                "settings-storage",
                "Data folder",
                theme,
                body.child(note(
                    "Recordings, transcripts, summaries, and downloaded speech models live here.",
                    theme,
                )),
            ))
            .children(self.originals(theme, cx))
    }

    /// The old app's WebM files (moved here from P8): what they cost, the one action that moves
    /// them to the Trash, and Undo. Hidden when there are none.
    fn originals(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<a11y::Node> {
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
            Originals::Failed(error) => (format!("⚠︎ {error}"), None),
        };
        Some(section(
            "settings-originals",
            "Files from the old app",
            theme,
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    a11y::node("originals-status", Role::Status, line.clone())
                        .text_size(theme.text(13.))
                        .child(line),
                )
                .children(action.map(|action| div().flex().child(action)))
                .child(note(
                    "Nothing is deleted. The Trash keeps the files until you empty it.",
                    theme,
                )),
        ))
    }
}

fn section(
    id: &'static str,
    title: &'static str,
    theme: &Theme,
    body: impl IntoElement,
) -> a11y::Node {
    a11y::node(id, Role::Group, title)
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            a11y::node(format!("{id}-title"), Role::Heading, title)
                .text_size(theme.text(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(body)
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let this = cx.entity().downgrade();
        let mut tabs = a11y::node("settings-tabs", Role::TabList, "Settings")
            .flex()
            .justify_center()
            .gap(px(4.))
            .pt(px(12.))
            .pb(px(8.))
            .border_b_1()
            .border_color(theme.divider);
        for tab in Tab::ALL {
            let this = this.clone();
            tabs = tabs.child(
                a11y::tab(
                    tab.id(),
                    tab.title(),
                    tab == self.tab,
                    &self.tab_focus[&tab],
                    &theme,
                    move |_, cx| {
                        let _ = this.update(cx, |w, cx| w.select(tab, cx));
                    },
                )
                .flex(),
            );
        }
        let page = match self.tab {
            Tab::General => self.general(&theme, cx),
            Tab::Recording => self.recording(&theme),
            Tab::Transcription => self.transcription(&theme, cx),
            Tab::Summaries => self.summaries(&theme, cx),
            Tab::Storage => self.storage(&theme, cx),
        };
        let error = self.models.read(cx).error().cloned();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .text_color(theme.text)
            .child(tabs)
            .child(
                a11y::node("settings-panel", Role::TabPanel, self.tab.title())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(24.))
                    .py(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .when_some(error, |panel, error| {
                        panel.child(
                            a11y::node("settings-error", Role::Status, error.clone())
                                .text_size(theme.text(12.))
                                .text_color(theme.attention)
                                .child(format!("⚠︎ {error}")),
                        )
                    })
                    .child(page),
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
}
