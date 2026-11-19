//! The language combobox: a popover that searches the languages Dory knows by their English
//! name, their own name, or their code. It serves two fields.
//!
//! - The meeting languages (UX-29, `docs/design/settings-icons-blur.md` section 4.3): chips in a
//!   field. Settings > Transcription and onboarding's Models step both show it.
//! - **Translate…** in the Summary tab's action row: a button whose list puts the meeting
//!   languages above a divider and every other language under it.
//!
//! Keys in the popover's search: Up and Down move the highlight and wrap, and Escape, Tab, or a
//! click outside closes it and returns focus to the field. For the meeting languages Return adds
//! or removes the highlighted language and keeps the popover open, and Delete in an empty search
//! removes the last chip. For **Translate…** Return asks for the translation and closes it. The
//! search field is Kit's, so the popover catches those keys in the capture phase, before the field
//! turns them into cursor moves.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use dory_core::domain::LangTag;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{
    Backspace, Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp,
    OutdentInline,
};
use gpui_kit::{
    Anchor, App, AppContext as _, Bounds, BoxShadow, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render, Role,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    anchored, canvas, deferred, div, point, prelude::FluentBuilder as _, px, relative, rems,
};

use crate::a11y;
use crate::actions::TranslateSummaryTo;
use crate::app::{AppState, Session};
use crate::icons::Icon;
use crate::languages::{self, Edit, KEEP_ONE, Matches, Offer};
use crate::models_view::Models;
use crate::theme::Theme;

pub const HINT: &str = "Return adds or removes · Esc closes · Delete removes the last chip";
const TRANSLATE_HINT: &str = "Return translates · Esc closes";
/// How wide **Translate…**'s popover is at 100% text, since its button is narrower than a list.
const TRANSLATE_WIDTH: f32 = 300.;
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
    /// to fit that room, which is never more than the window holds, even when the page has
    /// scrolled the field out of view.
    fn of(field: Bounds<Pixels>, window_h: Pixels, want: Pixels) -> Placement {
        let gap = px(GAP + MARGIN);
        let most = window_h - px(2. * MARGIN);
        let below = (window_h - field.bottom() - gap).min(most);
        let above = (field.top() - gap).min(most);
        let up = below < want && above > below;
        Placement {
            above: up,
            max_h: (if up { above } else { below }).max(px(0.)),
        }
    }
}

/// What a combobox edits, which decides its field, its rows, and what a pick does.
enum Purpose {
    /// Every language, alphabetical. A pick adds or removes a meeting language and keeps the
    /// popover open.
    Meeting(Entity<Models>),
    /// [`Offer::translate`] for the selected recording's summary. A pick asks for that
    /// translation and closes the popover.
    Translate {
        app: Entity<AppState>,
        session: Entity<Session>,
    },
}

pub struct LanguageCombobox {
    purpose: Purpose,
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
    /// The meeting-languages field.
    pub fn new(models: Entity<Models>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let on_models = cx.observe(&models, |_, _, cx| cx.notify());
        if models.read(cx).settings().is_none() {
            models.update(cx, |m, cx| m.refresh(cx));
        }
        Self::with(Purpose::Meeting(models), on_models, window, cx)
    }

    /// **Translate…** for the summary `session` shows.
    pub fn translate(
        app: Entity<AppState>,
        session: Entity<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let on_session = cx.observe(&session, |_, _, cx| cx.notify());
        Self::with(Purpose::Translate { app, session }, on_session, window, cx)
    }

