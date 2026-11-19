//! Window › Processing Queue (UX-15, `docs/design/floating-surfaces.md` section 4 and the
//! operator's decisions): a small floating window, like Activity Monitor's, that remembers where
//! it sat. It is non-modal, so the main window stays usable while jobs run.
//!
//! It takes the dialog family's fill, header, and footer without the scrim: the title and the live
//! summary, then Now (the running job with its stage, a 24 pt mini waveform with the reading
//! head, and Cancel), Up next (the line, with Move to Top, a ⋯ menu, and a drag handle), Needs
//! attention (each blocked or failed recording with its cause and one fix), and Finished this
//! session, and the footer's Processing Settings… and Close. ⌘W also closes it.
//!
//! Each row is one button with one sentence. Choosing it (a click or Return) selects that
//! recording in the main window and leaves this window open. On a focused queued row ⌥↑ and ⌥↓
//! move it, ⌥⌘↑ moves it to the top, and Delete cancels it. Cancel acts at once and offers Undo
//! for 5 s in a toast (⌘Z also undoes). VoiceOver hears only stage changes, finished jobs, and
//! moves. GPUI 0.3 has no accessibility custom actions, so the ⋯ menu carries the moves for
//! VoiceOver users.

use std::collections::HashMap;
use std::time::Instant;

use dory_core::domain::{JobId, RecordingId};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, FontWeight,
    Global, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, ParentElement as _,
    Render, Role, StatefulInteractiveElement as _, Styled as _, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowKind, WindowOptions, canvas, div, point, px, size,
};

use crate::a11y::{self, Node};
use crate::actions::{CloseWindow, QueueCommand, QueueVerb, ShowProcessingQueue, UndoTrash};
use crate::app::AppState;
use crate::glyphs;
use crate::icons::Icon;
use crate::library::Selection;
use crate::motion;
use crate::processing::Phase;
use crate::processing_queue::{self, Ended, Move, Queue};
use crate::surface::{self, Look};
use crate::theme::Theme;

pub const WIDTH: f32 = 560.;
pub const HEIGHT: f32 = 640.;
pub const MIN_HEIGHT: f32 = 320.;
const CONTEXT: &str = "ProcessingWindow";

pub const EMPTY_TITLE: &str = "Nothing is waiting";
pub const EMPTY_TEXT: &str = "When you stop a recording, it waits here while Dory transcribes and summarizes it. You can reorder or cancel jobs, and open the ones that just finished.";

struct Open(AnyWindowHandle, Entity<ProcessingWindow>);

impl Global for Open {}

/// Binds ⌘W to close this window, and ⌘Z to undo a cancel in it. Call once at launch.
pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-w", CloseWindow, Some(CONTEXT)),
        KeyBinding::new("cmd-z", UndoTrash, Some(CONTEXT)),
    ]);
    cx.on_window_closed(|cx, _| restack(cx)).detach();
}

/// Floats the window above the main window while Settings is closed. Settings is a normal
/// window, which never comes above a floating one, so the queue drops to the normal level while
/// Settings is open.
pub fn restack(cx: &mut App) {
    let Some((handle, view)) = cx.try_global::<Open>().map(|open| (open.0, open.1.clone())) else {
        return;
    };
    let floating = crate::settings_window::current_window(cx)
        .is_none_or(|settings| !cx.windows().contains(&settings));
    let _ = handle.update(cx, |_, window, cx| {
        crate::platform::set_floating(window, floating);
        view.update(cx, |view, _| view.floating = floating);
    });
}

/// The open Processing window's view, for tests and the launch hook.
pub fn current(cx: &App) -> Option<Entity<ProcessingWindow>> {
    cx.try_global::<Open>().map(|open| open.1.clone())
}

pub fn current_window(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<Open>().map(|open| open.0)
}

