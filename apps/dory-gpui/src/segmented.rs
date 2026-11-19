//! The segmented switch: a few options side by side in one outlined pill, the chosen one filled
//! with `accent`. The transcript's and summary's Original / Translated and a vocabulary word's
//! Normal / Strong are this control. VoiceOver reads it as a radio group, each choice a radio
//! with its place in the set, and each choice is its own Tab stop.

use gpui_kit::{
    AnyElement, App, FocusHandle, FontWeight, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, px,
};

use crate::a11y;
use crate::theme::Theme;

/// One option of a [`switch`].
pub struct Choice {
    pub id: SharedString,
    pub label: SharedString,
    pub description: Option<SharedString>,
}

/// Draws `choices` as one radio group with `selected` filled. `pick` gets the clicked index.
pub fn switch<const N: usize>(
    (id, label): (impl Into<SharedString>, impl Into<SharedString>),
    choices: [Choice; N],
    focus: &[FocusHandle; N],
    selected: usize,
    pick: impl Fn(usize, &mut App) + Clone + 'static,
    theme: &Theme,
) -> AnyElement {
    let options = choices.into_iter().enumerate().map(|(ix, choice)| {
        let chosen = ix == selected;
        let pick = pick.clone();
        let el = a11y::focusable(
            a11y::node(choice.id, Role::RadioButton, choice.label.clone()),
            &focus[ix],
            theme,
            chosen,
        )
        .aria_toggled(if chosen {
            gpui_kit::accesskit::Toggled::True
        } else {
            gpui_kit::accesskit::Toggled::False
        })
        .aria_position_in_set(ix + 1)
        .aria_size_of_set(N)
        .on_click(move |_, _, cx| pick(ix, cx))
        .cursor_pointer()
        .min_h(theme.text(26.).max(px(24.)))
        .px(px(10.))
        .flex()
        .items_center()
        .text_size(theme.text(13.))
        .rounded(px(6.));
        let el = match choice.description {
            Some(description) => el.aria_description(description),
            None => el,
        };
        if chosen {
            el.bg(theme.accent)
                .text_color(theme.on_accent)
                .font_weight(FontWeight::SEMIBOLD)
        } else {
            el.text_color(theme.text_secondary)
        }
        .child(choice.label)
    });
    a11y::node(id, Role::RadioGroup, label)
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.control_border)
        .children(options)
        .into_any_element()
}
