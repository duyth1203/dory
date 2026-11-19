//! The inset sidebar panel (UX-1, UX-25): search, the status filter, the recordings grouped
//! by date, and the Settings gear at the bottom left. While a search has results, the date
//! groups give way to two sections, "Titles" and "In conversations" (UX-32), with a line saying
//! so above them when the search was corrected.
//! It reads `AppState` and writes only through its methods.
//!
//! Each row has two lines (`docs/design/processing-view.md` section 4): the title, then the date
//! and duration on the left with the status glyph and word on the right. The row is at least
//! 52 pt tall and grows with the text. The title truncates up to 125% text and wraps above it;
//! the status word never truncates, so the title gives way first. A conversation hit adds a third
//! line, the snippet around the first match, in 12 pt secondary text: one line with an ellipsis
//! up to 125% text, two lines above. Matched words, in the title and the snippet, are drawn
//! semibold on the `search_match` fill, so the fill never carries a match alone. On the
//! accent-filled row they are underlined in `on_accent` instead of filled.
//!
//! The list is one Tab stop. Up and Down move the selection, and the selected row is the list's
//! active descendant, so VoiceOver follows the selection the way it does in a native source
//! list. With focus in the list the selected row is `accent`-filled with an `on_accent` ring
//! between two accent bands (`Theme::row_ring`), so the ring shows on a near-white sidebar too;
//! with focus elsewhere it takes `selection_inactive`. The list scrolls the selected row into
//! view, so the keyboard never moves the selection out of sight, and Return renames it.

use std::ops::Range;

use chrono::Local;
use dory_core::search::Section;
use gpui_kit::component::Side;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, FontWeight, HighlightStyle, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, Role,
    ScrollHandle, StatefulInteractiveElement as _, Styled, StyledText, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative,
};

