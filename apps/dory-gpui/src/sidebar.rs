//! The inset sidebar panel (UX-1, UX-25): search, the status filter, and the recordings grouped
//! by date. It reads `AppState` and writes only through its methods.
//!
//! The list is one Tab stop. Up and Down move the selection, and the selected row is the list's
//! active descendant, so VoiceOver follows the selection the way it does in a native source
//! list. With focus in the list the selected row is `accent`-filled with an `on_accent` ring;
//! with focus elsewhere it takes `selection_inactive`. The list scrolls the selected row into
//! view, so the keyboard never moves the selection out of sight, and Return renames it.

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, Role, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::actions::{FilterAll, FilterDone, FilterNeedsAttention, FilterProcessing, Rename};
use crate::app::AppState;
use crate::library::{self, DateGroup, Glyph, RowStatus, Selection, StatusFilter};
use crate::menus;
use crate::prefs;
use crate::theme::Theme;

pub struct Sidebar {
    app: Entity<AppState>,
    search: Entity<InputState>,
    list_focus: FocusHandle,
    scroll: ScrollHandle,
    /// The selected row's child index the list last scrolled to.
    scrolled_to: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let on_type = cx.subscribe_in(
            &search,
            window,
            |this, search, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = search.read(cx).value();
                    this.app.update(cx, |app, cx| app.set_search(&text, cx));
                }
            },
        );
        let on_app = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            search,
            list_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            scrolled_to: None,
            _subscriptions: vec![on_type, on_app],
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.focus(window, cx));
    }

    pub fn list_focus(&self) -> &FocusHandle {
        &self.list_focus
    }

    fn filter_button(&self, filter: StatusFilter) -> impl IntoElement {
        Button::new("filter")
            .ghost()
            .compact()
            .label(filter.label())
            .accessibility_label(format!("Filter, {}", filter.label()))
            .dropdown_menu(move |menu, _, _| {
                menu.menu_with_check("All", filter == StatusFilter::All, Box::new(FilterAll))
                    .menu_with_check(
                        "Processing",
                        filter == StatusFilter::Processing,
                        Box::new(FilterProcessing),
                    )
                    .menu_with_check(
                        "Needs attention",
                        filter == StatusFilter::NeedsAttention,
                        Box::new(FilterNeedsAttention),
                    )
                    .menu_with_check("Done", filter == StatusFilter::Done, Box::new(FilterDone))
            })
    }

    /// The group headers and rows, and the selected row's index among them.
    fn rows(&self, window: &Window, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<usize>) {
        let theme = Theme::get(cx);
        let app = self.app.read(cx);
        let now = Local::now();
        let today = now.date_naive();
        let list_focused = self.list_focus.is_focused(window);
        let selected = app.selection.id().cloned();
        let visible = app.visible();
        if visible.is_empty() {
            let why = if !app.is_loaded() {
                String::new()
            } else if !app.search.is_empty() {
                format!("No recordings match “{}”.", app.search.text())
            } else if app.recordings.is_empty() {
                "No recordings yet".into()
            } else {
                format!("No recordings are {}.", app.filter.label().to_lowercase())
            };
            let empty = div()
                .px(px(12.))
                .py(px(8.))
                .text_size(theme.text(13.))
                .text_color(theme.text_secondary)
                .child(why)
                .into_any_element();
            return (vec![empty], None);
        }
        let count = visible.len();
        let groups = library::group_by_date(visible.iter().map(|(row, _)| *row), &now);
        let statuses: std::collections::HashMap<_, _> = visible
            .iter()
            .map(|(row, status)| (row.recording.id.clone(), *status))
            .collect();
        let mut position = 0;
        let mut children: Vec<AnyElement> = Vec::new();
        let mut selected_ix = None;
        for (group, rows) in groups {
            children.push(group_header(group, today, &theme).into_any_element());
            for row in rows {
                position += 1;
                let id = row.recording.id.clone();
                let is_selected = selected.as_ref() == Some(&id);
                if is_selected {
                    selected_ix = Some(children.len());
                }
                let filled = is_selected && list_focused;
                let status = statuses[&id];
                let words = app.status_text(row);
                let title = library::title(row, &now.timezone());
                let meta = library::row_meta(row, group, &now);
                let spoken_time = library::local(row.recording.created_at, &now.timezone())
                    .format("%H:%M")
                    .to_string();
                let label = match row.recording.take {
                    Some(take) => format!(
                        "{title}, {spoken_time}, {}, {}",
                        a11y::spoken_duration(take.duration),
                        words
                    ),
                    None => format!("{title}, {spoken_time}, {words}"),
                };
                let (fg, fg2) = if filled {
                    (theme.on_accent, theme.on_accent)
                } else {
                    (theme.text, theme.text_secondary)
                };
                let glyph_color = if filled {
                    theme.on_accent
                } else {
                    theme.tone(status.tone())
                };
                let app = self.app.clone();
                let focus = self.list_focus.clone();
                let menu_id = id.clone();
                let row_el = a11y::node(format!("row-{id}"), Role::ListItem, label)
                    .aria_selected(is_selected)
                    .aria_position_in_set(position)
                    .aria_size_of_set(count)
                    .when(is_selected, |row| row.aria_active_descendant())
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(8.))
                    .border_2()
                    .border_color(if filled {
                        theme.focus_ring(true)
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .when(filled, |row| row.bg(theme.accent))
                    .when(is_selected && !list_focused, |row| {
                        row.bg(theme.selection_inactive)
                    })
                    .on_click(move |_, window, cx| {
                        window.focus(&focus, cx);
                        app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
                    })
                    .child(
                        div()
                            .text_size(theme.text(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .truncate()
                            .child(title),
                    )
                    .child(div().text_size(theme.text(12.)).text_color(fg2).child(meta))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .text_size(theme.text(12.))
                            .text_color(fg2)
                            .child(div().text_color(glyph_color).child(glyph(status)))
                            .child(words),
                    )
                    .context_menu(move |menu, window, cx| {
                        menus::recording_menu(menu, &menu_id, window, cx)
                    });
                children.push(row_el.into_any_element());
            }
        }
        (children, selected_ix)
    }
}

fn group_header(group: DateGroup, today: chrono::NaiveDate, theme: &Theme) -> impl IntoElement {
    let label = group.label(today);
    a11y::node(format!("group-{label}"), Role::Heading, label.clone())
        .aria_level(3)
        .px(px(12.))
        .pt(px(12.))
        .pb(px(4.))
        .text_size(theme.text(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_secondary)
        .child(label)
}

/// Text glyphs until the app ships its own icon set. Each one always sits next to its word.
fn glyph(status: RowStatus) -> &'static str {
    match status.glyph() {
        Glyph::Dot => "●",
        Glyph::Doc => "▤",
        Glyph::Clock => "◷",
        Glyph::Ring(_) => "◔",
        Glyph::Check => "✓",
        Glyph::Triangle => "▲",
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (filter, width, nothing_selected) = {
            let app = self.app.read(cx);
            (
                app.filter,
                app.prefs
                    .sidebar_width
                    .clamp(*prefs::SIDEBAR_WIDTH.start(), *prefs::SIDEBAR_WIDTH.end()),
                app.selection == Selection::None,
            )
        };
        let (rows, selected_ix) = self.rows(window, cx);
        if selected_ix != self.scrolled_to {
            self.scrolled_to = selected_ix;
            if let Some(ix) = selected_ix {
                self.scroll.scroll_to_item(ix);
            }
        }
        let list_ring = theme.focus_ring(false);
        div()
            .flex()
            .flex_none()
            .h_full()
            .child(
                a11y::node("sidebar", Role::Navigation, "Sidebar")
                    .w(px(width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(44.))
                    .px(px(8.))
                    .rounded(px(12.))
                    .bg(theme.sidebar_bg)
                    .border_1()
                    .border_color(theme.panel_border)
                    .when(theme.shadows, |panel| panel.shadow_sm())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(4.))
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.search)
                                        .id("search")
                                        .aria_label("Search recordings")
                                        .cleanable(true),
                                ),
                            )
                            .child(self.filter_button(filter)),
                    )
                    .child(
                        a11y::node("recordings", Role::List, "Recordings")
                            .track_focus(&self.list_focus.clone().tab_stop(true))
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .pb(px(8.))
                            .rounded(px(10.))
                            .border_2()
                            .border_color(gpui_kit::transparent_black())
                            .when(nothing_selected, |list| {
                                list.focus_visible(move |style| style.border_color(list_ring))
                            })
                            .aria_keyshortcuts("Return")
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                if event.keystroke.key == "enter"
                                    && !event.keystroke.modifiers.modified()
                                {
                                    if this.app.read(cx).selection.id().is_some() {
                                        window.dispatch_action(Box::new(Rename), cx);
                                        cx.stop_propagation();
                                    }
                                    return;
                                }
                                let delta = match event.keystroke.key.as_str() {
                                    "down" => 1,
                                    "up" => -1,
                                    _ => return,
                                };
                                this.app
                                    .update(cx, |app, cx| app.select_relative(delta, cx));
                                cx.stop_propagation();
                            }))
                            .children(rows),
                    ),
            )
            .child(prefs::sidebar_resize_handle(&self.app, cx))
    }
}
