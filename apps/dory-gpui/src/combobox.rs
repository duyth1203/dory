//! The meeting-languages combobox (UX-29, `docs/design/settings-icons-blur.md` section 4.3):
//! chips in a field, and a popover that searches every language Dory knows by its English name,
//! its own name, or its code. Settings > Transcription and onboarding's Models step both show it.
//!
//! Keys in the popover's search: Up and Down move the highlight and wrap, Return adds or removes
//! the highlighted language and keeps the popover open, Delete in an empty search removes the
//! last chip, and Escape, Tab, or a click outside closes it and returns focus to the field. The
//! search field is Kit's, so the popover catches those keys in the capture phase, before the field
//! turns them into cursor moves.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use dory_core::domain::LangTag;
use gpui_kit::component::input::{
    Backspace, Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp,
    OutdentInline,
};
use gpui_kit::{
    Anchor, App, AppContext as _, Bounds, BoxShadow, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render, Role,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    anchored, canvas, deferred, div, point, prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::icons::Icon;
use crate::languages::{self, Edit, KEEP_ONE};
use crate::models_view::Models;
use crate::theme::Theme;

pub const HINT: &str = "Return adds or removes · Esc closes · Delete removes the last chip";
/// The list shows this many rows, then scrolls.
pub const VISIBLE_ROWS: f32 = 8.;
const ROW: f32 = 28.;
/// The popover's clearance from the window's edges.
const MARGIN: f32 = 8.;
/// The gap between the field and the popover.
const GAP: f32 = 6.;
/// What the popover holds besides its rows: the search field, the hint, and the padding between.
const CHROME: f32 = 88.;

/// How the field shows keyboard focus. Its 1 pt border takes `color` and a band `halo` wide of the
/// same color is drawn outside it, as Kit's own fields draw theirs. Recoloring the border alone
/// is not enough: under Increase Contrast the resting border (`text_secondary`) and the ring
/// (`text`) are about 1.4 to 1, and the field looked the same focused or not.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusEdge {
    pub color: gpui_kit::Hsla,
    pub halo: Pixels,
}

pub fn focus_edge(theme: &Theme) -> FocusEdge {
    FocusEdge {
        color: theme.ring,
        halo: px(2.),
    }
}

/// Which side of its field the popover opens on, and the tallest it may be there.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    above: bool,
    max_h: Pixels,
}

impl Placement {
    /// Below the field when the popover's full height fits there, as it does in a roomy window.
    /// Otherwise on the side with more room, so it never covers the field. Its list gives up rows
    /// to fit that room.
    fn of(field: Bounds<Pixels>, window_h: Pixels, want: Pixels) -> Placement {
        let gap = px(GAP + MARGIN);
        let below = window_h - field.bottom() - gap;
        let above = field.top() - gap;
        let up = below < want && above > below;
        Placement {
            above: up,
            max_h: (if up { above } else { below }).max(px(0.)),
        }
    }
}