    fn with(
        purpose: Purpose,
        on_source: Subscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search languages"));
        a11y::register_text_field(&search, cx);
        let on_type = cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.highlight = this.matches(cx).best;
                this.scroll_to_highlight();
                cx.notify();
            }
        });
        Self {
            purpose,
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
            _subscriptions: vec![on_type, on_source],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn query(&self, cx: &App) -> SharedString {
        self.search.read(cx).value()
    }

    fn offer(&self, cx: &App) -> Offer {
        match &self.purpose {
            Purpose::Meeting(_) => Offer::every(),
            Purpose::Translate { app, session } => {
                let detail = session.read(cx).detail();
                let written = detail
                    .as_ref()
                    .and_then(|d| d.summary.as_ref())
                    .and_then(|s| s.language.as_ref());
                Offer::translate(&app.read(cx).settings().meeting_languages, written)
            }
        }
    }

    fn matches(&self, cx: &App) -> Matches {
        self.offer(cx).search(&self.query(cx))
    }

    /// What the list checks: the meeting languages, or the language the summary is translated
    /// into now. `None` until the meeting languages load.
    fn chosen(&self, cx: &App) -> Option<Vec<LangTag>> {
        match &self.purpose {
            Purpose::Meeting(models) => models
                .read(cx)
                .settings()
                .map(|s| s.meeting_languages.clone()),
            Purpose::Translate { session, .. } => Some(
                session
                    .read(cx)
                    .detail()
                    .and_then(|d| d.summary_translation.as_ref().map(|t| t.target.clone()))
                    .into_iter()
                    .collect(),
            ),
        }
    }

    fn meeting(&self) -> bool {
        matches!(self.purpose, Purpose::Meeting(_))
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
            self.highlight = self.matches(cx).best;
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
        let Purpose::Meeting(models) = &self.purpose else {
            return None;
        };
        let name = languages::name(&tag);
        let edit = models.update(cx, |m, cx| m.toggle_language(tag, cx))?;
        match &edit {
            Edit::Added => self.say(format!("{name} added")),
            Edit::Removed { .. } => self.say(format!("{name} removed")),
            Edit::KeptLast => self.say(KEEP_ONE.into()),
            Edit::Confirmed => self.say(format!("{name} chosen")),
        }
        cx.notify();
        Some(edit)
    }

    /// Return or a click on a row. For the meeting languages it toggles the row, clears the
    /// search, and keeps the highlight on the same language in the full list. For
    /// **Translate…** it asks for the translation and closes the popover.
    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.matches(cx).rows.into_iter().nth(ix) else {
            return;
        };
        if !self.meeting() {
            // Close first: the action goes out later, from what has focus now, and the search
            // field is gone by then while the button stays.
            self.close(window, cx);
            window.dispatch_action(Box::new(TranslateSummaryTo { target: row.tag }), cx);
            return;
        }
        self.toggle(row.tag.clone(), cx);
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.highlight = self.matches(cx).rows.iter().position(|r| r.tag == row.tag);
        self.scroll_to_highlight();
        cx.notify();
    }

    fn remove_last(&mut self, cx: &mut Context<Self>) {
        let last = self.chosen(cx).and_then(|chosen| chosen.last().cloned());
        if let Some(tag) = last {
            self.toggle(tag, cx);
        }
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.matches(cx).rows.len() as isize;
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
        .size(crate::icons::scaled(24.))
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
        viewport: gpui_kit::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let field = self.field_bounds.get();
        let want = theme.text(ROW) * VISIBLE_ROWS + theme.text(CHROME);
        let place = Placement::of(field, viewport.height, want);
        let meeting = self.meeting();
        let (id, name, hint, width) = if meeting {
            (
                "meeting-languages-popover",
                "Add meeting languages",
                HINT,
                field.size.width,
            )
        } else {
            (
                "translate-popover",
                "Translate summary into",
                TRANSLATE_HINT,
                theme
                    .text(TRANSLATE_WIDTH)
                    .min(viewport.width - px(2. * MARGIN)),
            )
        };
        let query = self.query(cx);
        let matches = self.matches(cx);
        let chosen = self.chosen(cx).unwrap_or_default();
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
            let on = chosen.iter().any(|t| t.primary() == lang.tag.primary());
            let lit = self.highlight == Some(ix);
            let this = this.clone();
            let (fg, secondary) = if lit {
                (theme.on_accent, theme.on_accent)
            } else {
                (theme.text, theme.text_secondary)
            };
            let option = a11y::node(
                format!("language-option-{}", lang.tag),
                Role::ListBoxOption,
                lang.english.clone(),
            )
            .aria_selected(on)
            .when_some(lang.native, |row, native| row.aria_description(native))
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
                    .w(crate::icons::scaled(16.))
                    .when(on, |d| d.child(Icon::Check.view(12., fg))),
            )
            .child(div().flex_1().min_w_0().child(lang.english.clone()))
            .when_some(lang.native_hint(), |row, native| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(theme.text(12.))
                        .text_color(secondary)
                        .child(native),
                )
            });
            // The divider rides on the row under it, so the list's children stay one per row
            // and `reveal_highlight` finds row `ix` at child `ix`.
            list = if ix == matches.lead && ix > 0 {
                list.child(
                    div()
                        .flex_none()
                        .child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(theme.divider))
                        .child(option),
                )
            } else {
                list.child(option)
            };
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
        let popover = a11y::node(id, Role::Group, name)
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
            .capture_action(
                cx.listener(move |this, _: &Escape, window, cx| close(this, window, cx)),
            )
            .capture_action(
                cx.listener(move |this, _: &IndentInline, window, cx| close(this, window, cx)),
            )
            .capture_action(
                cx.listener(move |this, _: &OutdentInline, window, cx| close(this, window, cx)),
            )
            .when(meeting, |p| {
                p.capture_action(cx.listener(|this, _: &Backspace, _, cx| {
                    if this.query(cx).is_empty() {
                        this.remove_last(cx);
                        cx.stop_propagation();
                    }
                }))
            })
            .w(width)
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
                crate::surface::tracked_scroll_area(
                    "language-options-scroll",
                    &self.scroll,
                    list,
                    cx,
                )
                .max_h(theme.text(ROW) * VISIBLE_ROWS)
                .flex_shrink(1.),
            )
            .children(empty)
            .child(
                a11y::node("language-hint", Role::Label, hint)
                    .text_size(theme.text(11.))
                    .text_color(theme.text_secondary)
                    .child(hint),
            );
        // The anchor sits at the field's bottom-left corner. Above, the popover's bottom edge
        // goes one gap over the field's top edge. **Translate…** sits at the right end of its
        // row, so its wider popover lines up with the button's right edge instead.
        let lift = if place.above {
            -(field.size.height + px(GAP))
        } else {
            px(GAP)
        };
        let (corner, shift) = match (place.above, meeting) {
            (true, true) => (Anchor::BottomLeft, px(0.)),
            (false, true) => (Anchor::TopLeft, px(0.)),
            (true, false) => (Anchor::BottomRight, field.size.width),
            (false, false) => (Anchor::TopRight, field.size.width),
        };
        deferred(
            anchored()
                .anchor(corner)
                .offset(gpui_kit::point(shift, lift))
                .snap_to_window_with_margin(px(MARGIN))
                .child(popover),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl LanguageCombobox {
    /// The meeting languages' field: a chip for each, then Add Language.
    fn chips_field(
        &mut self,
        chosen: &[LangTag],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> a11y::Node {
        let names: Vec<String> = chosen.iter().map(languages::name).collect();
        let only = chosen.len() == 1;
        let chips: Vec<_> = chosen
            .iter()
            .map(|tag| self.chip(tag, only, theme, cx))
            .collect();
        let this = cx.entity().downgrade();
        let add = a11y::focusable(
            a11y::node("add-language", Role::Button, "Add Language"),
            &self.add,
            theme,
            false,
        )
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            let _ = this.update(cx, |c, cx| c.open(window, cx));
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
        let focus = focus_edge(theme);
        a11y::node("meeting-languages", Role::ComboBox, "Meeting languages")
            .aria_value(names.join(", "))
            .track_focus(&self.field.clone().tab_stop(true))
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
            .child(add)
    }

    /// **Translate…**, drawn to Kit's ghost button metrics so it matches Copy and Export beside
    /// it. Its value is the language the summary is translated into now.
    fn translate_field(&self, chosen: &[LangTag], theme: &Theme, cx: &App) -> a11y::Node {
        let kit = cx.theme();
        let hover = if theme.dark {
            kit.accent.opacity(0.5)
        } else {
            kit.accent
        };
        a11y::focusable(
            a11y::node("translate", Role::ComboBox, "Translate summary"),
            &self.field,
            theme,
            false,
        )
        .when_some(chosen.first(), |field, tag| {
            field.aria_value(languages::name(tag))
        })
        .flex_none()
        .h(rems(2.))
        .px(rems(0.5))
        .flex()
        .items_center()
        .gap(rems(0.5))
        .rounded(kit.radius)
        .text_sm()
        .line_height(relative(1.25))
        .text_color(theme.accent_text)
        .hover(move |style| style.bg(hover))
        .child("Translate…")
        .child(Icon::ChevronDown.view(12., theme.accent_text))
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
        let meeting = self.meeting();
        let wrap = div().flex().flex_col().when(meeting, |w| w.w_full());
        let Some(chosen) = self.chosen(cx) else {
            return wrap.child(
                div()
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child("Loading"),
            );
        };
        let field = if meeting {
            self.chips_field(&chosen, &theme, cx)
        } else {
            self.translate_field(&chosen, &theme, cx)
        };
        let this = cx.entity().downgrade();
        let field = field
            .aria_expanded(self.open)
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |c, cx| c.open(window, cx));
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "down" | "enter" | "space" => this.open(window, cx),
                    "backspace" if this.meeting() => this.remove_last(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .relative()
            .cursor_pointer();
        let field_bounds = self.field_bounds.clone();
        let (this_view, open) = (cx.entity_id(), self.open);
        // `anchored` is absolutely positioned, so it starts at its parent's origin. This
        // zero-height strip puts that origin at the field's bottom edge.
        let anchor = div().relative().w_full().h_0().children(
            self.open
                .then(|| self.popover(&theme, window.viewport_size(), cx)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn field(top: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(top)), gpui_kit::size(px(100.), px(32.)))
    }

    #[test]
    fn the_popover_opens_below_when_it_fits_and_never_outgrows_the_window() {
        let roomy = Placement::of(field(100.), px(720.), px(300.));
        assert!(!roomy.above);
        assert_eq!(roomy.max_h, px(720. - 132. - GAP - MARGIN));

        let low = Placement::of(field(500.), px(560.), px(300.));
        assert!(low.above);
        assert_eq!(low.max_h, px(500. - GAP - MARGIN));

        let below_the_fold = Placement::of(field(900.), px(560.), px(300.));
        assert!(below_the_fold.above);
        assert_eq!(below_the_fold.max_h, px(560. - 2. * MARGIN));
        let above_the_fold = Placement::of(field(-400.), px(560.), px(300.));
        assert!(!above_the_fold.above);
        assert_eq!(above_the_fold.max_h, px(560. - 2. * MARGIN));
    }
}
