//! The one radio group and the checkbox row (`docs/design/settings-icons-blur.md` section 3).
//! Every radio in the app is a [`RadioGroup`], so every circle sits at the same place: 14 pt
//! from the row's left edge and centered on the label's first line, never on the row. A row with
//! a description is taller, and centering on the row is what once put the Microphone radios out
//! of line with Transcription's.
//!
//! The group is one Tab stop. The arrows move the selection and wrap, Space selects, and the
//! focus ring goes on the radio. The selected row is the group's active descendant, so
//! VoiceOver reads "Microphone, radio group", then "Automatic, selected, 1 of 4".

use gpui_kit::{
    AnyElement, App, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Pixels, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Window, div, prelude::FluentBuilder as _, px, relative,
};

use crate::a11y;
use crate::icons::{self, Icon};
use crate::theme::Theme;

/// The control's size and its distance from the row's left edge.
pub const RADIO: f32 = 16.;
pub const INSET: f32 = 14.;
/// The label's first line, which the radio and a trailing control center on.
pub const LINE: f32 = 18.;
/// From the radio to the label.
pub const GAP: f32 = 10.;
const TRAILING: f32 = 24.;

/// One option of a [`RadioGroup`].
pub struct Radio {
    id: SharedString,
    label: SharedString,
    heading: Option<SharedString>,
    description: Option<SharedString>,
    warning: Option<SharedString>,
    enabled: bool,
    trailing: Option<AnyElement>,
}

impl Radio {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Radio {
            id: id.into(),
            label: label.into(),
            heading: None,
            description: None,
            warning: None,
            enabled: true,
            trailing: None,
        }
    }

    /// A title over this row and the ones after it, such as "Apple Speech" over its languages.
    pub fn heading(mut self, text: Option<impl Into<SharedString>>) -> Self {
        self.heading = text.map(Into::into);
        self
    }

    /// The effect, in one line (UX-20).
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        self.description = (!text.is_empty()).then_some(text);
        self
    }

    /// A limit worth noticing, in `attention`: "Doesn't support Tagalog".
    pub fn warning(mut self, text: Option<impl Into<SharedString>>) -> Self {
        self.warning = text.map(Into::into);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A control at the row's right end, such as the speech model's info button. Clicks on it do
    /// not select the row.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

pub struct RadioGroup {
    id: SharedString,
    label: SharedString,
    options: Vec<Radio>,
    selected: Option<usize>,
}

/// The enabled option `step` places from `from`, wrapping, or `from` when none is enabled.
pub fn step(enabled: &[bool], from: Option<usize>, step: isize) -> Option<usize> {
    let n = enabled.len() as isize;
    if n == 0 || !enabled.iter().any(|e| *e) {
        return from;
    }
    let mut at = match from {
        Some(ix) => ix as isize,
        None if step > 0 => -1,
        None => n,
    };
    loop {
        at = (at + step).rem_euclid(n);
        if enabled[at as usize] {
            return Some(at as usize);
        }
    }
}

impl RadioGroup {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        RadioGroup {
            id: id.into(),
            label: label.into(),
            options: Vec::new(),
            selected: None,
        }
    }

    pub fn option(mut self, option: Radio) -> Self {
        self.options.push(option);
        self
    }

    pub fn selected(mut self, ix: Option<usize>) -> Self {
        self.selected = ix;
        self
    }

    pub fn render(
        self,
        focus: &FocusHandle,
        theme: &Theme,
        window: &Window,
        on_select: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    ) -> a11y::Node {
        let enabled: Vec<bool> = self.options.iter().map(|o| o.enabled).collect();
        let selected = self.selected;
        let current = selected.or_else(|| step(&enabled, None, 1));
        let ring = focus.is_focused(window) && window.last_input_was_keyboard();
        let count = self.options.len();
        let on_key = on_select.clone();
        let keys_enabled = enabled.clone();
        let mut group = a11y::node(self.id, Role::RadioGroup, self.label)
            .track_focus(&focus.clone().tab_stop(true))
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let next = match event.keystroke.key.as_str() {
                    "down" | "right" => step(&keys_enabled, selected, 1),
                    "up" | "left" => step(&keys_enabled, selected, -1),
                    "space" => selected.or_else(|| step(&keys_enabled, None, 1)),
                    _ => return,
                };
                cx.stop_propagation();
                if let Some(next) = next
                    && Some(next) != selected
                {
                    on_key(next, window, cx);
                }
            })
            .flex()
            .flex_col();
        for (ix, option) in self.options.into_iter().enumerate() {
            if let Some(heading) = option.heading.clone() {
                group = group.child(
                    a11y::node(
                        format!("heading-{}", option.id),
                        Role::Heading,
                        heading.clone(),
                    )
                    .border_t_1()
                    .border_color(theme.divider)
                    .pl(px(INSET))
                    .pt(px(11.))
                    .text_size(theme.text(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .child(heading),
                );
            }
            let on = selected == Some(ix);
            let select = on_select.clone();
            let group_focus = focus.clone();
            let mut spoken: Vec<SharedString> = Vec::new();
            spoken.extend(option.description.clone());
            spoken.extend(option.warning.clone());
            let spoken = spoken.join(". ");
            let row = a11y::node(option.id, Role::RadioButton, option.label.clone())
                .aria_toggled(if on {
                    gpui_kit::accesskit::Toggled::True
                } else {
                    gpui_kit::accesskit::Toggled::False
                })
                .aria_position_in_set(ix + 1)
                .aria_size_of_set(count)
                .when(!spoken.is_empty(), |row| row.aria_description(spoken))
                .when(on, |row| row.aria_active_descendant())
                .when(option.enabled, |row| {
                    row.cursor_pointer().on_click(move |_, window, cx| {
                        window.focus(&group_focus, cx);
                        if !on {
                            select(ix, window, cx);
                        }
                    })
                })
                .when(ix > 0 && option.heading.is_none(), |row| {
                    row.border_t_1().border_color(theme.divider)
                });
            let indicator = circle(on, option.enabled, ring && current == Some(ix), theme);
            group = group.child(layout(
                row,
                indicator,
                option.label,
                option.description,
                option.warning,
                option.enabled,
                option.trailing,
                theme,
            ));
        }
        group
    }
}

