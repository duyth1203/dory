//! Set Up Summaries (UX-13, `docs/design/floating-surfaces.md` section 5): a 480 pt modal dialog
//! in two steps, install Ollama and then download the summary model, named "Set Up Summaries"
//! for VoiceOver.
//!
//! - U1: step 1 is active, with Get Ollama…, which opens the website. Dory checks again whenever
//!   the window comes back to the front, and says "Ollama found" when it is. Step 2, dashed,
//!   names the catalog's summary model and its size, and the footer's Download Model waits for
//!   step 1.
//! - U2: the download, with a progress bar, Cancel Download, and Continue in Background. Closing
//!   the dialog leaves the download running, and the waiting recordings are summarized when it
//!   finishes.
//! - U3: summaries are ready.
//!
//! The commands that install Ollama by hand live on in Settings › Summaries (`ollama_steps`).

use dory_core::domain::Blocker;
use dory_core::models::{Llm, ModelId, ModelRef, Resolution};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, FontWeight,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::icons::Icon;
use crate::models_view::{self, Models, button, chosen_pick, progress_bar, size_text};
use crate::processing::{Download, Phase, Problem};
use crate::surface::{self, Look};
use crate::theme::Theme;

/// What the user runs in Terminal, in order, when Ollama is not running.
pub const COMMANDS: [&str; 2] = ["brew install ollama", "ollama serve"];

pub const TITLE: &str = "Set Up Summaries";
pub const WIDTH: f32 = 480.;
pub const OLLAMA_URL: &str = "https://ollama.com/download";
pub const ABOUT_OLLAMA: &str = "Ollama is a free app that runs AI models on your Mac, so Dory can write summaries without sending your meetings anywhere.";

/// Where summaries stand on this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Nothing is known yet.
    Checking,
    /// Ollama does not answer. `model` is the catalog's summary model, when the catalog lists it.
    NotRunning {
        model: Option<Model>,
    },
    Missing {
        model: ModelId<Llm>,
        name: String,
        size: Option<String>,
    },
    Ready {
        name: String,
    },
}

/// A summary model as step 2 names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub name: String,
    pub bytes: Option<u64>,
}

/// The settings' summary choice, resolved the way a job resolves it.
pub fn readiness(models: &Models) -> Readiness {
    let (Some(settings), Some(rows)) = (models.settings(), models.rows::<Llm>()) else {
        return Readiness::Checking;
    };
    let row_of = |model: &ModelRef| rows.iter().find(|r| &r.status.entry.id.erase() == model);
    let named = |model: &ModelRef| {
        row_of(model).map(|row| Model {
            name: row.status.entry.display_name.clone(),
            bytes: row.status.entry.disk_bytes,
        })
    };
    let fallback = || {
        rows.iter()
            .find(|r| r.status.entry.recommended)
            .or(rows.first())
            .map(|row| Model {
                name: row.status.entry.display_name.clone(),
                bytes: row.status.entry.disk_bytes,
            })
    };
    match chosen_pick(rows, settings) {
        Resolution::Use(id) => Readiness::Ready {
            name: row_of(&id.erase())
                .map_or_else(|| id.to_string(), |r| r.status.entry.display_name.clone()),
        },
        Resolution::Blocked(Blocker::NeedsProvision { model, bytes }) => match row_of(&model) {
            Some(row) => {
                let entry = &row.status.entry;
                Readiness::Missing {
                    model: entry.id.clone(),
                    name: entry.display_name.clone(),
                    size: bytes.or(entry.disk_bytes).map(size_text),
                }
            }
            None => Readiness::NotRunning { model: fallback() },
        },
        Resolution::Blocked(Blocker::Unavailable { model, .. }) => Readiness::NotRunning {
            model: named(&model).or_else(fallback),
        },
        Resolution::Blocked(_) | Resolution::Skip => Readiness::NotRunning { model: fallback() },
    }
}