use crate::a11y;
use crate::actions::{
    FilterAll, FilterDone, FilterNeedsAttention, FilterProcessing, OpenSettings, Rename,
};
use crate::app::{AppEvent, AppState};
use crate::fonts;
use crate::icons::Icon;
use crate::library::{self, DateGroup, Glyph, Selection, StatusFilter};
use crate::marks::marked;
use crate::menus;
use crate::prefs;
use crate::search_field;
use crate::theme::Theme;
use crate::{glass, glyphs, motion};

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
        a11y::register_text_field(&search, cx);
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
        let on_fill = cx.subscribe_in(&app, window, |this, _, event: &AppEvent, window, cx| {
            if let AppEvent::SearchFilled(text) = event {
                this.search
                    .update(cx, |search, cx| search.set_value(text.clone(), window, cx));
            }
        });
        Self {
            app,
            search,
            list_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            scrolled_to: None,
            _subscriptions: vec![on_type, on_app, on_fill],
        }
    }

    /// Empties the field, keeps focus in it, and brings the date groups back.
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| {
            search.set_value("", window, cx);
            search.focus(window, cx);
        });
        self.app.update(cx, |app, cx| app.set_search("", cx));
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
            .icon(Icon::ListFilter.kit())
            .dropdown_menu(move |menu, _, _| {
                StatusFilter::ALL
                    .into_iter()
                    .fold(menu.check_side(Side::Right), |menu, f| {
                        menu.item(
                            PopupMenuItem::new(f.label())
                                .icon(f.icon().kit())
                                .checked(f == filter)
                                .action(match f {
                                    StatusFilter::All => Box::new(FilterAll),
                                    StatusFilter::Processing => Box::new(FilterProcessing),
                                    StatusFilter::NeedsAttention => Box::new(FilterNeedsAttention),
                                    StatusFilter::Done => Box::new(FilterDone),
                                }),
                        )
                    })
            })
    }

    /// The gear at the panel's bottom left. It sends the same `OpenSettings` as Command-Comma.
    fn settings_button() -> impl IntoElement {
        Button::new("sidebar-settings")
            .ghost()
            .compact()
            .icon(Icon::Settings.kit())
            .accessibility_label("Settings")
            .tooltip_with_action("Settings", &OpenSettings, None)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx))
    }

    /// The headings and rows, and the selected row's index among them.
    fn rows(&self, window: &Window, cx: &mut Context<Self>) -> Rows {
        let theme = Theme::get(cx);
        let loops = motion::looping(window, cx);
        let at = loops.then(motion::clock_ms);
        let mut breathing = false;
        let app = self.app.read(cx);
        let now = Local::now();
        let today = now.date_naive();
        let list_focused = self.list_focus.is_focused(window);
        let leaders = theme
            .presentation(f32::from(window.viewport_size().width))
            .toc_leaders;
        let outline = theme.highlight_outline();
        let selected = app.selection.id().cloned();
        let visible = app.visible();
        if visible.is_empty() {
            let why = if !app.is_loaded() {
                String::new()
            } else if !app.typed.is_empty() {
                format!("No recordings match “{}”.", app.typed.text())
            } else if app.recordings.is_empty() {
                "No recordings yet".into()
            } else {
                format!("No recordings are {}.", app.filter.label().to_lowercase())
            };
            let empty = a11y::node("sidebar-empty", Role::Label, why.clone())
                .px(px(12.))
                .py(px(8.))
                .text_size(theme.text(13.))
                .text_color(theme.text_secondary)
                .child(why)
                .into_any_element();
            return Rows {
                children: vec![empty],
                selected: None,
                headers: Vec::new(),
            };
        }
        let count = visible.len();
        let statuses: std::collections::HashMap<_, _> = visible
            .iter()
            .map(|(row, status)| (row.recording.id.clone(), *status))
            .collect();
        let blocks: Vec<Block> = match app.visible_sections() {
            Some(sections) => sections
                .into_iter()
                .filter(|(_, rows)| !rows.is_empty())
                .map(|(section, rows)| Block {
                    id: format!("section-{}", library::section_label(section)),
                    label: library::section_label(section).into(),
                    section: Some(section),
                    rows: rows.into_iter().map(|(row, _)| row).collect(),
                })
                .collect(),
            None => library::group_by_date(visible.iter().map(|(row, _)| *row), &now)
                .into_iter()
                .map(|(group, rows)| {
                    let label = group.label(today);
                    Block {
                        id: format!("group-{label}"),
                        label,
                        section: None,
                        rows,
                    }
                })
                .collect(),
        };
        let query = app.search.as_ref().map(|results| &results.query);
        let mut position = 0;
        let mut children: Vec<AnyElement> = Vec::new();
        let mut selected_ix = None;
        let mut headers = Vec::new();
        if let Some(words) = app
            .search
            .as_ref()
            .and_then(|results| library::corrected_words(&results.corrections))
        {
            children.push(correction_line(&words, &theme).into_any_element());
        }
        for block in blocks {
            headers.push((children.len(), block.label.clone()));
            children.push(heading(&block.id, &block.label, &theme).into_any_element());
            for row in block.rows {
                position += 1;
                let id = row.recording.id.clone();
                let is_selected = selected.as_ref() == Some(&id);
                if is_selected {
                    selected_ix = Some(children.len());
                }
                let filled = is_selected && list_focused;
                let status = statuses[&id];
                let words = status.label();
                breathing |= status.glyph() == Glyph::Dots || status.glyph() == Glyph::DotsCircle;
                let title = library::title(row, &now.timezone());
                let group = DateGroup::of(
                    library::local(row.recording.created_at, &now.timezone()).date_naive(),
                    today,
                );
                let (meta, length) = if leaders {
                    library::row_meta_parts(row, group, &now)
                } else {
                    (library::row_meta(row, group, &now), None)
                };
                let spoken_time = library::local(row.recording.created_at, &now.timezone())
                    .format("%H:%M")
                    .to_string();
                let mut label = match row.recording.take {
                    Some(take) => format!(
                        "{title}, {spoken_time}, {}, {words}",
                        library::short_duration(take.duration),
                    ),
                    None => format!("{title}, {spoken_time}, {words}"),
                };
                let excerpt = (block.section == Some(Section::Conversations))
                    .then(|| app.snippets.get(&id))
                    .flatten();
                if let Some(section) = block.section
                    && let Some(found) = library::match_words(section, excerpt)
                {
                    label = format!("{label}. {found}");
                }
                let title_marks = query
                    .map(|query| library::title_marks(query, &row.recording))
                    .unwrap_or_default();
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
                let row_key = format!("row-{id}");
                let row_el = a11y::node(row_key.clone(), Role::ListItem, label)
                    .aria_selected(is_selected)
                    .aria_position_in_set(position)
                    .aria_size_of_set(count)
                    .when(is_selected, |row| row.aria_active_descendant())
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(2.))
                    .flex_none()
                    .min_h(px(52.))
                    .px(px(12.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .border_2()
                    .border_color(row_edge(&theme, filled))
                    .when(filled, |row| {
                        row.bg(theme.accent)
                            .relative()
                            .child(row_ring_line(&row_key, &theme))
                    })
                    .when(is_selected && !list_focused, |row| {
                        row.bg(theme.selection_inactive)
                    })
                    .on_click(move |_, window, cx| {
                        window.focus(&focus, cx);
                        app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
                    })
                    .when_some(
                        outline.filter(|_| is_selected && !list_focused),
                        |row, color| {
                            row.relative().child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(color),
                            )
                        },
                    )
                    .child(if theme.serif() {
                        let title = marked(title, &title_marks, FontWeight::BOLD, filled, &theme);
                        contents_line(title, length, fg, fg2, &theme)
                    } else {
                        div()
                            .text_size(theme.text(14.))
                            .line_height(relative(1.25))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .when(theme.text_scale <= 1.25, |title| title.truncate())
                            .child(marked(
                                title,
                                &title_marks,
                                FontWeight::BOLD,
                                filled,
                                &theme,
                            ))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_x(px(8.))
                            .text_size(theme.text(12.))
                            .text_color(fg2)
                            .child(div().min_w_0().child(meta))
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(px(4.))
                                    .whitespace_nowrap()
                                    .child(glyphs::row(status.glyph(), glyph_color, 12., at))
                                    .child(words),
                            ),
                    )
                    .when_some(excerpt, |row, excerpt| {
                        row.child(
                            div()
                                .text_size(theme.text(12.))
                                .line_height(relative(1.25))
                                .text_color(fg2)
                                .when(theme.text_scale <= 1.25, |line| line.truncate())
                                .when(theme.text_scale > 1.25, |line| {
                                    line.line_clamp(2).text_ellipsis()
                                })
                                .child(marked(
                                    excerpt.snippet.text.clone(),
                                    &excerpt.snippet.marks,
                                    FontWeight::SEMIBOLD,
                                    filled,
                                    &theme,
                                )),
                        )
                    })
                    .context_menu(move |menu, window, cx| {
                        menus::recording_menu(menu, &menu_id, window, cx)
                    });
                children.push(row_el.into_any_element());
            }
        }
        if breathing && loops {
            window.request_animation_frame();
        }
        Rows {
            children,
            selected: selected_ix,
            headers,
        }
    }

    /// The group whose header the list has scrolled past, with the header's height: the label that
    /// stays on screen under the search field while the group's rows scroll. Read from the last
    /// frame's layout, whose item bounds do not include the scroll offset. `None` while the first header is fully shown.
    fn sticky(&self, headers: &[(usize, String)]) -> Option<(String, Pixels)> {
        let top = self.scroll.bounds().top();
        let scrolled = self.scroll.offset().y;
        headers.iter().rev().find_map(|(ix, label)| {
            let header = self.scroll.bounds_for_item(*ix)?;
            (header.top() + scrolled < top - px(0.5)).then(|| (label.clone(), header.size.height))
        })
    }
}

