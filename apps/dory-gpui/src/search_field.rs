//! The search field both sidebars use, the main window's and Settings', so they look and behave
//! the same (UX-1): Lucide `search` at the start, and a ✕ at the end while it holds text.
//!
//! Kit's own clear button is smaller than the 24 pt minimum target (A11Y-9), so the field draws
//! its own: 24 pt, named "Clear search", and it keeps focus in the field after clearing.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    StatefulInteractiveElement as _, Styled as _, Window, prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::icons::Icon;
use crate::theme::Theme;

/// The field over `state`. Its clear button is `<id>-clear`; `cleared` runs after the field is
/// emptied, for callers that act on the text, since emptying it here sends no change event.
pub fn view(
    state: &Entity<InputState>,
    id: &'static str,
    label: &'static str,
    theme: &Theme,
    cx: &App,
    cleared: impl Fn(&mut Window, &mut App) + 'static,
) -> Input {
    let searching = !state.read(cx).value().is_empty();
    Input::new(state)
        .id(id)
        .aria_label(label)
        .prefix(Icon::Search.view(14., theme.text_secondary))
        .when(searching, |input| {
            input.suffix(clear_button(state.clone(), id, theme, cleared))
        })
}

fn clear_button(
    state: Entity<InputState>,
    id: &'static str,
    theme: &Theme,
    cleared: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let hover = theme.control_bg;
    a11y::node(format!("{id}-clear"), Role::Button, "Clear search")
        .size(crate::icons::scaled(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |button| button.bg(hover))
        .child(Icon::X.view(12., theme.text_secondary))
        .on_click(move |_, window, cx| {
            state.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            });
            cleared(window, cx);
        })
}