/// The dialog's state, U1 to U3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// U1: Ollama is not found yet.
    Install { model: Option<Model> },
    /// U1 once Ollama is found: Download Model is ready.
    Download {
        model: ModelId<Llm>,
        name: String,
        size: Option<String>,
    },
    /// U2.
    Downloading { name: String, download: Download },
    /// U3. `waiting` names the recording being summarized now, if any.
    Ready { waiting: Option<String> },
}

impl Step {
    pub fn of(readiness: Readiness, download: Option<Download>, waiting: Option<String>) -> Step {
        match readiness {
            Readiness::Checking => Step::Install { model: None },
            Readiness::NotRunning { model } => Step::Install { model },
            Readiness::Missing { model, name, size } => match download {
                Some(download) => Step::Downloading { name, download },
                None => Step::Download { model, name, size },
            },
            Readiness::Ready { .. } => Step::Ready { waiting },
        }
    }

    pub fn subtitle(&self) -> Option<&'static str> {
        match self {
            Step::Install { .. } | Step::Download { .. } => {
                Some("2 short steps. Everything stays on this Mac.")
            }
            Step::Downloading { .. } => Some("Step 2 of 2"),
            Step::Ready { .. } => None,
        }
    }

    /// Step 2's heading: "Download the summary model · Gemma 4 E4B · 9.5 GB".
    pub fn model_heading(&self) -> String {
        let (name, size) = match self {
            Step::Install { model: Some(m) } => (Some(m.name.clone()), m.bytes.map(size_text)),
            Step::Download { name, size, .. } => (Some(name.clone()), size.clone()),
            Step::Downloading { name, download } => {
                (Some(name.clone()), download.total.map(size_text))
            }
            _ => (None, None),
        };
        let mut parts = vec!["Download the summary model".to_owned()];
        parts.extend(name);
        parts.extend(size);
        parts.join(" · ")
    }

    /// U3's sentence.
    pub fn ready_text(&self) -> String {
        match self {
            Step::Ready {
                waiting: Some(title),
            } => format!(
                "{title} is being summarized now. New recordings get a summary automatically."
            ),
            _ => "New recordings get a summary automatically.".into(),
        }
    }

    fn found(&self) -> bool {
        !matches!(self, Step::Install { .. })
    }
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

/// "1.2 of 3.3 GB" under the bar, and its spoken value.
pub fn progress_words(download: Download) -> (String, String) {
    match download.total {
        Some(total) => (
            format!("{} of {} GB", gigabytes(download.done), gigabytes(total)),
            format!(
                "{} of {} gigabytes",
                gigabytes(download.done),
                gigabytes(total)
            ),
        ),
        None => (
            format!("{} GB", gigabytes(download.done)),
            format!("{} gigabytes", gigabytes(download.done)),
        ),
    }
}

/// Each command with its own Copy button.
pub(crate) fn ollama_steps(copy_focus: &[FocusHandle], theme: &Theme) -> gpui_kit::Div {
    let mut steps = div().flex().flex_col().gap(px(8.));
    for (command, focus) in COMMANDS.into_iter().zip(copy_focus) {
        steps = steps.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    a11y::node(
                        format!("command-{command}"),
                        Role::Label,
                        format!("Command: {command}"),
                    )
                    .flex_1()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(theme.field_bg)
                    .font_family("Menlo")
                    .text_size(theme.text(13.))
                    .child(command),
                )
                .child(button(
                    format!("copy-{command}"),
                    format!("Copy {command}"),
                    "Copy",
                    false,
                    focus,
                    theme,
                    move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(command.into())),
                )),
        );
    }
    steps
}

/// The window the dialog opens over: the main window, which draws the dialog layer.
struct MainWindow(AnyWindowHandle);

impl Global for MainWindow {}

/// Called once when the main window opens.
pub fn set_main_window(window: &Window, cx: &mut App) {
    cx.set_global(MainWindow(window.window_handle()));
}

/// Opens the dialog over the main window, wherever it was asked for.
pub fn sheet(window: &mut Window, cx: &mut App) {
    open(None, window, cx);
}