/// What the list draws: its children in order, the selected row's index among them, and each
/// group header's index and label.
struct Rows {
    children: Vec<AnyElement>,
    selected: Option<usize>,
    headers: Vec<(usize, String)>,
}

/// One heading and the rows under it: a date group, or a search section.
struct Block<'a> {
    id: String,
    label: String,
    section: Option<Section>,
    rows: Vec<&'a dory_core::domain::RecordingRow>,
}

/// "Showing results for roadmap", above the sections of a search that was corrected.
/// The 2 pt border of a row: the outer `accent` band of the selected row's ring while the list
/// has focus, clear on every other row.
pub fn row_edge(theme: &Theme, filled: bool) -> Hsla {
    if filled {
        theme.row_ring().edge
    } else {
        gpui_kit::transparent_black()
    }
}

/// The ring's line, over the row's inner edge so it lies between the `accent` edge and the
/// `accent` fill. `row_key` is the row's own id.
pub fn row_ring_line(row_key: &str, theme: &Theme) -> a11y::Node {
    a11y::probe(format!("{row_key}-ring"))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(px(6.))
        .border_2()
        .border_color(theme.row_ring().line)
}

fn correction_line(words: &str, theme: &Theme) -> impl IntoElement {
    let lead = "Showing results for ";
    let line = format!("{lead}{words}");
    let words = lead.len()..line.len();
    a11y::node("search-correction", Role::Label, line.clone())
        .flex_none()
        .px(px(12.))
        .pt(px(8.))
        .text_size(theme.text(12.))
        .text_color(theme.text_secondary)
        .child(marked_words(line, words, theme.text))
}

fn marked_words(text: String, words: Range<usize>, color: Hsla) -> AnyElement {
    let style = HighlightStyle {
        color: Some(color),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..HighlightStyle::default()
    };
    StyledText::new(text)
        .with_highlights([(words, style)])
        .into_any_element()
}

