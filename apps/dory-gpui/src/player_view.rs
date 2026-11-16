//! The player (handoff section 4, A11Y-3).
//!
//! - The card at the top of the page: a 56 pt waveform of 2 pt bars with 2 pt gaps (played bars
//!   `accent`, the rest `wave_rest`, a 2 pt playhead in `text`), then `elapsed / total`, Back 5 s,
//!   Play (40 pt `accent` circle), Forward 5 s. No speed button: the `Player` port has no speed.
//! - The docked bar: Back, Play, Forward, the waveform, and the time on one 52 pt line in a
//!   `card_bg` rounded bar. `set_docked` switches between the two; the detail pane calls it when
//!   the card scrolls out of view. Both forms draw the same element ids around the one `focus`
//!   handle, so focus moves with the dock and VoiceOver sees one slider. The new form fades in
//!   over 150 ms unless Reduce Motion is on.
//! - The waveform is the slider, built with `a11y::slider`: it speaks "12 minutes 30 seconds of
//!   48 minutes", the arrows move 5 s and Page Up and Page Down 1 min, and Space plays or pauses.
//!   Clicking or dragging it seeks.
//! - Each bar's height is the take's loudness there, from `PlayerHandle::waveform`, resampled to
//!   however many bars fit the width. Until the waveform is read, or when there is no take, every
//!   bar sits at the silence floor: a calm flat line, never a shape the audio does not have.
//! - When the audio cannot open (no take, or a file the platform player refuses, such as legacy
//!   WebM), the player says why next to the time, from `PlayerHandle::unavailable`.
//!
//! Playback state lives in `PlayerHandle` (see `app/player.rs`), shared with the transcript view
//! for click-to-seek. This view only draws it and calls `toggle`, `step`, and `seek`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::domain::{Ms, Waveform};
use gpui_kit::{
    App, Bounds, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, fill,
    point, prelude::FluentBuilder as _, px, size,
};

use crate::a11y::{self, Node};
use crate::app::PlayerHandle;
use crate::theme::Theme;

/// Back and Forward move as far as the slider's arrows (A11Y-3).
pub const SKIP_MS: i64 = 5_000;
const FADE: Duration = Duration::from_millis(150);
const BAR: f32 = 2.;
const GAP: f32 = 2.;

#[derive(Clone, Copy)]
enum Control {
    Back,
    Play,
    Forward,
}

pub struct PlayerView {
    player: Entity<PlayerHandle>,
    focus: FocusHandle,
    controls: [FocusHandle; 3],
    docked: bool,
    shown_at: Option<Instant>,
    wave: Rc<Cell<Bounds<Pixels>>>,
    painted: Rc<RefCell<Vec<f32>>>,
}

impl PlayerView {
    pub fn new(player: Entity<PlayerHandle>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        player.update(cx, |player, cx| player.load_waveform(cx));
        Self {
            player,
            focus: cx.focus_handle(),
            controls: [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()],
            docked: false,
            shown_at: None,
            wave: Rc::default(),
            painted: Rc::default(),
        }
    }

    /// The bar heights of the last frame, each 0..=1 of the waveform's height.
    pub fn bar_heights(&self) -> Vec<f32> {
        self.painted.borrow().clone()
    }

    pub fn set_docked(&mut self, docked: bool, cx: &mut Context<Self>) {
        if docked == self.docked {
            return;
        }
        self.docked = docked;
        self.shown_at = (!cx.reduce_motion()).then(Instant::now);
        cx.notify();
    }