/// The dialog pinned to `step`, for the visual pass and the layout checks.
pub fn preview(step: Step, window: &mut Window, cx: &mut App) {
    open(Some(step), window, cx);
}

fn open(pinned: Option<Step>, window: &mut Window, cx: &mut App) {
    if let Some(main) = cx.try_global::<MainWindow>().map(|m| m.0)
        && main != window.window_handle()
    {
        let _ = main.update(cx, move |_, window, cx| {
            window.activate_window();
            open(pinned, window, cx)
        });
        return;
    }
    let panel = cx.new(|cx| SetUp::new(pinned, window, cx));
    let first = panel.read(cx).first_focus(cx);
    surface::open_dialog(TITLE, WIDTH, panel.into(), &first, window, cx);
}

pub struct SetUp {
    /// `None` when `models_view::install` never ran, so the dialog can only show step 1.
    models: Option<Entity<Models>>,
    pinned: Option<Step>,
    get: FocusHandle,
    not_now: FocusHandle,
    download: FocusHandle,
    cancel: FocusHandle,
    background: FocusHandle,
    done: FocusHandle,
    close: FocusHandle,
    was_found: bool,
    _observe: Option<Subscription>,
    _active: Subscription,
}

impl SetUp {
    fn new(pinned: Option<Step>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let models = models_view::shared(cx);
        let observe = models.as_ref().map(|models| {
            models.update(cx, |m, cx| m.refresh(cx));
            cx.observe(models, |this, _, cx| {
                let found = this.step(cx).found();
                if found && !this.was_found {
                    a11y::announce("Ollama found");
                }
                this.was_found = found;
                cx.notify();
            })
        });
        let active = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active()
                && let Some(models) = &this.models
            {
                models.update(cx, |m, cx| m.refresh(cx));
            }
        });
        let mut this = Self {
            models,
            pinned,
            get: cx.focus_handle(),
            not_now: cx.focus_handle(),
            download: cx.focus_handle(),
            cancel: cx.focus_handle(),
            background: cx.focus_handle(),
            done: cx.focus_handle(),
            close: cx.focus_handle(),
            was_found: false,
            _observe: observe,
            _active: active,
        };
        this.was_found = this.step(cx).found();
        this
    }

    pub fn step(&self, cx: &App) -> Step {
        if let Some(step) = &self.pinned {
            return step.clone();
        }
        let Some(models) = &self.models else {
            return Step::Install { model: None };
        };
        let models = models.read(cx);
        let readiness = readiness(models);
        let app = models.app().read(cx);
        let download = match &readiness {
            Readiness::Missing { model, .. } => app.download_of(&model.erase()),
            _ => None,
        };
        let queue = app.processing_queue();
        let waiting = queue
            .now
            .as_ref()
            .filter(|r| matches!(r.phase, Phase::Summarizing { .. }))
            .map(|r| r.title.clone())
            .or_else(|| {
                queue
                    .attention
                    .iter()
                    .find(|b| b.problem == Problem::SummaryModelMissing)
                    .map(|b| b.title.clone())
            });
        Step::of(readiness, download, waiting)
    }

    /// U1 starts on Get Ollama… (or Download Model once Ollama is found), U2 on Continue in
    /// Background, U3 on Done.
    fn first_focus(&self, cx: &App) -> FocusHandle {
        match self.step(cx) {
            Step::Install { .. } => self.get.clone(),
            Step::Download { .. } => self.download.clone(),
            Step::Downloading { .. } => self.background.clone(),
            Step::Ready { .. } => self.done.clone(),
        }
    }

    fn card(id: &str, label: String, active: bool, dashed: bool, theme: &Theme) -> a11y::Node {
        a11y::node(id.to_owned(), Role::Group, label)
            .p(px(12.))
            .rounded(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(active, |c| surface::inset(c, theme))
            .when(!active, |c| {
                c.border_1()
                    .border_color(theme.inset_border)
                    .when(dashed, |c| c.border_dashed())
            })
    }

    fn heading(text: String, done: bool, theme: &Theme) -> gpui_kit::Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .when(done, |row| {
                row.child(Icon::CircleCheck.view(14., theme.accent_text))
            })
            .child(
                div()
                    .text_size(theme.text(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text),
            )
    }
}

