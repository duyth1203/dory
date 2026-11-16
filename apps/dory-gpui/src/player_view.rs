//! The player (handoff section 4, A11Y-3). Stub for agent 2, who owns this file together with
//! `transcript_view.rs`.
//!
//! Agent 2 builds, without editing any other file:
//! - The player card at the top of the page: a 48 to 64 pt waveform of 2 pt bars (played bars
//!   `accent`, the rest `wave_rest`, a 2 pt playhead in `text`), then `elapsed / total`, Back 5 s,
//!   Play (40 pt `accent` circle), Forward 5 s. The waveform is the slider: build it with
//!   `a11y::slider`, which already speaks "12 minutes 30 seconds of 48 minutes" and handles the
//!   arrow and page keys. No speed button: the `Player` port has no speed yet.
//! - The docked bar: the same controls on one line, 52 pt tall, in a `card_bg` rounded bar.
//!   `set_docked` switches between the two; the detail pane calls it when the card scrolls out
//!   of view. Both forms use the one `focus` handle, so focus moves with the dock and VoiceOver
//!   sees one slider. Cross-fade 150 ms unless Reduce Motion is on.
//! - Waveform peaks need audio data the core does not expose yet. Draw evenly spaced bars of a
//!   fixed height until it does, and say so in the PR.
//!
//! Playback state lives in `PlayerHandle` (see `app/player.rs`), shared with the transcript view
//! for click-to-seek. This view only draws it and calls `toggle`, `step`, and `seek`.

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::{
    Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render, Role, Styled as _,
    Window, div, px,
};

use crate::a11y;
use crate::app::PlayerHandle;
use crate::theme::Theme;

pub struct PlayerView {
    player: Entity<PlayerHandle>,
    focus: FocusHandle,
    play_focus: FocusHandle,
    docked: bool,
}

impl PlayerView {
    pub fn new(player: Entity<PlayerHandle>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            focus: cx.focus_handle(),
            play_focus: cx.focus_handle(),
            docked: false,
        }
    }

    pub fn set_docked(&mut self, docked: bool, cx: &mut Context<Self>) {
        self.docked = docked;
        cx.notify();
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
}

impl Render for PlayerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (position, total, playing, unavailable) = {
            let p = self.player.read(cx);
            (
                p.position(),
                p.duration(),
                p.is_playing(),
                p.unavailable().cloned(),
            )
        };
        let player = self.player.clone();
        let step = move |delta, _: &mut Window, cx: &mut gpui_kit::App| {
            player.update(cx, |p, cx| p.step(delta, cx));
        };
        let toggle = self.player.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(16.))
            .rounded(px(14.))
            .bg(theme.card_bg)
            .border_1()
            .border_color(theme.card_border)
            .child(
                a11y::slider(
                    "player",
                    "Playback position",
                    position,
                    total,
                    &self.focus,
                    &theme,
                    step,
                )
                .h(px(48.))
                .rounded(px(8.))
                .bg(theme.wave_rest.opacity(0.25)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "{} / {}",
                        a11y::clock(position),
                        a11y::clock(total)
                    ))
                    .child(
                        a11y::focusable(
                            a11y::node(
                                "play",
                                Role::Button,
                                if playing { "Pause" } else { "Play" },
                            ),
                            &self.play_focus,
                            &theme,
                            false,
                        )
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .text_color(theme.accent_text)
                        .on_click(move |_, _, cx| toggle.update(cx, |p, cx| p.toggle(cx)))
                        .child(if playing { "Pause" } else { "Play" }),
                    )
                    .children(unavailable),
            )
    }
}