/// Opens the Processing window, or brings it forward when it is open. The menu bar's
/// Processing Queue… runs this while another app is frontmost, which hides this panel.
pub fn open(app: &Entity<AppState>, cx: &mut App) {
    cx.activate(true);
    if let Some(handle) = current_window(cx)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let origin = app.read(cx).prefs.processing_origin;
    let bounds = match origin {
        Some((x, y)) => Bounds::new(point(px(x), px(y)), size(px(WIDTH), px(HEIGHT))),
        None => Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx),
    };
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Processing Queue".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.), px(20.))),
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(420.), px(MIN_HEIGHT))),
        kind: WindowKind::Floating,
        is_minimizable: false,
        ..Default::default()
    };
    let owner = app.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| {
        crate::platform::print_window_id(window);
        cx.new(|cx| ProcessingWindow::new(owner, window, cx))
    }) {
        Ok((handle, view)) => {
            cx.set_global(Open(handle, view));
            restack(cx);
        }
        Err(error) => app.update(cx, |app, cx| {
            app.say(format!("Could not open the Processing window: {error}"), cx)
        }),
    }
}

/// A queued row on its way to a new place, by its handle.
#[derive(Clone)]
struct Dragged {
    job: JobId,
    title: String,
}

struct DragPreview {
    title: String,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        surface::paint(div(), surface::Kind::Menu, &theme)
            .px(px(10.))
            .py(px(4.))
            .text_size(theme.text(13.))
            .child(self.title.clone())
    }
}

