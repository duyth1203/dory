//! The floating-surface family (`docs/design/floating-surfaces.md` section 3): modal dialogs,
//! popovers (the model explainer and the combobox list), menus, toasts, and the Processing
//! window, which takes a dialog's surface, header, and footer without the scrim.
//!
//! Every surface is opaque `surface_raised` with a 1 pt `surface_border`, its kind's radius, and
//! its kind's shadow ([`Theme::shadow`]), so nothing can drift back to a private fill. Increase
//! Contrast's border and missing shadow come through the theme.
//!
//! Modal dialogs open on a host built here from GPUI Base's unstyled `Dialog`, not Kit's, for two
//! things Kit 0.9.1 cannot do: name the dialog for VoiceOver with its title
//! (`docs/plans/upstream/kit-dialog-has-no-accessible-name.md`), and cast `shadow_float` in place
//! of its fixed 10% shadow. The host dims the whole window with `scrim` (`scrim_opaque` under
//! Reduce Transparency), traps Tab, closes on Escape or a click on the scrim, and gives focus back
//! to whatever opened it. The window's root view draws [`dialog_layer`].
//!
//! Motion follows the section 3 table through [`entrance`], with each Reduce Motion stand-in.
//! GPUI draws no transforms, so the dialog's 97% to 100% size change is left out and it fades.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyView, App, ClickEvent, Div, FocusHandle, FontWeight, Global, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled,
    Window, WindowId, div, prelude::FluentBuilder as _, px,
};

use crate::a11y::{self, Node};
use crate::icons::Icon;
use crate::theme::{Elevation, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dialog,
    Popover,
    Menu,
    Toast,
}

impl Kind {
    pub fn radius(self) -> f32 {
        match self {
            Kind::Dialog => 14.,
            Kind::Popover => 10.,
            Kind::Menu => 8.,
            Kind::Toast => 12.,
        }
    }

    pub fn elevation(self) -> Elevation {
        match self {
            Kind::Dialog => Elevation::Float,
            _ => Elevation::Pop,
        }
    }
}

/// Paints `el` as a `kind` surface: fill, border, radius, and shadow.
pub fn paint<E: Styled>(el: E, kind: Kind, theme: &Theme) -> E {
    el.bg(theme.surface_raised)
        .border_1()
        .border_color(theme.surface_border)
        .rounded(px(kind.radius()))
        .shadow(theme.shadow(kind.elevation()))
        .text_color(theme.text)
}

/// A card inside a surface: the running job, a setup step.
pub fn inset<E: Styled>(el: E, theme: &Theme) -> E {
    el.bg(theme.surface_inset)
        .border_1()
        .border_color(theme.inset_border)
        .rounded(px(10.))
}

/// One frame of a surface's entrance: its opacity, and how far it still sits from its resting
/// place, in points, toward its anchor (a popover) or below it (a toast).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub opacity: f32,
    pub travel: f32,
}

impl Frame {
    pub const REST: Frame = Frame {
        opacity: 1.,
        travel: 0.,
    };
}

/// The open motion of the section 3 table: how long, and how far it travels. Menus appear at
/// once, like macOS menus.
pub fn opening(kind: Kind, reduce_motion: bool) -> (Duration, f32) {
    let ms = |n| Duration::from_millis(n);
    match (kind, reduce_motion) {
        (Kind::Menu, _) => (Duration::ZERO, 0.),
        (Kind::Dialog, false) => (ms(180), 0.),
        (Kind::Dialog, true) => (ms(120), 0.),
        (Kind::Popover, false) => (ms(120), 4.),
        (Kind::Popover, true) => (ms(120), 0.),
        (Kind::Toast, false) => (ms(200), 8.),
        (Kind::Toast, true) => (ms(200), 0.),
    }
}

/// The scrim fades in over 150 ms, Reduce Motion or not: a fade is not movement.
pub const SCRIM_FADE: Duration = Duration::from_millis(150);

/// Where a `kind` surface opened `elapsed` ago stands now, on `ease_standard`.
pub fn entrance(kind: Kind, elapsed: Duration, reduce_motion: bool) -> Frame {
    let (over, travel) = opening(kind, reduce_motion);
    if over.is_zero() || elapsed >= over {
        return Frame::REST;
    }
    let t = crate::motion::ease_standard(elapsed.as_secs_f32() / over.as_secs_f32());
    Frame {
        opacity: t,
        travel: travel * (1. - t),
    }
}

/// [`entrance`] for a surface that opened at `since`, asking for the next frame while it runs.
pub fn animate(kind: Kind, since: Option<Instant>, window: &mut Window, cx: &App) -> Frame {
    let Some(since) = since else {
        return Frame::REST;
    };
    let frame = entrance(kind, since.elapsed(), cx.reduce_motion());
    if frame != Frame::REST {
        window.request_animation_frame();
    }
    frame
}