/// The shared row: control, then the label with its description below, then an optional
/// trailing control, each centered on the label's first line.
#[allow(clippy::too_many_arguments)]
fn layout(
    row: a11y::Node,
    control: gpui_kit::Div,
    label: SharedString,
    description: Option<SharedString>,
    warning: Option<SharedString>,
    enabled: bool,
    trailing: Option<AnyElement>,
    theme: &Theme,
) -> a11y::Node {
    let line = theme.text(LINE);
    let trailing_h = line.max(px(TRAILING));
    row.w_full()
        .flex()
        .items_start()
        .gap(px(GAP))
        .pl(px(INSET))
        .pr(px(INSET))
        .py(px(11.))
        .child(
            div().flex_none().h(line).flex().items_center().child(
                div()
                    .id("control")
                    .test_support()
                    .size(px(RADIO))
                    .child(control),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .id("label")
                        .test_support()
                        .text_size(theme.text(13.))
                        .line_height(line)
                        .font_weight(FontWeight::MEDIUM)
                        .when(!enabled, |d| d.text_color(theme.text_secondary))
                        .child(label),
                )
                .children(description.map(|text| {
                    div()
                        .text_size(theme.text(12.))
                        .line_height(relative(1.4))
                        .text_color(theme.text_secondary)
                        .child(text)
                }))
                .children(warning.map(|text| {
                    icons::beside(
                        Icon::TriangleAlert,
                        12.,
                        theme.attention,
                        theme.text(12.) * 1.4,
                        text,
                    )
                    .text_size(theme.text(12.))
                    .text_color(theme.attention)
                })),
        )
        .children(trailing.map(|trailing| {
            div()
                .id("trailing")
                .flex_none()
                .min_h(trailing_h)
                .mt((line - trailing_h) / 2.)
                .flex()
                .items_center()
                .gap(px(8.))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(trailing)
        }))
}