pub struct ProcessingWindow {
    app: Entity<AppState>,
    root: FocusHandle,
    close: FocusHandle,
    settings: FocusHandle,
    clear: FocusHandle,
    undo: FocusHandle,
    focus: HashMap<(String, &'static str), FocusHandle>,
    last: Queue,
    /// When the Undo toast appeared, for its entrance.
    toast_at: Option<Instant>,
    /// Above the main window; see [`restack`].
    floating: bool,
    _observe: Subscription,
    _bounds: Subscription,
}

impl ProcessingWindow {
    fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&app, |this, app, cx| {
            let queue = app.read(cx).processing_queue();
            if let Some(said) = processing_queue::announcement(&this.last, &queue) {
                a11y::announce(&said);
            }
            if let Some(running) = &queue.now
                && app.read(cx).waveform_of(&running.recording).is_none()
            {
                let id = running.recording.clone();
                app.update(cx, |app, cx| app.want_waveform(&id, cx));
            }
            let toast = app.read(cx).cancelled().is_some();
            if toast != this.toast_at.is_some() {
                this.toast_at = toast.then(Instant::now);
            }
            this.last = queue;
            cx.notify();
        });
        let bounds = cx.observe_window_bounds(window, |this, window, cx| {
            let origin = window.bounds().origin;
            let at = (f32::from(origin.x), f32::from(origin.y));
            this.app.update(cx, |app, cx| {
                if app.prefs.processing_origin != Some(at) {
                    app.prefs.processing_origin = Some(at);
                    cx.notify();
                }
            });
        });
        let last = app.read(cx).processing_queue();
        if let Some(running) = &last.now {
            let id = running.recording.clone();
            app.update(cx, |app, cx| app.want_waveform(&id, cx));
        }
        let close = cx.focus_handle();
        let this = Self {
            app,
            root: cx.focus_handle(),
            close: close.clone(),
            settings: cx.focus_handle(),
            clear: cx.focus_handle(),
            undo: cx.focus_handle(),
            focus: HashMap::new(),
            last,
            toast_at: None,
            floating: true,
            _observe: observe,
            _bounds: bounds,
        };
        window.focus(&close, cx);
        this
    }

    pub fn queue(&self) -> &Queue {
        &self.last
    }

    /// Whether the window floats above Dory's other windows.
    pub fn floats(&self) -> bool {
        self.floating
    }

    fn focus_for(&mut self, key: String, part: &'static str, cx: &mut App) -> FocusHandle {
        self.focus
            .entry((key, part))
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn close_window(window: &mut Window) {
        window.remove_window();
    }

    /// Selects `id` in the main window, which stays where it is; this window stays open.
    fn choose(app: &Entity<AppState>, id: &RecordingId, cx: &mut App) {
        app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
    }

    fn reorder(&self, job: JobId, to: Move, cx: &mut App) {
        let order = self.last.order();
        let Some(at) = processing_queue::target(&order, job, to) else {
            return;
        };
        let title = self
            .last
            .next
            .iter()
            .find(|w| w.job == job)
            .map(|w| w.title.clone())
            .unwrap_or_default();
        self.app.update(cx, |app, cx| app.move_job(job, at, cx));
        a11y::announce(&processing_queue::moved(&title, at));
    }

    fn command(&mut self, command: &QueueCommand, window: &mut Window, cx: &mut Context<Self>) {
        let job = JobId(command.job);
        match command.verb {
            QueueVerb::MoveToTop => self.reorder(job, Move::Top, cx),
            QueueVerb::MoveUp => self.reorder(job, Move::Up, cx),
            QueueVerb::MoveDown => self.reorder(job, Move::Down, cx),
            QueueVerb::Cancel => self.cancel(job, window, cx),
        }
    }

    fn cancel(&mut self, job: JobId, window: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| app.cancel_job(job, cx));
        // The row goes with the job, so focus waits on Close rather than falling out of the window.
        window.focus(&self.close, cx);
    }

    fn section_title(id: &str, text: String, theme: &Theme) -> Node {
        a11y::node(id.to_owned(), Role::Heading, text.clone())
            .aria_level(2)
            .pt(px(6.))
            .pb(px(6.))
            .text_size(theme.text(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text_secondary)
            .child(text)
    }

    fn now_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Node> {
        let running = self.last.now.clone()?;
        let theme = Theme::get(cx);
        let key = format!("{}", running.job.0);
        let row_focus = self.focus_for(key.clone(), "row", cx);
        let cancel_focus = self.focus_for(key.clone(), "cancel", cx);
        let fast_focus = self.focus_for(key, "fast", cx);
        let app = self.app.clone();
        let id = running.recording.clone();
        let looping = motion::looping(window, cx);
        let at = looping.then(motion::clock_ms);
        if looping {
            window.request_animation_frame();
        }
        let waveform = app.read(cx).waveform_of(&id).unwrap_or_default();
        let done = running.phase.transcribed().get();
        let transcribing = matches!(running.phase, Phase::Transcribing { .. });
        let has_head = transcribing || matches!(running.phase, Phase::Preparing);
        let head = gpui_kit::Hsla {
            a: at.filter(|_| transcribing).map_or(1.0, motion::breathe),
            ..theme.accent
        };
        let (wave_done, wave_rest, outline) = (
            theme.wave_done,
            theme.wave_rest,
            theme.differentiate_without_color,
        );
        let wave = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                crate::player_view::paint_bars(
                    bounds,
                    &waveform,
                    None,
                    &|center| {
                        if center < done {
                            (wave_done, false)
                        } else {
                            (wave_rest, outline)
                        }
                    },
                    has_head.then_some((done, head)),
                    window,
                );
            },
        )
        .size_full();
        let length = running
            .audio
            .map(|audio| format!(" · {} min", (audio.0 + 30_000) / 60_000));
        let cancel = {
            let this = cx.entity();
            let job = running.job;
            surface::button(
                format!("queue-cancel-{}", job.0),
                format!("Cancel {}", running.title),
                "Cancel",
                Look::Secondary,
                &cancel_focus,
                &theme,
                move |window, cx| this.update(cx, |this, cx| this.cancel(job, window, cx)),
            )
        };
        let light = running.phase.light().map(|reason| {
            let app = app.clone();
            let id = id.clone();
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    a11y::node(
                        "queue-light",
                        Role::Label,
                        crate::processing::light_chip(reason),
                    )
                    .px(px(8.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(theme.chip_bg)
                    .text_size(theme.text(12.))
                    .child(crate::processing::light_chip(reason)),
                )
                .child(surface::button(
                    "queue-run-fast",
                    "Use Fast This Time",
                    "Use Fast This Time",
                    Look::Link,
                    &fast_focus,
                    &theme,
                    move |_, cx| app.update(cx, |app, cx| app.run_fast(id.clone(), cx)),
                ))
        });
        let choose = {
            let (app, id) = (app.clone(), id.clone());
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                Self::choose(&app, &id, cx)
            }
        };
        let caption = running.caption();
        Some(
            surface::inset(
                a11y::focusable(
                    a11y::node(
                        format!("queue-job-{}", running.job.0),
                        Role::Button,
                        running.sentence(),
                    ),
                    &row_focus,
                    &theme,
                    false,
                )
                .on_click(choose)
                .cursor_pointer()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(6.)),
                &theme,
            )
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
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(theme.text(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(format!(
                                        "{}{}",
                                        running.title,
                                        length.unwrap_or_default()
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(theme.text(12.))
                                    .text_color(theme.text_secondary)
                                    .font_features(crate::theme::tabular())
                                    .child(glyphs::dots(
                                        theme.accent_text,
                                        12. * theme.text_scale,
                                        at,
                                    ))
                                    .child(running.stage_line()),
                            ),
                    )
                    .child(cancel),
            )
            .child(div().h(px(24.)).mt(px(4.)).child(wave))
            .children(caption.map(|caption| {
                div()
                    .text_size(theme.text(11.))
                    .text_color(theme.text_secondary)
                    .font_features(crate::theme::tabular())
                    .child(caption)
            }))
            .children(light),
        )
    }

    fn next_rows(&mut self, cx: &mut Context<Self>) -> Vec<Node> {
        let theme = Theme::get(cx);
        let of = self.last.next.len();
        let rows = self.last.next.clone();
        let this = cx.entity();
        rows.into_iter()
            .enumerate()
            .map(|(ix, waiting)| {
                let key = format!("{}", waiting.job.0);
                let row_focus = self.focus_for(key.clone(), "row", cx);
                let top_focus = self.focus_for(key.clone(), "top", cx);
                let job = waiting.job;
                let position = ix + 1;
                let (app, id) = (self.app.clone(), waiting.recording.clone());
                let keys = this.clone();
                let on_key = move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                    let m = event.keystroke.modifiers;
                    let to = match event.keystroke.key.as_str() {
                        "up" if m.alt && m.platform => Some(Move::Top),
                        "up" if m.alt => Some(Move::Up),
                        "down" if m.alt => Some(Move::Down),
                        "backspace" | "delete" if !m.alt && !m.platform => {
                            keys.update(cx, |this, cx| this.cancel(job, window, cx));
                            cx.stop_propagation();
                            return;
                        }
                        _ => None,
                    };
                    if let Some(to) = to {
                        keys.update(cx, |this, cx| this.reorder(job, to, cx));
                        cx.stop_propagation();
                    }
                };
                let dragged = Dragged {
                    job,
                    title: waiting.title.clone(),
                };
                let drop_on = this.clone();
                let top = (position > 1).then(|| {
                    let this = this.clone();
                    surface::button(
                        format!("queue-promote-{}", job.0),
                        format!("Move {} to top", waiting.title),
                        "Move to Top",
                        Look::Secondary,
                        &top_focus,
                        &theme,
                        move |_, cx| this.update(cx, |this, cx| this.reorder(job, Move::Top, cx)),
                    )
                });
                let menu_root = self.root.clone();
                let more = Button::new(gpui_kit::SharedString::from(format!(
                    "queue-more-{}",
                    job.0
                )))
                .ghost()
                .compact()
                .accessibility_label(format!("More actions for {}", waiting.title))
                .child(Icon::Ellipsis.view(14., theme.text_secondary))
                .dropdown_menu(move |menu, _, _| {
                    let item = |verb| Box::new(QueueCommand { job: job.0, verb });
                    let mut menu = menu.action_context(menu_root.clone());
                    if position > 1 {
                        menu = menu
                            .menu("Move to Top", item(QueueVerb::MoveToTop))
                            .menu("Move Up", item(QueueVerb::MoveUp));
                    }
                    if position < of {
                        menu = menu.menu("Move Down", item(QueueVerb::MoveDown));
                    }
                    menu.separator().menu("Cancel", item(QueueVerb::Cancel))
                });
                a11y::focusable(
                    a11y::node(
                        format!("queue-job-{}", job.0),
                        Role::Button,
                        waiting.sentence(position, of),
                    ),
                    &row_focus,
                    &theme,
                    false,
                )
                .aria_keyshortcuts("Option-Up Option-Down Option-Command-Up Delete")
                .on_click(move |_, _, cx| Self::choose(&app, &id, cx))
                .on_key_down(on_key)
                .on_drop(move |dragged: &Dragged, _, cx| {
                    drop_on.update(cx, |this, cx| this.reorder(dragged.job, Move::To(ix), cx))
                })
                .drag_over::<Dragged>(move |style, _, _, _| style.bg(theme.selection_inactive))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(44.))
                .px(px(4.))
                .rounded(px(8.))
                .child(
                    div()
                        .id(gpui_kit::SharedString::from(format!(
                            "queue-handle-{}",
                            job.0
                        )))
                        .flex_none()
                        .cursor_grab()
                        .on_drag(dragged, |dragged, _, _, cx| {
                            cx.new(|_| DragPreview {
                                title: dragged.title.clone(),
                            })
                        })
                        .child(Icon::GripVertical.view(14., theme.text_secondary)),
                )
                .child(
                    a11y::probe(format!("queue-badge-{position}"))
                        .flex_none()
                        .min_w(theme.text(22.))
                        .h(theme.text(22.))
                        .px(theme.text(4.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme.surface_inset)
                        .border_1()
                        .border_color(theme.inset_border)
                        .text_size(theme.text(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .font_features(crate::theme::tabular())
                        .child(format!("{position}")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(theme.text(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(waiting.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(theme.text(12.))
                                .text_color(theme.text_secondary)
                                .font_features(crate::theme::tabular())
                                .child(waiting.line()),
                        ),
                )
                .children(top)
                .child(more)
            })
            .collect()
    }

    fn attention_rows(&mut self, cx: &mut Context<Self>) -> Vec<Node> {
        let theme = Theme::get(cx);
        let blocked = self.last.attention.clone();
        blocked
            .into_iter()
            .map(|blocked| {
                let key = blocked.recording.to_string();
                let row_focus = self.focus_for(key.clone(), "row", cx);
                let fix_focus = self.focus_for(key.clone(), "fix", cx);
                let (app, id) = (self.app.clone(), blocked.recording.clone());
                let fix = blocked.problem.fix().zip(blocked.problem.button()).map(
                    |(fix, (visible, spoken))| {
                        let (app, id) = (app.clone(), id.clone());
                        surface::button(
                            format!("queue-fix-{key}"),
                            spoken,
                            visible,
                            Look::Secondary,
                            &fix_focus,
                            &theme,
                            move |window, cx| {
                                crate::player_view::run_fix(&app, &id, &fix, window, cx)
                            },
                        )
                    },
                );
                a11y::focusable(
                    a11y::node(
                        format!("queue-attention-{key}"),
                        Role::Button,
                        blocked.sentence(),
                    ),
                    &row_focus,
                    &theme,
                    false,
                )
                .on_click(move |_, _, cx| Self::choose(&app, &id, cx))
                .cursor_pointer()
                .flex()
                .items_start()
                .gap(px(10.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(theme.surface_inset)
                .border_1()
                .border_color(theme.attention_border)
                .child(
                    div()
                        .flex_none()
                        .pt(px(1.))
                        .child(Icon::TriangleAlert.view(14., theme.attention)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(theme.text(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(blocked.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(theme.text(12.))
                                .text_color(theme.text_secondary)
                                .child(blocked.problem.text()),
                        ),
                )
                .children(fix)
            })
            .collect()
    }

    fn finished_rows(&mut self, cx: &mut Context<Self>) -> Vec<Node> {
        let theme = Theme::get(cx);
        let finished = self.last.finished.clone();
        finished
            .into_iter()
            .map(|finished| {
                let key = format!("{}", finished.job.0);
                let row_focus = self.focus_for(key.clone(), "row", cx);
                let act_focus = self.focus_for(key.clone(), "act", cx);
                let (app, id) = (self.app.clone(), finished.recording.clone());
                let (icon, color, action) = match finished.ended {
                    Ended::Done => (Icon::CircleCheck, theme.accent_text, "Open"),
                    Ended::Cancelled => (Icon::CircleX, theme.text_secondary, "Process Again"),
                };
                let act = {
                    let (app, id) = (app.clone(), id.clone());
                    let ended = finished.ended;
                    surface::button(
                        format!("queue-finished-act-{key}"),
                        format!("{action} {}", finished.title),
                        action,
                        Look::Secondary,
                        &act_focus,
                        &theme,
                        move |_, cx| match ended {
                            Ended::Done => Self::choose(&app, &id, cx),
                            Ended::Cancelled => {
                                app.update(cx, |app, cx| app.try_again(id.clone(), cx))
                            }
                        },
                    )
                };
                a11y::focusable(
                    a11y::node(
                        format!("queue-finished-{key}"),
                        Role::Button,
                        finished.sentence(),
                    ),
                    &row_focus,
                    &theme,
                    false,
                )
                .on_click(move |_, _, cx| Self::choose(&app, &id, cx))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_h(px(36.))
                .px(px(4.))
                .child(icon.view(14., color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap_x(px(8.))
                        .py(px(4.))
                        .child(
                            a11y::probe(format!("queue-finished-title-{key}"))
                                .text_size(theme.text(13.))
                                .truncate()
                                .child(finished.title.clone()),
                        )
                        .child(
                            a11y::probe(format!("queue-finished-line-{key}"))
                                .flex_none()
                                .text_size(theme.text(12.))
                                .text_color(theme.text_secondary)
                                .font_features(crate::theme::tabular())
                                .child(finished.line()),
                        ),
                )
                .child(act)
            })
            .collect()
    }

    fn toast(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let theme = Theme::get(cx);
        let cancelled = self.app.read(cx).cancelled()?.clone();
        let frame = surface::animate(surface::Kind::Toast, self.toast_at, window, cx);
        let (app, close) = (self.app.clone(), self.close.clone());
        let undo = surface::button(
            "queue-undo",
            "Undo cancel",
            "Undo",
            Look::Secondary,
            &self.undo,
            &theme,
            // The toast leaves with the cancel, so focus moves on before it goes.
            move |window, cx| {
                window.focus(&close, cx);
                app.update(cx, |app, cx| app.undo_cancel(cx))
            },
        )
        .aria_keyshortcuts("Command-Z");
        Some(
            div()
                .absolute()
                .bottom(px(28. - frame.travel))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .opacity(frame.opacity)
                .child(
                    crate::record_control::toast(
                        "queue-toast",
                        format!(
                            "{}. Undo, {} seconds left",
                            cancelled.text, cancelled.seconds_left
                        ),
                        cancelled.text.clone(),
                        undo,
                        cancelled.seconds_left,
                        &theme,
                    )
                    .max_w(px(WIDTH - 48.)),
                ),
        )
    }
}

impl Render for ProcessingWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let queue = self.last.clone();
        let mut body = div().px(px(20.)).pb(px(16.)).flex().flex_col().gap(px(8.));
        let nothing = queue.now.is_none() && queue.next.is_empty() && queue.attention.is_empty();
        if nothing && queue.finished.is_empty() {
            body = body.child(
                a11y::node("queue-empty", Role::Group, EMPTY_TITLE)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .py(px(40.))
                    .px(px(20.))
                    .child(
                        a11y::node("queue-empty-title", Role::Heading, EMPTY_TITLE)
                            .aria_level(2)
                            .text_size(theme.text(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(EMPTY_TITLE),
                    )
                    .child(
                        a11y::node("queue-empty-text", Role::Label, EMPTY_TEXT)
                            .max_w(px(380.))
                            .text_center()
                            .text_size(theme.text(13.))
                            .text_color(theme.text_secondary)
                            .child(EMPTY_TEXT),
                    ),
            );
        }
        if let Some(card) = self.now_card(window, cx) {
            body = body
                .child(Self::section_title("queue-now", "Now".into(), &theme))
                .child(card);
        }
        if !queue.next.is_empty() {
            let rows = self.next_rows(cx);
            body = body
                .child(Self::section_title(
                    "queue-next",
                    format!("Up next · {}", queue.next.len()),
                    &theme,
                ))
                .child(
                    a11y::node("queue-next-list", Role::List, "Up next")
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(rows),
                );
        }
        if !queue.attention.is_empty() {
            let rows = self.attention_rows(cx);
            body = body
                .child(Self::section_title(
                    "queue-attention",
                    format!("Needs attention · {}", queue.attention.len()),
                    &theme,
                ))
                .child(
                    a11y::node("queue-attention-list", Role::List, "Needs attention")
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .children(rows),
                );
        }
        if !queue.finished.is_empty() {
            let rows = self.finished_rows(cx);
            let app = self.app.clone();
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(div().flex_1().child(Self::section_title(
                            "queue-finished",
                            format!("Finished this session · {}", queue.finished.len()),
                            &theme,
                        )))
                        .child(surface::button(
                            "queue-clear",
                            "Clear finished",
                            "Clear",
                            Look::Link,
                            &self.clear,
                            &theme,
                            move |_, cx| app.update(cx, |app, cx| app.clear_finished(cx)),
                        )),
                )
                .child(
                    a11y::node("queue-finished-list", Role::List, "Finished this session")
                        .flex()
                        .flex_col()
                        .children(rows),
                );
        }
        let settings = {
            let app = self.app.clone();
            surface::button(
                "queue-settings",
                "Processing Settings…",
                "Processing Settings…",
                Look::Link,
                &self.settings,
                &theme,
                move |_, cx| {
                    crate::settings_window::open_at(
                        &app,
                        crate::settings_window::Pane::Transcription,
                        cx,
                    )
                },
            )
        };
        let close = surface::button(
            "queue-close",
            "Close",
            "Close",
            Look::Primary,
            &self.close,
            &theme,
            |window, _| Self::close_window(window),
        );
        let toast = self.toast(window, cx);
        a11y::node("processing-window", Role::Dialog, "Processing Queue")
            .track_focus(&self.root)
            .key_context(CONTEXT)
            .on_action(|_: &CloseWindow, window, _| Self::close_window(window))
            .on_action(|_: &ShowProcessingQueue, window, _| window.activate_window())
            .on_action(cx.listener(|this, _: &UndoTrash, _, cx| {
                this.app.update(cx, |app, cx| app.undo_cancel(cx))
            }))
            .on_action(cx.listener(|this, command: &QueueCommand, window, cx| {
                this.command(command, window, cx)
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.surface_raised)
            .text_color(theme.text)
            .font_family(".SystemUIFont")
            .child(
                surface::header("queue", "Processing Queue", queue.summary(), None, &theme)
                    .pl(px(92.)),
            )
            .child(surface::scroll_area("queue-body", body, window, cx))
            .child(surface::footer(Some(settings), vec![close], &theme))
            .children(toast)
    }
}