/// The look of a button in a surface's footer or body. All are 28 pt tall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Accent, at the far right.
    Primary,
    /// Bordered.
    Secondary,
    /// A text link, at the left of a footer.
    Link,
    /// A primary button that cannot be pressed yet: `track` with secondary text.
    Disabled,
}

/// A button that takes focus and activates with a click, Return, or Space. Return is also a
/// dialog's Confirm key, so the button answers `Confirm` itself or the dialog would take it.
pub fn button(
    id: impl Into<SharedString>,
    spoken: impl Into<SharedString>,
    text: impl Into<SharedString>,
    look: Look,
    focus: &FocusHandle,
    theme: &Theme,
    press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> Node {
    let on_confirm = press.clone();
    let enabled = look != Look::Disabled;
    let el = a11y::node(id, Role::Button, spoken);
    let el = if enabled {
        a11y::focusable(el, focus, theme, look == Look::Primary)
            .on_click(move |_, window, cx| press(window, cx))
            .on_action(
                move |_: &gpui_kit::component::dialog::Confirm, window, cx| {
                    on_confirm(window, cx);
                    cx.stop_propagation();
                },
            )
    } else {
        el.border_2().border_color(gpui_kit::transparent_black())
    };
    el.flex_none()
        .min_h(px(28.))
        .min_w(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .text_size(theme.text(13.))
        .font_weight(FontWeight::MEDIUM)
        .map(|b| match look {
            Look::Primary => b.bg(theme.accent).text_color(theme.on_accent),
            Look::Secondary => b.bg(theme.control_bg).text_color(theme.text).child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(theme.control_border),
            ),
            Look::Link => b.px(px(4.)).text_color(theme.accent_text),
            Look::Disabled => b.bg(theme.track).text_color(theme.text_secondary),
        })
        .relative()
        .child(text.into())
}

/// The 28 by 28 close button with Lucide `x`, named "Close".
pub fn close_button(
    id: impl Into<SharedString>,
    focus: &FocusHandle,
    theme: &Theme,
    press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> Node {
    let on_confirm = press.clone();
    a11y::focusable(a11y::node(id, Role::Button, "Close"), focus, theme, false)
        .on_click(move |_, window, cx| press(window, cx))
        .on_action(
            move |_: &gpui_kit::component::dialog::Confirm, window, cx| {
                on_confirm(window, cx);
                cx.stop_propagation();
            },
        )
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.close_bg)
        .when(theme.high_contrast, |b| {
            b.child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.text_secondary),
            )
        })
        .relative()
        .child(Icon::X.view(14., theme.text_secondary))
}

/// A dialog's header: the title (15 semibold), an optional subtitle (13, secondary) that carries
/// a live summary, and the close button at the top right. Padding 18 / 16 / 14 / 20.
pub fn header(
    id: &str,
    title: &str,
    subtitle: Option<String>,
    close: Option<Node>,
    theme: &Theme,
) -> Div {
    div()
        .flex_none()
        .flex()
        .items_start()
        .gap(px(12.))
        .pt(px(18.))
        .pr(px(16.))
        .pb(px(14.))
        .pl(px(20.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    a11y::node(format!("{id}-title"), Role::Heading, title.to_owned())
                        .aria_level(1)
                        .text_size(theme.text(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_owned()),
                )
                .children(subtitle.map(|text| {
                    a11y::node(format!("{id}-subtitle"), Role::Label, text.clone())
                        .text_size(theme.text(13.))
                        .text_color(theme.text_secondary)
                        .child(text)
                })),
        )
        .children(close)
}

/// A dialog's footer: a 1 pt divider above, an optional link on the left, and the buttons on
/// the right, secondary then primary. Padding 12 20.
pub fn footer(left: Option<Node>, right: Vec<Node>, theme: &Theme) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(20.))
        .py(px(12.))
        .border_t_1()
        .border_color(if theme.high_contrast {
            theme.text_secondary
        } else {
            theme.divider
        })
        .children(left)
        .child(div().flex_1())
        .children(right)
}

struct Open {
    title: SharedString,
    view: AnyView,
    width: f32,
    focus: FocusHandle,
    /// What had focus when the dialog opened, which gets it back on close.
    restore: Option<FocusHandle>,
    opened: Instant,
}

#[derive(Default)]
struct Dialogs(HashMap<WindowId, Open>);

impl Global for Dialogs {}