pub struct LanguageCombobox {
    models: Entity<Models>,
    search: Entity<InputState>,
    field: FocusHandle,
    add: FocusHandle,
    remove: HashMap<LangTag, FocusHandle>,
    open: bool,
    highlight: Option<usize>,
    /// The list's scroll area, and the rows inside it, which sit one level down.
    scroll: ScrollHandle,
    rows: ScrollHandle,
    /// The highlight moved, so the next frame scrolls its row into view once the rows are laid out.
    reveal: bool,
    /// The field's bounds in the window, as of its last layout.
    field_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The last thing VoiceOver was told, kept as a status line so it can be read again.
    said: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl LanguageCombobox {
    pub fn new(models: Entity<Models>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search languages"));
        a11y::register_text_field(&search, cx);
        let on_type = cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = search.read(cx).value();
                this.highlight = languages::search(&query).best;
                this.scroll_to_highlight();
                cx.notify();
            }
        });
        let on_models = cx.observe(&models, |_, _, cx| cx.notify());
        if models.read(cx).settings().is_none() {
            models.update(cx, |m, cx| m.refresh(cx));
        }
        Self {
            models,
            search,
            field: cx.focus_handle(),
            add: cx.focus_handle(),
            remove: HashMap::new(),
            open: false,
            highlight: None,
            scroll: ScrollHandle::new(),
            rows: ScrollHandle::new(),
            reveal: false,
            field_bounds: Rc::new(Cell::new(Bounds::new(
                Default::default(),
                gpui_kit::size(px(360.), px(36.)),
            ))),
            said: None,
            _subscriptions: vec![on_type, on_models],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn query(&self, cx: &App) -> SharedString {
        self.search.read(cx).value()
    }

    fn scroll_to_highlight(&mut self) {
        self.reveal = self.highlight.is_some();
    }

    /// Scrolls the list the least that shows the highlighted row, from the rows' last layout.
    fn reveal_highlight(&self) {
        let Some(row) = self.highlight.and_then(|ix| self.rows.bounds_for_item(ix)) else {
            return;
        };
        let view = self.scroll.bounds();
        let mut offset = self.scroll.offset();
        if row.top() < view.top() {
            offset.y += view.top() - row.top();
        } else if row.bottom() > view.bottom() {
            offset.y -= row.bottom() - view.bottom();
        } else {
            return;
        }
        self.scroll.set_offset(offset);
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.open = true;
            self.highlight = languages::search(&self.query(cx)).best;
        }
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        window.focus(&self.field, cx);
        cx.notify();
    }

    fn say(&mut self, text: String) {
        a11y::announce(&text);
        self.said = Some(text.into());
    }

    fn toggle(&mut self, tag: LangTag, cx: &mut Context<Self>) -> Option<Edit> {
        let name = languages::name(&tag);
        let edit = self.models.update(cx, |m, cx| m.toggle_language(tag, cx))?;
        match &edit {
            Edit::Added => self.say(format!("{name} added")),
            Edit::Removed { .. } => self.say(format!("{name} removed")),
            Edit::KeptLast => self.say(KEEP_ONE.into()),
            Edit::Confirmed => self.say(format!("{name} chosen")),
        }
        cx.notify();
        Some(edit)
    }

    /// Return or a click on a row: toggles it, clears the search, and keeps the highlight on the
    /// same language in the full list.
    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let rows = languages::search(&self.query(cx)).rows;
        let Some(lang) = rows.get(ix).copied() else {
            return;
        };
        let Ok(tag) = LangTag::parse(lang.code) else {
            return;
        };
        self.toggle(tag, cx);
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.highlight = languages::search("")
            .rows
            .iter()
            .position(|l| l.code == lang.code);
        self.scroll_to_highlight();
        cx.notify();
    }

    fn remove_last(&mut self, cx: &mut Context<Self>) {
        let last = self
            .models
            .read(cx)
            .settings()
            .and_then(|s| s.meeting_languages.last().cloned());
        if let Some(tag) = last {
            self.toggle(tag, cx);
        }
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = languages::search(&self.query(cx)).rows.len() as isize;
        if count == 0 {
            return;
        }
        let at = self.highlight.map_or(-1, |ix| ix as isize);
        let next = if at < 0 && delta < 0 {
            count - 1
        } else {
            (at + delta).rem_euclid(count)
        };
        self.highlight = Some(next as usize);
        self.scroll_to_highlight();
        cx.notify();
    }

    fn remove_focus(&mut self, tag: &LangTag, cx: &mut App) -> FocusHandle {
        self.remove
            .entry(tag.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn chip(
        &mut self,
        tag: &LangTag,
        only: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let name = languages::name(tag);
        let focus = self.remove_focus(tag, cx);
        let label = if only {
            format!("Remove {name}, unavailable")
        } else {
            format!("Remove {name}")
        };
        let this = cx.entity().downgrade();
        let removed = tag.clone();
        let remove = a11y::focusable(
            a11y::node(format!("remove-language-{tag}"), Role::Button, label),
            &focus,
            theme,
            false,
        )
        .flex_none()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .child(Icon::X.view(12., theme.text_secondary))
        .when(only, |b| b.aria_description(KEEP_ONE))
        .when(!only, |b| {
            b.cursor_pointer().on_click(move |_, _, cx| {
                cx.stop_propagation();
                let removed = removed.clone();
                let _ = this.update(cx, |c, cx| c.toggle(removed, cx));
            })
        });
        div()
            .id(SharedString::from(format!("chip-{tag}")))
            .on_click(|_, _, cx| cx.stop_propagation())
            .min_h(px(26.))
            .pl(px(10.))
            .pr(px(1.))
            .flex()
            .items_center()
            .gap(px(2.))
            .rounded(px(13.))
            .bg(theme.chip_bg)
            .text_color(theme.text)
            .text_size(theme.text(13.))
            .child(name)
            .child(remove)
    }

    /// The popover never grows past the room on its side of the field: the list gives up rows
    /// first, so the search field and the hint stay in view.
    fn popover(
        &mut self,
        theme: &Theme,
        window_h: Pixels,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let field = self.field_bounds.get();
        let want = theme.text(ROW) * VISIBLE_ROWS + theme.text(CHROME);
        let place = Placement::of(field, window_h, want);
        let query = self.query(cx);
        let matches = languages::search(&query);
        let chosen: Vec<LangTag> = self
            .models
            .read(cx)
            .settings()
            .map(|s| s.meeting_languages.clone())
            .unwrap_or_default();
        let count = match matches.rows.len() {
            1 => "1 match".to_owned(),
            n => format!("{n} matches"),
        };
        let this = cx.entity().downgrade();
        let mut list = a11y::node("language-options", Role::ListBox, "Languages")
            .track_scroll(&self.rows)
            .pr(px(12.))
            .flex()
            .flex_col();
        for (ix, lang) in matches.rows.iter().enumerate() {
            let on = chosen.iter().any(|t| t.as_str() == lang.code);
            let lit = self.highlight == Some(ix);
            let this = this.clone();
            let (fg, secondary) = if lit {
                (theme.on_accent, theme.on_accent)
            } else {
                (theme.text, theme.text_secondary)
            };
            list = list.child(
                a11y::node(
                    format!("language-option-{}", lang.code),
                    Role::ListBoxOption,
                    lang.english,
                )
                .aria_selected(on)
                .aria_description(lang.native)
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |c, cx| c.pick(ix, window, cx));
                })
                .flex_none()
                .min_h(theme.text(ROW))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(5.))
                .text_size(theme.text(13.))
                .text_color(fg)
                .when(lit, |row| row.bg(theme.accent))
                .child(
                    div()
                        .flex_none()
                        .w(px(16.))
                        .when(on, |d| d.child(Icon::Check.view(12., fg))),
                )
                .child(div().flex_1().min_w_0().child(lang.english))
                .when(lang.native != lang.english, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(theme.text(12.))
                            .text_color(secondary)
                            .child(lang.native),
                    )
                }),
            );
        }
        let empty = matches.rows.is_empty().then(|| {
            let text = format!("No language matches “{}”.", query.trim());
            a11y::node("language-no-match", Role::Label, text.clone())
                .px(px(6.))
                .py(px(6.))
                .text_size(theme.text(13.))
                .text_color(theme.text_secondary)
                .child(text)
        });
        let close = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            this.close(window, cx);
            cx.stop_propagation();
        };
        let popover = a11y::node(
            "meeting-languages-popover",
            Role::Group,
            "Add meeting languages",
        )
        .occlude()
        .on_mouse_down_out(cx.listener(move |this, _, window, cx| this.close(window, cx)))
        .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
            this.move_highlight(-1, cx);
            cx.stop_propagation();
        }))
        .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
            this.move_highlight(1, cx);
            cx.stop_propagation();
        }))
        .capture_action(cx.listener(|this, _: &Enter, window, cx| {
            if let Some(ix) = this.highlight {
                this.pick(ix, window, cx);
            }
            cx.stop_propagation();
        }))
        .capture_action(cx.listener(move |this, _: &Escape, window, cx| close(this, window, cx)))
        .capture_action(
            cx.listener(move |this, _: &IndentInline, window, cx| close(this, window, cx)),
        )
        .capture_action(
            cx.listener(move |this, _: &OutdentInline, window, cx| close(this, window, cx)),
        )
        .capture_action(cx.listener(|this, _: &Backspace, _, cx| {
            if this.query(cx).is_empty() {
                this.remove_last(cx);
                cx.stop_propagation();
            }
        }))
        .w(field.size.width)
        .max_h(place.max_h)
        .p(px(6.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .map(|p| crate::surface::paint(p, crate::surface::Kind::Popover, theme))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&self.search)
                            .id("language-search")
                            .aria_label("Search languages"),
                    ),
                )
                .child(
                    a11y::node("language-match-count", Role::Label, count.clone())
                        .flex_none()
                        .text_size(theme.text(12.))
                        .text_color(theme.text_secondary)
                        .child(count),
                ),
        )
        .child(
            crate::surface::tracked_scroll_area("language-options-scroll", &self.scroll, list, cx)
                .max_h(theme.text(ROW) * VISIBLE_ROWS)
                .flex_shrink(1.),
        )
        .children(empty)
        .child(
            a11y::node("language-hint", Role::Label, HINT)
                .text_size(theme.text(11.))
                .text_color(theme.text_secondary)
                .child(HINT),
        );
        // The anchor sits at the field's bottom edge. Above, the popover's bottom-left corner
        // goes one gap over the field's top edge.
        let (corner, lift) = if place.above {
            (Anchor::BottomLeft, -(field.size.height + px(GAP)))
        } else {
            (Anchor::TopLeft, px(GAP))
        };
        deferred(
            anchored()
                .anchor(corner)
                .offset(gpui_kit::point(px(0.), lift))
                .snap_to_window_with_margin(px(MARGIN))
                .child(popover),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl Render for LanguageCombobox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reveal) && self.open {
            // After this frame, when the rows have their new bounds.
            cx.defer_in(window, |this, _, cx| {
                this.reveal_highlight();
                cx.notify();
            });
        }
        let theme = Theme::get(cx);
        let wrap = div().flex().flex_col().w_full();
        let Some(chosen) = self
            .models
            .read(cx)
            .settings()
            .map(|s| s.meeting_languages.clone())
        else {
            return wrap.child(
                div()
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child("Loading"),
            );
        };
        let names: Vec<String> = chosen.iter().map(languages::name).collect();
        let only = chosen.len() == 1;
        let chips: Vec<_> = chosen
            .iter()
            .map(|tag| self.chip(tag, only, &theme, cx))
            .collect();
        let this = cx.entity().downgrade();
        let add = a11y::focusable(
            a11y::node("add-language", Role::Button, "Add Language"),
            &self.add,
            &theme,
            false,
        )
        .cursor_pointer()
        .on_click({
            let this = this.clone();
            move |_, window, cx| {
                cx.stop_propagation();
                let _ = this.update(cx, |c, cx| c.open(window, cx));
            }
        })
        .min_h(px(26.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded(px(6.))
        .text_size(theme.text(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.accent_text)
        .child(Icon::Plus.view(14., theme.accent_text))
        .child("Add Language");
        let field_bounds = self.field_bounds.clone();
        let (this_view, open) = (cx.entity_id(), self.open);
        let focus = focus_edge(&theme);
        let field = a11y::node("meeting-languages", Role::ComboBox, "Meeting languages")
            .aria_value(names.join(", "))
            .aria_expanded(self.open)
            .track_focus(&self.field.clone().tab_stop(true))
            .on_click({
                let this = this.clone();
                move |_, window, cx| {
                    let _ = this.update(cx, |c, cx| c.open(window, cx));
                }
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "down" | "enter" | "space" => this.open(window, cx),
                    "backspace" => this.remove_last(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .relative()
            .cursor_pointer()
            .w_full()
            .min_h(px(36.))
            .px(px(6.))
            .py(px(4.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .rounded(px(8.))
            .bg(theme.control_bg)
            .border_1()
            .border_color(theme.control_border)
            .focus_visible(move |style| {
                style.border_color(focus.color).shadow(vec![BoxShadow {
                    color: focus.color,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: focus.halo,
                    inset: false,
                }])
            })
            .children(chips)
            .child(add);
        // `anchored` is absolutely positioned, so it starts at its parent's origin. This
        // zero-height strip puts that origin at the field's bottom edge.
        let anchor = div().relative().w_full().h_0().children(
            self.open
                .then(|| self.popover(&theme, window.viewport_size().height, cx)),
        );
        let status = self.said.clone().map(|said| {
            a11y::node("meeting-languages-status", Role::Status, said)
                .absolute()
                .size_0()
                .overflow_hidden()
        });
        // The wrap is as big as the field, border included, so the canvas measures it.
        wrap.relative()
            .child(field)
            .child(anchor)
            .children(status)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        // The popover is placed from these bounds, so draw it again when they
                        // move while it is open.
                        if field_bounds.replace(bounds) != bounds && open {
                            cx.notify(this_view);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

/// The combobox for a view that keeps no entity of its own, such as onboarding's Models step.
pub fn meeting_languages(
    app: &Entity<crate::app::AppState>,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let models = crate::models_view::install(app, cx);
    window
        .use_keyed_state("meeting-languages-combobox", cx, |window, cx| {
            LanguageCombobox::new(models, window, cx)
        })
        .into_any_element()
}