fn heading(id: &str, label: &str, theme: &Theme) -> impl IntoElement {
    let header = a11y::node(id.to_owned(), Role::Heading, label.to_owned()).aria_level(3);
    heading_style(header, label, theme)
}

/// The group header that stays at the list's top while its group scrolls. It repeats the heading
/// in the list, which VoiceOver already reads, so it has no node of its own.
fn sticky_header(label: &str, height: Pixels, fill: Hsla, theme: &Theme) -> AnyElement {
    heading_style(a11y::probe("sidebar-sticky-header"), label, theme)
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(height)
        .bg(fill)
        .border_b_1()
        .border_color(theme.divider)
        .into_any_element()
}

fn heading_style<E: Styled + ParentElement>(el: E, label: &str, theme: &Theme) -> E {
    let header = el
        .flex_none()
        .px(px(12.))
        .pt(px(12.))
        .pb(px(4.))
        .text_color(theme.text_secondary);
    // Book sets the group in serif italic, as a part title in a table of contents.
    if theme.serif() {
        theme
            .reading(header)
            .italic()
            .text_size(theme.text(13.))
            .child(label.to_owned())
    } else {
        header
            .text_size(theme.text(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(label.to_owned())
    }
}

/// Book's entry title (serif 15). With leaders it reads "Title ...... 48 min": the title on one
/// line, dotted leaders of at least 12 pt, and the length in oldstyle figures. Without them the
/// title wraps and the length stays in the meta line. The leaders are drawing only.
fn contents_line(
    title: AnyElement,
    length: Option<String>,
    fg: Hsla,
    fg2: Hsla,
    theme: &Theme,
) -> AnyElement {
    let line = theme
        .reading(div())
        .text_size(theme.text(15.))
        .line_height(gpui_kit::relative(1.3))
        .text_color(fg);
    let Some(length) = length else {
        return line.child(title).into_any_element();
    };
    line.flex()
        .items_baseline()
        .gap(px(6.))
        .child(div().min_w_0().truncate().child(title))
        .child(
            div()
                .flex_1()
                .min_w(px(12.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(theme.leader)
                .child(LEADER_DOTS),
        )
        .child(
            div()
                .flex_none()
                .text_color(fg2)
                .font_features(fonts::oldstyle())
                .child(length),
        )
        .into_any_element()
}

/// More dots than any sidebar is wide; the leader's box clips them.
const LEADER_DOTS: &str = ". . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .";

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
        let Rows {
            children: rows,
            selected: selected_ix,
            headers,
        } = self.rows(window, cx);
        let sticky = self.sticky(&headers);
        if selected_ix != self.scrolled_to {
            self.scrolled_to = selected_ix;
            if let Some(ix) = selected_ix {
                self.scroll.scroll_to_item(ix);
            }
        }
        let list_ring = theme.focus_ring(false);
        let fill = glass::sidebar_panel(&theme, cx);
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
                    .bg(fill)
                    .border_1()
                    .border_color(theme.panel_border)
                    // A shadow would show through a translucent panel and muddy it.
                    .when(theme.shadows && fill.a >= 1.0, |panel| panel.shadow_sm())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(4.))
                            .child(
                                div()
                                    .flex_1()
                                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                                        if this.search.read(cx).value().is_empty() {
                                            cx.propagate();
                                        } else {
                                            this.clear_search(window, cx);
                                        }
                                    }))
                                    .child(search_field::view(
                                        &self.search,
                                        "search",
                                        "Search recordings",
                                        &theme,
                                        cx,
                                        {
                                            let app = self.app.clone();
                                            move |_, cx| {
                                                app.update(cx, |app, cx| app.set_search("", cx))
                                            }
                                        },
                                    )),
                            )
                            .child(self.filter_button(filter)),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
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
                                        list.focus_visible(move |style| {
                                            style.border_color(list_ring)
                                        })
                                    })
                                    .aria_keyshortcuts("Return")
                                    .on_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, window, cx| {
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
                                            this.app.update(cx, |app, cx| {
                                                app.select_relative(delta, cx)
                                            });
                                            cx.stop_propagation();
                                        },
                                    ))
                                    .children(rows),
                            )
                            .children(sticky.map(|(label, height)| {
                                sticky_header(
                                    &label,
                                    height,
                                    if fill.a >= 1.0 {
                                        fill
                                    } else {
                                        theme.sidebar_bg
                                    },
                                    &theme,
                                )
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(4.))
                            .pb(px(8.))
                            .child(Self::settings_button()),
                    ),
            )
            .child(prefs::sidebar_resize_handle(&self.app, cx))
    }
}