/// Opens `view` as a modal dialog over `window`, named `title` for VoiceOver, at most `width`
/// wide. `first` takes focus. One dialog at a time: a new one replaces the old.
pub fn open_dialog(
    title: impl Into<SharedString>,
    width: f32,
    view: AnyView,
    first: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let id = window.window_handle().window_id();
    let restore = cx
        .try_global::<Dialogs>()
        .and_then(|d| d.0.get(&id))
        .map_or_else(|| window.focused(cx), |old| old.restore.clone());
    let open = Open {
        title: title.into(),
        view,
        width,
        focus: cx.focus_handle(),
        restore,
        opened: Instant::now(),
    };
    cx.default_global::<Dialogs>().0.insert(id, open);
    let first = first.clone();
    window.defer(cx, move |window, cx| window.focus(&first, cx));
    window.refresh();
}

/// Closes `window`'s dialog and gives focus back to what opened it.
pub fn close_dialog(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let Some(open) = cx.default_global::<Dialogs>().0.remove(&id) else {
        return;
    };
    if let Some(restore) = open.restore {
        window.focus(&restore, cx);
    }
    window.refresh();
}

pub fn has_dialog(window: &Window, cx: &App) -> bool {
    cx.try_global::<Dialogs>()
        .is_some_and(|d| d.0.contains_key(&window.window_handle().window_id()))
}

/// The open dialog of `window` and its scrim, for the window's root view to draw last.
pub fn dialog_layer(window: &mut Window, cx: &mut App) -> Option<impl IntoElement + use<>> {
    let id = window.window_handle().window_id();
    let (title, view, width, focus, opened) = {
        let open = cx.try_global::<Dialogs>()?.0.get(&id)?;
        (
            open.title.clone(),
            open.view.clone(),
            open.width,
            open.focus.clone(),
            open.opened,
        )
    };
    let theme = Theme::get(cx);
    let viewport = window.viewport_size();
    let frame = animate(Kind::Dialog, Some(opened), window, cx);
    let scrim_in = (opened.elapsed().as_secs_f32() / SCRIM_FADE.as_secs_f32()).min(1.);
    if scrim_in < 1. {
        window.request_animation_frame();
    }
    let scrim: Hsla = if crate::glass::reduces_transparency(cx) {
        theme.scrim_opaque
    } else {
        theme.scrim
    };
    let width = width.min(f32::from(viewport.width) - 48.).max(0.);
    let inset = if f32::from(viewport.height) < 640. {
        48.
    } else {
        80.
    };
    let max_height = (f32::from(viewport.height) - inset).max(0.);
    let close = |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        window.defer(cx, close_dialog);
        true
    };
    Some(
        gpui_kit::base::Dialog::new(cx)
            .accessibility_label(title)
            .focus_handle(focus)
            .layer(0, true)
            .on_cancel(close)
            .on_ok(|_, _, _| false)
            .backdrop(div().absolute().size_full().bg(Hsla {
                a: scrim.a * scrim_in,
                ..scrim
            }))
            .popup(paint(
                div()
                    .id("dialog-surface")
                    .occlude()
                    .w(px(width))
                    .max_h(px(max_height))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .opacity(frame.opacity)
                    .child(view),
                Kind::Dialog,
                &theme,
            )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_has_its_radius_and_shadow() {
        let got: Vec<_> = [Kind::Dialog, Kind::Popover, Kind::Menu, Kind::Toast]
            .into_iter()
            .map(|k| (k.radius(), k.elevation()))
            .collect();
        assert_eq!(
            got,
            [
                (14., Elevation::Float),
                (10., Elevation::Pop),
                (8., Elevation::Pop),
                (12., Elevation::Pop),
            ]
        );
    }

    #[test]
    fn entrances_follow_the_motion_table_and_reduce_motion_only_fades() {
        let at = |kind, ms, reduce| entrance(kind, Duration::from_millis(ms), reduce);
        assert_eq!(
            at(Kind::Menu, 0, false),
            Frame::REST,
            "menus appear at once"
        );
        assert_eq!(at(Kind::Dialog, 180, false), Frame::REST);
        assert_eq!(at(Kind::Dialog, 120, true), Frame::REST);
        assert!((at(Kind::Toast, 0, false).travel - 8.).abs() < 1e-3);
        assert!((at(Kind::Popover, 0, false).travel - 4.).abs() < 1e-3);
        assert!(at(Kind::Popover, 0, false).opacity < 1e-3);
        for kind in [Kind::Dialog, Kind::Popover, Kind::Toast] {
            let mid = at(kind, 60, true);
            assert_eq!(mid.travel, 0., "{kind:?} does not move under Reduce Motion");
            assert!(mid.opacity > 0. && mid.opacity < 1., "{kind:?} still fades");
        }
        let early = at(Kind::Toast, 50, false);
        let late = at(Kind::Toast, 150, false);
        assert!(early.travel > late.travel && early.opacity < late.opacity);
    }
}
