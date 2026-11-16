//! The Set up summaries sheet (UX-13, P6). Summaries need Ollama and a summary model. The sheet
//! checks both and offers the one next step: the commands that install and start Ollama with
//! **Check again**, a one-click download of the model, or word that summaries are ready.

use dory_core::domain::Blocker;
use dory_core::models::{Llm, ModelId, ModelRef, Resolution};
use gpui_kit::component::{WindowExt as _, dialog::DialogFooter};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, IntoElement,
    ParentElement as _, Render, Role, Styled as _, Subscription, Window, div, px,
};

use crate::a11y;
use crate::models_view::{self, Models, button, chosen_pick, note, progress_bar, size_text};
use crate::theme::Theme;

/// What the user runs in Terminal, in order, when Ollama is not running.
pub const COMMANDS: [&str; 2] = ["brew install ollama", "ollama serve"];

pub const EXPLANATION: &str = "Dory writes summaries with Gemma, which runs on this Mac through Ollama. Install and start Ollama in Terminal, then choose Check again.";

/// Where summaries stand on this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Nothing is known yet.
    Checking,
    /// Ollama does not answer, or no summary model is known.
    NotRunning,
    Missing {
        model: ModelId<Llm>,
        name: String,
        size: Option<String>,
    },
    Ready {
        name: String,
    },
}

/// The settings' summary choice, resolved the way a job resolves it.
pub fn readiness(models: &Models) -> Readiness {
    let (Some(settings), Some(rows)) = (models.settings(), models.rows::<Llm>()) else {
        return Readiness::Checking;
    };
    let row_of = |model: &ModelRef| rows.iter().find(|r| &r.status.entry.id.erase() == model);
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
            None => Readiness::NotRunning,
        },
        Resolution::Blocked(_) | Resolution::Skip => Readiness::NotRunning,
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

pub fn sheet(window: &mut Window, cx: &mut App) {
    let panel = cx.new(SetUp::new);
    let done_focus = cx.focus_handle();
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = Theme::get(cx);
        let done = button(
            "set-up-done",
            "Done",
            "Done",
            false,
            &done_focus,
            &theme,
            |window, cx| window.close_dialog(cx),
        );
        dialog
            .title("Set up summaries")
            .w(px(520.))
            .child(panel.clone())
            .footer(DialogFooter::new().child(done))
    });
}

struct SetUp {
    /// `None` when `models_view::install` never ran, so the sheet can only show the steps.
    models: Option<Entity<Models>>,
    copy_focus: Vec<FocusHandle>,
    check_focus: FocusHandle,
    download_focus: FocusHandle,
    cancel_focus: FocusHandle,
    _observe: Option<Subscription>,
}

impl SetUp {
    fn new(cx: &mut Context<Self>) -> Self {
        let models = models_view::shared(cx);
        let observe = models.as_ref().map(|models| {
            models.update(cx, |m, cx| m.refresh(cx));
            cx.observe(models, |_, _, cx| cx.notify())
        });
        Self {
            models,
            copy_focus: COMMANDS.iter().map(|_| cx.focus_handle()).collect(),
            check_focus: cx.focus_handle(),
            download_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            _observe: observe,
        }
    }
}

impl Render for SetUp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let body = a11y::node("set-up-summaries", Role::Group, "Set up summaries")
            .flex()
            .flex_col()
            .gap(px(12.));
        let status = |text: String| {
            a11y::node("set-up-status", Role::Status, text.clone())
                .text_size(theme.text(13.))
                .text_color(theme.text_secondary)
                .child(text)
        };
        let Some(models) = self.models.clone() else {
            return body
                .child(status(EXPLANATION.into()))
                .child(ollama_steps(&self.copy_focus, &theme));
        };
        let state = readiness(models.read(cx));
        let checking = models.read(cx).is_checking();
        match state {
            Readiness::Checking | Readiness::NotRunning => {
                let check = models.clone();
                body.child(status(EXPLANATION.into()))
                    .child(ollama_steps(&self.copy_focus, &theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(button(
                                "set-up-check-again",
                                "Check again",
                                "Check again",
                                true,
                                &self.check_focus,
                                &theme,
                                move |_, cx| check.update(cx, |m, cx| m.refresh(cx)),
                            ))
                            .children(checking.then(|| note("Checking for Ollama", &theme))),
                    )
            }
            Readiness::Missing { model, name, size } => {
                let key = model.erase();
                let download = models.read(cx).app().read(cx).download_of(&key);
                let sized = match &size {
                    Some(size) => format!("{name}, {size}"),
                    None => name.clone(),
                };
                let body = body.child(status(format!(
                    "Ollama is running. Download {sized} to write summaries."
                )));
                match download {
                    None => body.child(div().flex().child(button(
                        "set-up-download",
                        format!("Download {sized}"),
                        format!("Download {name}"),
                        true,
                        &self.download_focus,
                        &theme,
                        move |_, cx| models.update(cx, |m, cx| m.download(model.clone(), cx)),
                    ))),
                    Some(download) => {
                        let percent = download.percent().map(|p| p.get());
                        let words = match percent {
                            Some(p) => format!("Downloading {name}, {p}%"),
                            None => format!("Downloading {name}"),
                        };
                        body.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().child(progress_bar(
                                    "set-up-progress",
                                    words.clone(),
                                    percent,
                                    &theme,
                                )))
                                .child(button(
                                    "set-up-cancel",
                                    format!("Cancel downloading {name}"),
                                    "Cancel",
                                    false,
                                    &self.cancel_focus,
                                    &theme,
                                    move |_, cx| models.update(cx, |m, cx| m.cancel(&key, cx)),
                                )),
                        )
                        .child(note(words, &theme))
                    }
                }
            }
            Readiness::Ready { name } => body.child(status(format!(
                "Summaries are ready. Dory writes them with {name} through Ollama."
            ))),
        }
    }
}