    pub fn is_docked(&self) -> bool {
        self.docked
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn seek_to_x(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let wave = self.wave.get();
        if wave.size.width <= px(0.) {
            return;
        }
        let fraction = ((x - wave.origin.x) / wave.size.width).clamp(0., 1.);
        let total = self.player.read(cx).duration();
        let to = Ms((total.0 as f64 * fraction as f64).round() as u64);
        self.player.update(cx, |player, cx| player.seek(to, cx));
    }

    fn waveform(&self, position: Ms, total: Ms, theme: &Theme, cx: &mut Context<Self>) -> Node {
        let player = self.player.clone();
        let step = move |delta, _: &mut Window, cx: &mut App| {
            player.update(cx, |player, cx| player.step(delta, cx));
        };
        let played = if total.0 == 0 {
            0.
        } else {
            position.0 as f32 / total.0 as f32
        };
        let (played_color, rest_color, head_color) = (theme.accent, theme.wave_rest, theme.text);
        let wave = self.wave.clone();
        let painted = self.painted.clone();
        let waveform: Arc<Waveform> = self.player.read(cx).waveform().unwrap_or_default();
        a11y::slider(
            "player",
            "Playback position",
            position,
            total,
            &self.focus,
            theme,
            step,
        )
        .aria_keyshortcuts("Space")
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "space" {
                this.player.update(cx, |player, cx| player.toggle(cx));
                cx.stop_propagation();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.seek_to_x(event.position.x, cx);
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                this.seek_to_x(event.position.x, cx);
            }
        }))
        .rounded(px(8.))
        .cursor_pointer()
        .child(
            canvas(
                move |bounds, _, _| wave.set(bounds),
                move |bounds, _, window, _| {
                    let head = bounds.origin.x + bounds.size.width * played;
                    let bars = ((bounds.size.width + px(GAP)) / px(BAR + GAP)).floor() as usize;
                    let heights = waveform.bars(bars);
                    for (i, fraction) in heights.iter().enumerate() {
                        let height = (bounds.size.height * *fraction).max(px(BAR));
                        let top = bounds.origin.y + (bounds.size.height - height) * 0.5;
                        let x = bounds.origin.x + px(i as f32 * (BAR + GAP));
                        let color = if x < head { played_color } else { rest_color };
                        window.paint_quad(fill(
                            Bounds::new(point(x, top), size(px(BAR), height)),
                            color,
                        ));
                    }
                    *painted.borrow_mut() = heights;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(head - px(1.), bounds.origin.y),
                            size(px(2.), bounds.size.height),
                        ),
                        head_color,
                    ));
                },
            )
            .size_full(),
        )
    }

    fn control(&self, control: Control, playing: bool, theme: &Theme) -> Node {
        let (ix, id, label, glyph): (usize, &str, &str, SharedString) = match control {
            Control::Back => (0, "player-back", "Back 5 seconds", "↺5".into()),
            Control::Play if playing => (1, "player-play", "Pause", "❚❚".into()),
            Control::Play => (1, "player-play", "Play", "▶".into()),
            Control::Forward => (2, "player-forward", "Forward 5 seconds", "5↻".into()),
        };
        let filled = matches!(control, Control::Play);
        let activate = {
            let player = self.player.clone();
            move |cx: &mut App| {
                player.update(cx, |player, cx| match control {
                    Control::Back => player.step(-SKIP_MS, cx),
                    Control::Play => player.toggle(cx),
                    Control::Forward => player.step(SKIP_MS, cx),
                })
            }
        };
        let button = a11y::focusable(
            a11y::node(id, Role::Button, label),
            &self.controls[ix],
            theme,
            filled,
        )
        .on_click(move |_, _, cx| activate(cx))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .font_weight(FontWeight::SEMIBOLD);
        if filled {
            button
                .min_w(px(40.))
                .min_h(px(40.))
                .px(px(4.))
                .bg(theme.accent)
                .text_color(theme.on_accent)
                .text_size(theme.text(15.))
                .child(glyph)
        } else {
            button
                .min_w(px(32.))
                .min_h(px(32.))
                .px(px(4.))
                .text_color(theme.text)
                .text_size(theme.text(13.))
                .child(glyph)
        }
    }
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let opacity = match self.shown_at.map(|at| at.elapsed()) {
            Some(elapsed) if elapsed < FADE => {
                window.request_animation_frame();
                elapsed.as_secs_f32() / FADE.as_secs_f32()
            }
            _ => 1.,
        };
        let time_text = format!("{} / {}", a11y::clock(position), a11y::clock(total));
        let time = a11y::node("player-time", Role::Label, time_text.clone())
            .flex_none()
            .text_size(theme.text(12.))
            .text_color(theme.text_secondary)
            .child(time_text);
        let why = unavailable.map(|why| {
            a11y::node("player-unavailable", Role::Status, why.clone())
                .min_w(px(0.))
                .text_size(theme.text(12.))
                .text_color(theme.attention)
                .child(format!("⚠︎ {why}"))
        });
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(self.control(Control::Back, playing, &theme))
            .child(self.control(Control::Play, playing, &theme))
            .child(self.control(Control::Forward, playing, &theme));
        let wave = self.waveform(position, total, &theme, cx);
        let root = a11y::node("player-root", Role::Group, "Player")
            .opacity(opacity)
            .bg(theme.card_bg)
            .border_1()
            .border_color(theme.card_border);
        if self.docked {
            root.min_h(px(52.))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .rounded(px(14.))
                .when(theme.shadows, |bar| bar.shadow_md())
                .child(controls)
                .child(wave.flex_1().min_w(px(0.)).h(px(28.)))
                .child(time)
                .children(why)
        } else {
            root.flex()
                .flex_col()
                .gap(px(8.))
                .p(px(16.))
                .rounded(px(14.))
                .child(wave.h(px(56.)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.))
                                .child(time)
                                .children(why),
                        )
                        .child(controls)
                        .child(div().flex_1()),
                )
        }
    }
}
