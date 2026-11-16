//! The window root: direction C's layout, the toolbar (UX-5), the start screen (UX-6), and the
//! window-level handlers for every action in `crate::actions`, so a command works whatever has
//! focus and no view has to know who owns it.
//!
//! Layout: the sidebar is a panel inset 8 pt from the top, left, and bottom edges. To its right,
//! a 52 pt toolbar holds only the record control, and below it the page scrolls one centered
//! column at most 720 pt wide, with 48 pt side padding (24 pt below a 900 pt window).

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    ParentElement as _, Render, Role, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, prelude::FluentBuilder as _, px,
};

use dory_core::ports::capture::Access;

use crate::a11y;
use crate::actions::*;
use crate::app::{AppEvent, AppState, DetailTab};
use crate::detail::DetailPane;
use crate::export::{self, CopyWhat, ExportFormat};
use crate::library::StatusFilter;
use crate::sidebar::Sidebar;
use crate::theme::{self, Theme};

const NARROW_WINDOW: f32 = 900.0;

pub struct Shell {
    app: Entity<AppState>,
    sidebar: Entity<Sidebar>,
    detail: Option<Entity<DetailPane>>,
    start_focus: FocusHandle,
    record_focus: FocusHandle,
    undo_focus: FocusHandle,
    grant_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(app.clone(), window, cx));
        let on_select = cx.subscribe_in(&app, window, |this, app, event: &AppEvent, window, cx| {
            if *event == AppEvent::SelectionChanged {
                this.detail = app.read(cx).session().cloned().map(|session| {
                    let app = app.clone();
                    cx.new(|cx| DetailPane::new(app, session, window, cx))
                });
                cx.notify();
            }
        });
        let on_app = cx.observe(&app, |_, _, cx| cx.notify());
        let on_appearance = cx.observe_window_appearance(window, |_, window, cx| {
            theme::sync(window, cx);
        });
        let start_focus = cx.focus_handle();
        // Nothing has focus at launch otherwise, and Tab does nothing (spike finding 4).
        window.focus(&start_focus, cx);
        Self {
            app,
            sidebar,
            detail: None,
            start_focus,
            record_focus: cx.focus_handle(),
            undo_focus: cx.focus_handle(),
            grant_focus: cx.focus_handle(),
            _subscriptions: vec![on_select, on_app, on_appearance],
        }
    }

    fn toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        a11y::node("toolbar", Role::Toolbar, "Toolbar")
            .h(px(52.))
            .flex_none()
            .flex()
            .items_center()
            .px(px(16.))
            .child(
                a11y::focusable(
                    a11y::node("record", Role::Button, "Start recording"),
                    &self.record_focus,
                    theme,
                    false,
                )
                .aria_keyshortcuts("Command-Shift-R")
                .on_click(move |_, _, cx| app.update(cx, |app, cx| app.start_recording(cx)))
                .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.app.update(cx, |app, cx| app.start_recording(cx));
                        cx.stop_propagation();
                    }
                }))
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(14.))
                .bg(theme.control_bg)
                .shadow_xs()
                .text_size(theme.text(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(div().size(px(10.)).rounded_full().bg(theme.record_red))
                .child("Record"),
            )
    }

    fn start_screen(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let line = if app.is_loaded() && app.recordings.is_empty() && app.search.is_empty() {
            "Record a call, and Dory writes its transcript and summary on this Mac."
        } else {
            "Pick a recording on the left, or start a new one."
        };
        let sources = app.sources.clone();
        let record = self.app.clone();
        let keycap = |key: &'static str| {
            div()
                .min_w(px(22.))
                .h(px(22.))
                .px(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(theme.control_bg)
                .border_1()
                .border_color(theme.control_border)
                .text_size(theme.text(12.))
                .child(key)
        };
        let source_row = |name: &'static str, device: Option<String>, access: Option<Access>| {
            let (status, ready) = match access {
                Some(Access::Granted) => ("Ready", true),
                Some(Access::Undetermined) => ("Asked when you record", true),
                Some(Access::Denied) => ("Not allowed", false),
                Some(Access::Unsupported) => ("Not available on this Mac", false),
                None => ("Checking", true),
            };
            let label = match &device {
                Some(device) => format!("{name}, {device}, {status}"),
                None => format!("{name}, {status}"),
            };
            a11y::node(format!("source-{name}"), Role::ListItem, label)
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(8.))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().text_size(theme.text(13.)).child(name))
                        .children(device.map(|d| {
                            div()
                                .text_size(theme.text(12.))
                                .text_color(theme.text_secondary)
                                .child(d)
                        })),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .text_size(theme.text(12.))
                        .text_color(if ready {
                            theme.text_secondary
                        } else {
                            theme.attention
                        })
                        .child(if ready { "✓" } else { "▲" })
                        .child(status),
                )
                .when(!ready && access == Some(Access::Denied), |row| {
                    row.child(
                        a11y::focusable(
                            a11y::node(
                                format!("grant-{name}"),
                                Role::Button,
                                format!("Grant {name}"),
                            ),
                            &self.grant_focus,
                            theme,
                            false,
                        )
                        .px(px(10.))
                        .rounded(px(6.))
                        .bg(theme.control_bg)
                        .on_click(|_, _, _| eprintln!("grant: onboarding (P6) wires this"))
                        .child("Grant"),
                    )
                })
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(20.))
            .pt(px(96.))
            .child(
                a11y::focusable(
                    a11y::node("start-recording", Role::Button, "Start Recording"),
                    &self.start_focus,
                    theme,
                    false,
                )
                .aria_keyshortcuts("Command-Shift-R")
                .on_click(move |_, _, cx| record.update(cx, |app, cx| app.start_recording(cx)))
                .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.app.update(cx, |app, cx| app.start_recording(cx));
                        cx.stop_propagation();
                    }
                }))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.))
                .p(px(12.))
                .rounded(px(16.))
                .child(
                    div()
                        .size(px(80.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme.control_bg)
                        .border_1()
                        .border_color(theme.control_border)
                        .child(div().size(px(30.)).rounded_full().bg(theme.record_red)),
                )
                .child(
                    div()
                        .text_size(theme.text(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Start Recording"),
                ),
            )
            .child(
                div()
                    .text_size(theme.text(15.))
                    .text_color(theme.text_secondary)
                    .child(line),
            )
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .child(keycap("⇧"))
                    .child(keycap("⌘"))
                    .child(keycap("R")),
            )
            .child(
                a11y::node("sources", Role::List, "Sources")
                    .w(px(360.))
                    .flex()
                    .flex_col()
                    .px(px(16.))
                    .py(px(8.))
                    .rounded(px(14.))
                    .bg(theme.card_bg)
                    .border_1()
                    .border_color(theme.card_border)
                    .child(source_row(
                        "Microphone",
                        sources.as_ref().and_then(|s| s.microphone_name.clone()),
                        sources.as_ref().map(|s| s.microphone),
                    ))
                    .child(source_row(
                        "System audio",
                        None,
                        sources.as_ref().map(|s| s.system_audio),
                    )),
            )
    }

    fn with_actions<E: InteractiveElement>(&self, el: E, cx: &mut Context<Self>) -> E {
        el.on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &NewRecording, _, cx| {
                this.app.update(cx, |app, cx| app.start_recording(cx))
            }))
            .on_action(cx.listener(|this, _: &Find, window, cx| {
                this.sidebar
                    .update(cx, |sidebar, cx| sidebar.focus_search(window, cx))
            }))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| {
                this.app.update(cx, |app, cx| app.select_relative(1, cx))
            }))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| {
                this.app.update(cx, |app, cx| app.select_relative(-1, cx))
            }))
            .on_action(cx.listener(|this, _: &ShowSummary, _, cx| {
                this.app
                    .update(cx, |app, cx| app.set_tab(DetailTab::Summary, cx))
            }))
            .on_action(cx.listener(|this, _: &ShowTranscript, _, cx| {
                this.app
                    .update(cx, |app, cx| app.set_tab(DetailTab::Transcript, cx))
            }))
            .on_action(cx.listener(|this, _: &TogglePlayback, _, cx| {
                if let Some(session) = this.app.read(cx).session().cloned() {
                    let player = session.read(cx).player().clone();
                    player.update(cx, |player, cx| player.toggle(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &CopySummary, _, cx| {
                if let Some(detail) = this.selected_detail(cx) {
                    export::copy(&detail, CopyWhat::Summary, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyTranscript, _, cx| {
                if let Some(detail) = this.selected_detail(cx) {
                    export::copy(&detail, CopyWhat::Transcript, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExportMarkdown, _, cx| {
                this.export(ExportFormat::Markdown, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ExportText, _, cx| this.export(ExportFormat::Text, cx)),
            )
            .on_action(cx.listener(|this, _: &ExportSrt, _, cx| this.export(ExportFormat::Srt, cx)))
            .on_action(cx.listener(|this, _: &MoveToTrash, _, cx| {
                this.app.update(cx, |app, cx| app.delete_selection(cx))
            }))
            .on_action(cx.listener(|this, _: &UndoTrash, _, cx| {
                this.app.update(cx, |app, cx| app.undo_delete(cx))
            }))
            .on_action(cx.listener(|this, _: &FilterAll, _, cx| {
                this.app
                    .update(cx, |app, cx| app.set_filter(StatusFilter::All, cx))
            }))
            .on_action(cx.listener(|this, _: &FilterProcessing, _, cx| {
                this.app
                    .update(cx, |app, cx| app.set_filter(StatusFilter::Processing, cx))
            }))
            .on_action(cx.listener(|this, _: &FilterNeedsAttention, _, cx| {
                this.app.update(cx, |app, cx| {
                    app.set_filter(StatusFilter::NeedsAttention, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &FilterDone, _, cx| {
                this.app
                    .update(cx, |app, cx| app.set_filter(StatusFilter::Done, cx))
            }))
    }

    fn selected_detail(&self, cx: &Context<Self>) -> Option<std::sync::Arc<crate::app::Detail>> {
        self.app.read(cx).session()?.read(cx).detail()
    }

    fn export(&self, format: ExportFormat, cx: &mut Context<Self>) {
        if let Some(detail) = self.selected_detail(cx) {
            export::export(&self.app, detail, format, cx);
        }
    }

    fn status_line(&self, theme: &Theme, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let app = self.app.read(cx);
        let notice = app.notice()?;
        let (text, undo) = (notice.text.clone(), notice.undo);
        let target = self.app.clone();
        Some(
            a11y::node("status", Role::Status, text.clone())
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(32.))
                .px(px(16.))
                .text_size(theme.text(13.))
                .text_color(theme.text_secondary)
                .child(text)
                .when(undo, |line| {
                    line.child(
                        a11y::focusable(
                            a11y::node("undo", Role::Button, "Undo"),
                            &self.undo_focus,
                            theme,
                            false,
                        )
                        .aria_keyshortcuts("Command-Z")
                        .on_click(move |_, _, cx| target.update(cx, |app, cx| app.undo_delete(cx)))
                        .px(px(8.))
                        .rounded(px(6.))
                        .text_color(theme.accent_text)
                        .child("Undo"),
                    )
                }),
        )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let narrow = window.viewport_size().width < px(NARROW_WINDOW);
        let side_padding = if narrow { 24. } else { 48. };
        let status = self.status_line(&theme, cx);
        let page = match &self.detail {
            Some(detail) => detail.clone().into_any_element(),
            None => self.start_screen(&theme, cx).into_any_element(),
        };
        let root = div()
            .id("dory")
            .size_full()
            .flex()
            .bg(theme.window_bg)
            .text_color(theme.text)
            .font_family(".SystemUIFont")
            .child(
                div()
                    .h_full()
                    .flex_none()
                    .pl(px(8.))
                    .py(px(8.))
                    .child(self.sidebar.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.toolbar(&theme, cx))
                    .child(
                        a11y::node("detail", Role::Main, "Recording")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .justify_center()
                            .items_start()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(720. + 2. * side_padding))
                                    .px(px(side_padding))
                                    .pb(px(32.))
                                    .child(page),
                            ),
                    )
                    .children(status),
            );
        self.with_actions(root, cx)
    }
}