impl Render for SetUp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let step = self.step(cx);
        let note = |text: String| {
            div()
                .text_size(theme.text(12.))
                .text_color(theme.text_secondary)
                .child(text)
        };
        let close_now = |window: &mut Window, cx: &mut App| surface::close_dialog(window, cx);
        let mut body = div()
            .id("set-up-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.));
        let mut footer = Vec::new();
        match &step {
            Step::Install { .. } | Step::Download { .. } => {
                let found = step.found();
                body = body.child(note(ABOUT_OLLAMA.into()));
                let first = if found {
                    Self::card(
                        "set-up-step-1",
                        "Step 1, Ollama is installed".into(),
                        false,
                        false,
                        &theme,
                    )
                    .child(Self::heading(
                        "Ollama is installed".into(),
                        true,
                        &theme,
                    ))
                } else {
                    Self::card(
                        "set-up-step-1",
                        "Step 1, Install Ollama".into(),
                        true,
                        false,
                        &theme,
                    )
                    .child(Self::heading("Install Ollama".into(), false, &theme))
                    .child(note(
                        "Not found on this Mac. Dory checks again when you come back.".into(),
                    ))
                    .child(div().flex().child(surface::button(
                        "set-up-get-ollama",
                        "Get Ollama…",
                        "Get Ollama…",
                        Look::Primary,
                        &self.get,
                        &theme,
                        |_, cx| cx.open_url(OLLAMA_URL),
                    )))
                };
                let second = Self::card(
                    "set-up-step-2",
                    format!("Step 2, {}", step.model_heading()),
                    found,
                    !found,
                    &theme,
                )
                .child(Self::heading(step.model_heading(), false, &theme))
                .when(!found, |c| c.child(note("Available after step 1".into())));
                body = body.child(first).child(second);
                footer.push(surface::button(
                    "set-up-not-now",
                    "Not Now",
                    "Not Now",
                    Look::Secondary,
                    &self.not_now,
                    &theme,
                    close_now,
                ));
                footer.push(match (&step, &self.models) {
                    (Step::Download { model, .. }, Some(models)) => {
                        let (model, models) = (model.clone(), models.clone());
                        surface::button(
                            "set-up-download",
                            "Download Model",
                            "Download Model",
                            Look::Primary,
                            &self.download,
                            &theme,
                            move |_, cx| models.update(cx, |m, cx| m.download(model.clone(), cx)),
                        )
                    }
                    _ => surface::button(
                        "set-up-download",
                        "Download Model, available after Ollama is installed",
                        "Download Model",
                        Look::Disabled,
                        &self.download,
                        &theme,
                        |_, _| {},
                    ),
                });
            }
            Step::Downloading { download, .. } => {
                let (shown, spoken) = progress_words(*download);
                let percent = download.percent().map(|p| p.get());
                let models = self.models.clone();
                let key = models.as_ref().and_then(|m| match readiness(m.read(cx)) {
                    Readiness::Missing { model, .. } => Some(model.erase()),
                    _ => None,
                });
                let cancel = surface::button(
                    "set-up-cancel",
                    "Cancel Download",
                    "Cancel Download",
                    Look::Secondary,
                    &self.cancel,
                    &theme,
                    move |_, cx| {
                        if let (Some(models), Some(key)) = (&models, &key) {
                            models.update(cx, |m, cx| m.cancel(key, cx));
                        }
                    },
                );
                body = body
                    .child(
                        Self::card(
                            "set-up-step-1",
                            "Step 1, Ollama is installed".into(),
                            false,
                            false,
                            &theme,
                        )
                        .child(Self::heading("Ollama is installed".into(), true, &theme)),
                    )
                    .child(
                        Self::card(
                            "set-up-step-2",
                            format!("Step 2, {}", step.model_heading()),
                            true,
                            false,
                            &theme,
                        )
                        .child(Self::heading(step.model_heading(), false, &theme))
                        .child(
                            progress_bar("set-up-progress", "Download progress", percent, &theme)
                                .aria_value(spoken),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex_grow(1.)
                                        .text_size(theme.text(12.))
                                        .text_color(theme.text_secondary)
                                        .font_features(crate::theme::tabular())
                                        .child(shown),
                                )
                                .child(cancel),
                        ),
                    )
                    .child(note(
                        "The download continues if you close this window. Waiting recordings are summarized when it finishes.".into(),
                    ));
                footer.push(surface::button(
                    "set-up-background",
                    "Continue in Background",
                    "Continue in Background",
                    Look::Primary,
                    &self.background,
                    &theme,
                    close_now,
                ));
            }
            Step::Ready { .. } => {
                body = body.child(
                    a11y::node("set-up-ready", Role::Status, "Summaries are ready")
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .py(px(16.))
                        .child(Icon::CircleCheck.view(40., theme.accent_text))
                        .child(
                            div()
                                .text_size(theme.text(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Summaries are ready"),
                        )
                        .child(
                            div()
                                .max_w(px(360.))
                                .text_center()
                                .text_size(theme.text(13.))
                                .text_color(theme.text_secondary)
                                .child(step.ready_text()),
                        ),
                );
                footer.push(surface::button(
                    "set-up-done",
                    "Done",
                    "Done",
                    Look::Primary,
                    &self.done,
                    &theme,
                    close_now,
                ));
            }
        }
        let close = surface::close_button("set-up-close", &self.close, &theme, close_now);
        a11y::node("set-up-summaries", Role::Group, TITLE)
            .flex()
            .flex_col()
            .min_h_0()
            .flex_1()
            .child(surface::header(
                "set-up",
                TITLE,
                step.subtitle().map(str::to_owned),
                Some(close),
                &theme,
            ))
            .child(body)
            .child(surface::footer(None, footer, &theme))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gemma() -> Model {
        Model {
            name: "Gemma 4 E4B".into(),
            bytes: Some(9_461_576_343),
        }
    }

    #[test]
    fn each_step_has_its_subtitle_and_names_the_catalog_model() {
        let install = Step::of(
            Readiness::NotRunning {
                model: Some(gemma()),
            },
            None,
            None,
        );
        assert_eq!(
            install.subtitle(),
            Some("2 short steps. Everything stays on this Mac.")
        );
        assert_eq!(
            install.model_heading(),
            "Download the summary model · Gemma 4 E4B · 9.5 GB"
        );
        assert!(!install.found());
        let provider = dory_core::models::ProviderId::parse("ollama").unwrap();
        let model: ModelId<Llm> = ModelId::new(provider, "gemma4:e4b").unwrap();
        let found = Step::of(
            Readiness::Missing {
                model: model.clone(),
                name: "Gemma 4 E4B".into(),
                size: Some("9.5 GB".into()),
            },
            None,
            None,
        );
        assert!(found.found());
        assert!(matches!(found, Step::Download { .. }));
        let downloading = Step::of(
            Readiness::Missing {
                model,
                name: "Gemma 4 E4B".into(),
                size: None,
            },
            Some(Download {
                done: 1_200_000_000,
                total: Some(3_300_000_000),
            }),
            None,
        );
        assert_eq!(downloading.subtitle(), Some("Step 2 of 2"));
        let ready = Step::of(
            Readiness::Ready {
                name: "Gemma".into(),
            },
            None,
            Some("Hiring debrief".into()),
        );
        assert_eq!(ready.subtitle(), None);
        assert_eq!(
            ready.ready_text(),
            "Hiring debrief is being summarized now. New recordings get a summary automatically."
        );
    }

    #[test]
    fn the_progress_reads_in_gigabytes() {
        let (shown, spoken) = progress_words(Download {
            done: 1_200_000_000,
            total: Some(3_300_000_000),
        });
        assert_eq!(shown, "1.2 of 3.3 GB");
        assert_eq!(spoken, "1.2 of 3.3 gigabytes");
    }
}