/// The 16 pt circle: a `radio_border` ring on `control_bg`, or an accent fill with a 6 pt white
/// dot. The focus ring is 2 pt `ring`, 2 pt out from the circle.
fn circle(on: bool, enabled: bool, focused: bool, theme: &Theme) -> gpui_kit::Div {
    div()
        .relative()
        .size(px(RADIO))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .when(on, |c| c.bg(theme.accent).border_color(theme.accent))
        .when(!on, |c| {
            c.bg(theme.control_bg).border_color(if enabled {
                theme.radio_border
            } else {
                theme.divider
            })
        })
        .when(on, |c| {
            c.child(div().size(px(6.)).rounded_full().bg(theme.on_accent))
        })
        .when(focused, |c| c.child(ring_around(px(RADIO), true, theme)))
}

fn ring_around(size: Pixels, round: bool, theme: &Theme) -> gpui_kit::Div {
    div()
        .absolute()
        .top(px(-5.))
        .left(px(-5.))
        .size(size + px(8.))
        .when(round, |r| r.rounded_full())
        .when(!round, |r| r.rounded(px(7.)))
        .border_2()
        .border_color(theme.ring)
}

/// A checkbox row with the radio's metrics, described by the same [`Radio`] builder, so a
/// checkbox and a radio group line up in one pane. The row is its own Tab stop with the ring on
/// the box; Space or a click toggles it.
pub fn checkbox(
    option: Radio,
    on: bool,
    focus: &FocusHandle,
    theme: &Theme,
    window: &Window,
    press: impl Fn(&mut Window, &mut App) + 'static,
) -> a11y::Node {
    let ring = focus.is_focused(window) && window.last_input_was_keyboard();
    let mut spoken: Vec<SharedString> = Vec::new();
    spoken.extend(option.description.clone());
    spoken.extend(option.warning.clone());
    let spoken = spoken.join(". ");
    let row = a11y::node(option.id, Role::CheckBox, option.label.clone())
        .track_focus(&focus.clone().tab_stop(true))
        .aria_toggled(if on {
            gpui_kit::accesskit::Toggled::True
        } else {
            gpui_kit::accesskit::Toggled::False
        })
        .when(!spoken.is_empty(), |row| row.aria_description(spoken))
        .when(option.enabled, |row| {
            row.cursor_pointer()
                .on_click(move |_, window, cx| press(window, cx))
        });
    let tick = div()
        .relative()
        .size(px(RADIO))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .when(on, |b| {
            b.bg(theme.accent)
                .border_color(theme.accent)
                .child(Icon::Check.view(12., theme.on_accent))
        })
        .when(!on, |b| {
            b.bg(theme.control_bg).border_color(theme.radio_border)
        })
        .when(ring, |b| b.child(ring_around(px(RADIO), false, theme)));
    layout(
        row,
        tick,
        option.label,
        option.description,
        option.warning,
        option.enabled,
        option.trailing,
        theme,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_wrap_and_skip_disabled_options() {
        let enabled = [true, false, true, true];
        assert_eq!(step(&enabled, Some(0), 1), Some(2));
        assert_eq!(step(&enabled, Some(3), 1), Some(0), "down wraps");
        assert_eq!(step(&enabled, Some(0), -1), Some(3), "up wraps");
        assert_eq!(step(&enabled, Some(2), -1), Some(0));
        assert_eq!(step(&enabled, None, 1), Some(0));
        assert_eq!(step(&[false, false], Some(1), 1), Some(1));
    }
}
