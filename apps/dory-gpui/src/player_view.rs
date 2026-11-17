//! The one card at the top of a recording: the processing card from Stop until the summary is
//! ready, then the player in the same place (`docs/design/processing-view.md`, handoff section 4,
//! UX-12, UX-13, A11Y-3). The waveform stays where it is, so nothing jumps at the hand-over.
//!
//! - Processing: the headline row (Move to Top while others are ahead), the detail line, the
//!   Light chip with Use Fast This Time, the fix button when it failed (it takes focus when the
//!   failure appears), the waveform filling as transcription passes through the audio with a 2 pt
//!   reading head at the frontier, the caption in meeting time, and the step row. One
//!   `role=status` element holds the VoiceOver sentence.
//! - Player: the waveform is the slider, built with `a11y::slider`: it speaks "12 minutes 30
//!   seconds of 48 minutes", the arrows move 5 s and Page Up and Page Down 1 min, and Space plays
//!   or pauses. Clicking or dragging it seeks. Below it, `elapsed / total`, Back 5 s, Play, and
//!   Forward 5 s. No speed button: the `Player` port has no speed.
//! - The docked bar: Back, Play, Forward, the waveform, and the time on one 52 pt line.
//!   `set_docked` switches forms; the detail pane calls it once the card scrolls out of view.
//!   Only the player docks.
//!
//! Both forms draw one bar per 6 pt, at most 110, 2 pt apart, from `PlayerHandle::waveform`, and
//! the silence floor until it is read. Motion follows section 5 of the spec, with the Reduce
//! Motion stand-ins, through `motion`.
//!
//! Playback state lives in `PlayerHandle` and processing state in `AppState`; this view draws
//! them and calls their methods.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::domain::{Ms, RecordingId, Waveform};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, Pixels, Render, Role, StatefulInteractiveElement as _,
    Styled as _, Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, quad, relative,
    size,
};

use crate::a11y::{self, Node};
use crate::app::{AppState, PlayerHandle};
use crate::detail::{button, spoken_button};
use crate::glyphs;
use crate::icons::{self, Icon};
use crate::library;
use crate::motion::{self, ease_standard, elapsed, mix};
use crate::processing::{Fix, Phase, Problem, Step, StepGlyph, light_chip};
use crate::theme::Theme;

/// Back and Forward move as far as the slider's arrows (A11Y-3).
pub const SKIP_MS: i64 = 5_000;
/// The transport icons' size, which their 1.5-stroke copies are drawn for.
const TRANSPORT: f32 = 20.;
const GAP: f32 = 2.;
/// One bar per this much width.
const BAR_PITCH: f32 = 6.;
const MAX_BARS: usize = 110;
const WAVE_H: f32 = 48.;
/// The reading head reaches this far above and below the bars.
const HEAD_OVERHANG: f32 = 4.;

#[derive(Clone, Copy)]
enum Control {
    Back,
    Play,
    Forward,
}

/// Where the reading head moves from and to, and over how long: the measured interval between
/// progress updates, so it glides at the pace the engine reports.
#[derive(Clone, Copy, Debug, Default)]
struct Head {
    from: f32,
    to: f32,
    at: Option<Instant>,
    over: Duration,
}

impl Head {
    fn now(&self) -> f32 {
        self.from + (self.to - self.from) * elapsed(self.at, self.over)
    }

    fn moving(&self) -> bool {
        elapsed(self.at, self.over) < 1.0
    }

    fn move_to(&mut self, to: f32, tween: bool) {
        if (to - self.to).abs() < f32::EPSILON {
            return;
        }
        let now = Instant::now();
        let over = self
            .at
            .map(|at| now - at)
            .unwrap_or(Duration::from_secs(1))
            .clamp(Duration::from_millis(200), Duration::from_secs(3));
        *self = Head {
            from: if tween { self.now() } else { to },
            to,
            at: tween.then_some(now),
            over,
        };
    }
}

/// What changed when, so each change animates once (section 5).
#[derive(Default)]
struct Timeline {
    last: Option<Phase>,
    /// The headline and detail before the last state change, and when it changed.
    text_from: Option<(Option<String>, Option<String>, Instant)>,
    head: Head,
    /// Where transcription stood before the last update: bars past it take the done color.
    settled: f32,
    /// When the bars changed color at the last update.
    colored_at: Option<Instant>,
    /// When the levels first showed in the card, for the bars' arrival.
    arrived: Option<Instant>,
    failed_at: Option<Instant>,
    chip_at: Option<Instant>,
    handover_at: Option<Instant>,
    steps: [Option<(StepGlyph, Instant)>; 3],
}

impl Timeline {
    fn any_running(&self) -> bool {
        let running = |at: Option<Instant>, over| elapsed(at, over) < 1.0;
        self.head.moving()
            || running(self.colored_at, motion::BASE)
            || running(self.arrived, motion::SLOW + motion::FAST)
            || running(self.failed_at, motion::BASE + motion::SLOW)
            || running(self.chip_at, motion::SLOW)
            || running(self.handover_at, motion::SLOW)
            || self
                .text_from
                .as_ref()
                .is_some_and(|(_, _, at)| running(Some(*at), motion::FAST * 2))
            || self
                .steps
                .iter()
                .any(|s| s.is_some_and(|(_, at)| running(Some(at), motion::FAST)))
    }
}

pub struct PlayerView {
    player: Entity<PlayerHandle>,
    app: Entity<AppState>,
    focus: FocusHandle,
    controls: [FocusHandle; 3],
    fix_focus: FocusHandle,
    top_focus: FocusHandle,
    fast_focus: FocusHandle,
    docked: bool,
    shown_at: Option<Instant>,
    wave: Rc<Cell<Bounds<Pixels>>>,
    painted: Rc<RefCell<Vec<f32>>>,
    timeline: Timeline,
}

impl PlayerView {
    pub fn new(
        player: Entity<PlayerHandle>,
        app: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        player.update(cx, |player, cx| player.load_waveform(cx));
        Self {
            player,
            app,
            focus: cx.focus_handle(),
            controls: [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()],
            fix_focus: cx.focus_handle(),
            top_focus: cx.focus_handle(),
            fast_focus: cx.focus_handle(),
            docked: false,
            shown_at: None,
            wave: Rc::default(),
            painted: Rc::default(),
            timeline: Timeline::default(),
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

    /// Notes what changed since the last frame, so each change animates once, and moves focus
    /// to the fix button when a failure appears.
    fn observe(&mut self, phase: &Phase, has_levels: bool, window: &mut Window, cx: &mut App) {
        let still = cx.reduce_motion();
        let now = Instant::now();
        let start = |at: &mut Option<Instant>| *at = (!still).then_some(now);
        let t = &mut self.timeline;
        let before = t.last.replace(phase.clone());
        let processing = phase.is_processing();
        if processing && has_levels && t.arrived.is_none() {
            t.arrived = Some(if still || before.is_none() {
                now - motion::SLOW * 4
            } else {
                now
            });
        }
        let frontier = phase.transcribed().get();
        if (frontier - t.head.to).abs() > f32::EPSILON {
            t.settled = if before.is_none() {
                frontier
            } else {
                t.head.to
            };
            start(&mut t.colored_at);
            t.head.move_to(frontier, !still && before.is_some());
        }
        let Some(before) = before else {
            return;
        };
        if before.headline() != phase.headline() && processing {
            t.text_from = (!still).then(|| (before.headline(), before.detail(None), now));
        }
        let failed = |p: &Phase| match p {
            Phase::Failed(problem) => Some(std::mem::discriminant(problem)),
            _ => None,
        };
        if failed(phase).is_some() && failed(phase) != failed(&before) {
            start(&mut t.failed_at);
            if phase.fix().is_some() && processing && !a11y::typing(window, cx) {
                window.focus(&self.fix_focus, cx);
            }
        }
        if phase.light().is_some() && before.light().is_none() {
            start(&mut t.chip_at);
        }
        if before.is_processing() && !processing {
            start(&mut t.handover_at);
        }
        if let (Some(old), Some(new)) = (before.steps(), phase.steps()) {
            for (slot, (old, new)) in t.steps.iter_mut().zip(old.iter().zip(new.iter())) {
                if old.glyph != new.glyph && !still {
                    *slot = Some((new.glyph, now));
                }
            }
        }
    }

    /// The bars, painted in one canvas: `paint_bar` gives each bar's color and whether it is an
    /// outline, from its center as a fraction of the width.
    fn bars(
        &self,
        waveform: Arc<Waveform>,
        arrive: Option<Instant>,
        paint_bar: impl Fn(f32) -> (Hsla, bool) + 'static,
        head: Option<(f32, Hsla)>,
    ) -> impl IntoElement {
        let wave = self.wave.clone();
        let painted = self.painted.clone();
        canvas(
            move |bounds, _, _| wave.set(bounds),
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let count = ((width / BAR_PITCH).floor() as usize).clamp(1, MAX_BARS);
                let bar_w = ((width - GAP * (count as f32 - 1.)) / count as f32).max(1.);
                let heights = waveform.bars(count);
                for (i, fraction) in heights.iter().enumerate() {
                    let height = (bounds.size.height * *fraction).max(px(2.));
                    let top = bounds.origin.y + (bounds.size.height - height) * 0.5;
                    let x = bounds.origin.x + px(i as f32 * (bar_w + GAP));
                    let center = (i as f32 + 0.5) / count as f32;
                    let (mut color, outline) = paint_bar(center);
                    if let Some(at) = arrive {
                        let since = at.elapsed().saturating_sub(motion::STAGGER * i as u32);
                        color.a *= ease_standard(since.as_secs_f32() / motion::FAST.as_secs_f32());
                    }
                    let rect = Bounds::new(point(x, top), size(px(bar_w), height));
                    if outline {
                        window.paint_quad(quad(
                            rect,
                            px(2.),
                            gpui_kit::transparent_black(),
                            px(1.),
                            color,
                            Default::default(),
                        ));
                    } else {
                        window.paint_quad(fill(rect, color).corner_radii(px(2.)));
                    }
                }
                *painted.borrow_mut() = heights;
                if let Some((at, color)) = head {
                    let x = bounds.origin.x + bounds.size.width * at;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x - px(1.), bounds.origin.y - px(HEAD_OVERHANG)),
                            size(px(2.), bounds.size.height + px(HEAD_OVERHANG * 2.)),
                        ),
                        color,
                    ));
                }
            },
        )
        .size_full()
    }

    fn slider(&self, position: Ms, total: Ms, theme: &Theme, cx: &mut Context<Self>) -> Node {
        let player = self.player.clone();
        let step = move |delta, _: &mut Window, cx: &mut App| {
            player.update(cx, |player, cx| player.step(delta, cx));
        };
        let played = if total.0 == 0 {
            0.
        } else {
            position.0 as f32 / total.0 as f32
        };
        let waveform = self.player.read(cx).waveform().unwrap_or_default();
        let fade = elapsed(self.timeline.handover_at, motion::SLOW);
        let (accent, done, rest) = (theme.accent, theme.wave_done, theme.wave_rest);
        let unplayed = mix(done, rest, ease_standard(fade));
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
        .child(self.bars(
            waveform,
            None,
            move |center| {
                if center < played {
                    (accent, false)
                } else {
                    (unplayed, false)
                }
            },
            Some((played, theme.text)),
        ))
    }

    fn control(&self, control: Control, playing: bool, theme: &Theme) -> Node {
        let (ix, id, label, icon) = match control {
            Control::Back => (0, "player-back", "Back 5 seconds", Icon::TransportRotateCcw),
            Control::Play if playing => (1, "player-play", "Pause", Icon::TransportPause),
            Control::Play => (1, "player-play", "Play", Icon::TransportPlay),
            Control::Forward => (
                2,
                "player-forward",
                "Forward 5 seconds",
                Icon::TransportRotateCw,
            ),
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
        .cursor_pointer();
        if filled {
            button
                .min_w(px(40.))
                .min_h(px(40.))
                .px(px(4.))
                .bg(theme.accent)
                .child(icon.view(TRANSPORT, theme.on_accent))
        } else {
            // The seconds sit inside the arrow's circle and keep their size, like the arrow.
            let seconds = div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(8.))
                .line_height(px(8.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text)
                .child(format!("{}", SKIP_MS / 1000));
            button.min_w(px(32.)).min_h(px(32.)).px(px(4.)).child(
                div()
                    .relative()
                    .size(px(TRANSPORT))
                    .child(icon.view(TRANSPORT, theme.text))
                    .child(seconds),
            )
        }
    }

    fn player(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
            Some(elapsed) if elapsed < motion::FAST => {
                window.request_animation_frame();
                elapsed.as_secs_f32() / motion::FAST.as_secs_f32()
            }
            _ => 1.,
        };
        let transport = ease_standard(elapsed(self.timeline.handover_at, motion::BASE));
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
                .child(icons::beside(
                    Icon::TriangleAlert,
                    12.,
                    theme.attention,
                    theme.text(12.) * 1.4,
                    why,
                ))
        });
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(self.control(Control::Back, playing, &theme))
            .child(self.control(Control::Play, playing, &theme))
            .child(self.control(Control::Forward, playing, &theme));
        let wave = self.slider(position, total, &theme, cx);
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
                .into_any_element()
        } else {
            root.flex()
                .flex_col()
                .gap(px(8.))
                .pt(px(18.))
                .px(px(20.))
                .pb(px(16.))
                .rounded(px(14.))
                .child(wave.h(px(WAVE_H)))
                .child(
                    div()
                        .opacity(transport)
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            // No `min_w(0)`: the column keeps the time's width, so at large text
                            // it pushes the controls right instead of running under them.
                            div()
                                .flex_1()
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
                .into_any_element()
        }
    }

    fn fix_button(
        &self,
        id: &RecordingId,
        problem: &Problem,
        theme: &Theme,
        opacity: f32,
    ) -> Option<Node> {
        let fix = problem.fix()?;
        let (visible, spoken) = problem.button()?;
        let app = self.app.clone();
        let id = id.clone();
        let press = move |window: &mut Window, cx: &mut App| match &fix {
            Fix::Download(model) => {
                let model = model.clone();
                app.update(cx, |app, cx| app.download(model, cx));
            }
            Fix::ChooseSpeechModel => crate::settings_window::open_at(
                &app,
                crate::settings_window::Pane::Transcription,
                cx,
            ),
            Fix::FreeUpSpace => {
                crate::settings_window::open_at(&app, crate::settings_window::Pane::Storage, cx)
            }
            Fix::SetUpSummaries => crate::set_up_summaries::sheet(window, cx),
            Fix::TryAgain => app.update(cx, |app, cx| app.try_again(id.clone(), cx)),
            Fix::ShowInFinder => app.update(cx, |app, cx| app.show_in_finder(id.clone(), cx)),
        };
        Some(
            spoken_button(
                "card-fix",
                visible,
                spoken,
                true,
                &self.fix_focus,
                theme,
                press,
            )
            .min_h(px(30.))
            .px(px(14.))
            .py(px(4.))
            .flex_shrink(1.)
            .min_w(px(0.))
            .opacity(opacity),
        )
    }

    fn card(&mut self, phase: &Phase, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::get(cx);
        let id = self.player.read(cx).recording().clone();
        let (title, audio, waveform) = {
            let app = self.app.read(cx);
            let title = app
                .row(&id)
                .map(|row| library::title(row, &chrono::Local))
                .unwrap_or_default();
            (title, app.audio_of(&id), self.player.read(cx).waveform())
        };
        let still = cx.reduce_motion();
        let loops = motion::looping(window, cx);
        let at = loops.then(motion::clock_ms);
        let t = &self.timeline;
        let headline = phase.headline();
        let detail = phase.detail(audio);
        let sentence = phase.sentence(&title);
        let failed = matches!(phase, Phase::Failed(_));

        let text_phase = t
            .text_from
            .as_ref()
            .map_or(1.0, |(_, _, at)| elapsed(Some(*at), motion::FAST * 2));
        let (shown_headline, shown_detail, text_opacity) = match &t.text_from {
            Some((old_h, old_d, _)) if text_phase < 0.5 => (
                old_h.clone(),
                old_d.clone(),
                1.0 - ease_standard(text_phase * 2.0),
            ),
            _ if text_phase < 1.0 => (
                headline.clone(),
                detail.clone(),
                ease_standard(text_phase * 2.0 - 1.0),
            ),
            _ => (headline.clone(), detail.clone(), 1.0),
        };

        let headline_el = a11y::node(
            "card-headline",
            Role::Heading,
            headline.clone().unwrap_or_default(),
        )
        .aria_level(2)
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(theme.text(17.))
        .line_height(relative(1.3))
        .font_weight(FontWeight::SEMIBOLD)
        .when(failed, |h| {
            h.child(Icon::TriangleAlert.view(18., theme.attention))
        })
        .child(div().min_w_0().child(shown_headline.unwrap_or_default()));
        // While the old words fade out, VoiceOver already reads the new ones.
        let detail_el = shown_detail.map(|text| {
            a11y::node(
                "card-detail",
                Role::Label,
                detail.clone().unwrap_or_default(),
            )
            .text_size(theme.text(14.))
            .line_height(relative(1.45))
            .text_color(if failed {
                theme.text
            } else {
                theme.text_secondary
            })
            .child(text)
        });
        let status = a11y::node("card-status", Role::Status, sentence)
            .flex()
            .flex_col()
            .gap(px(2.))
            .flex_grow(1.)
            .flex_shrink(1.)
            .flex_basis(px(260.))
            .min_w(px(0.))
            .opacity(text_opacity)
            .child(headline_el)
            .children(detail_el);
        let move_to_top = phase.can_move_to_top().then(|| {
            let (app, id) = (self.app.clone(), id.clone());
            button(
                "card-move-to-top",
                "Move to Top",
                false,
                &self.top_focus,
                &theme,
                move |_, cx| app.update(cx, |app, cx| app.move_to_top(id.clone(), cx)),
            )
        });
        let headline_row = div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(12.))
            .child(status)
            .children(move_to_top);

        let chip = phase.light().map(|reason| {
            let (app, id) = (self.app.clone(), id.clone());
            let label = light_chip(reason);
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .opacity(ease_standard(elapsed(t.chip_at, motion::SLOW)))
                .child(
                    a11y::node("card-light", Role::Label, label)
                        .min_h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(8.))
                        .rounded(px(11.))
                        .border_1()
                        .border_color(theme.control_border)
                        .text_size(theme.text(12.))
                        .text_color(theme.text_secondary)
                        .child(Icon::BatteryMedium.view(12., theme.text_secondary))
                        .child(label),
                )
                .child(
                    a11y::focusable(
                        a11y::node("card-run-fast", Role::Button, "Use Fast This Time"),
                        &self.fast_focus,
                        &theme,
                        false,
                    )
                    .on_click(move |_, _, cx| {
                        app.update(cx, |app, cx| app.run_fast(id.clone(), cx))
                    })
                    .min_h(px(24.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(theme.text(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.accent_text)
                    .child("Use Fast This Time"),
                )
        });
        let fix = match phase {
            Phase::Failed(problem) => {
                let fade = (t.failed_at.map_or(1.0, |at| {
                    (at.elapsed().saturating_sub(motion::BASE)).as_secs_f32()
                        / motion::SLOW.as_secs_f32()
                }))
                .min(1.0);
                self.fix_button(&id, problem, &theme, ease_standard(fade))
            }
            _ => None,
        };

        let frontier = if still { t.head.to } else { t.head.now() };
        let settled = t.settled;
        let colored = ease_standard(elapsed(t.colored_at, motion::BASE));
        let outline_rest = theme.differentiate_without_color;
        let (done, rest) = (theme.wave_done, theme.wave_rest);
        let transcribing = matches!(phase, Phase::Transcribing { .. });
        let head_color = Hsla {
            a: at.filter(|_| transcribing).map_or(1.0, motion::breathe),
            ..theme.accent
        };
        let target = t.head.to;
        let has_head = transcribing || matches!(phase, Phase::Preparing);
        let arrive = t.arrived.filter(|_| waveform.is_some());
        let wave = a11y::node(
            "card-wave",
            Role::ProgressIndicator,
            "Transcription progress",
        )
        .aria_value(match audio {
            Some(audio) => format!(
                "{} of {}",
                a11y::spoken_duration(Ms((audio.0 as f64 * f64::from(frontier)) as u64)),
                a11y::spoken_duration(audio)
            ),
            None => String::new(),
        })
        .mt(px(18.))
        .h(px(WAVE_H))
        .child(self.bars(
            waveform.unwrap_or_default(),
            arrive,
            move |center| {
                if center < settled {
                    (done, false)
                } else if center < target {
                    (mix(rest, done, colored), false)
                } else {
                    (rest, outline_rest)
                }
            },
            has_head.then_some((frontier, head_color)),
        ));
        let caption_left = match phase {
            Phase::Transcribing { done, .. } => audio.map(|audio| {
                format!(
                    "Transcribed up to {}",
                    a11y::clock(Ms((audio.0 as f64 * f64::from(done.get())) as u64))
                )
            }),
            Phase::Summarizing { .. } => Some("Transcribed".to_owned()),
            _ => None,
        };
        let caption = div()
            .font_features(crate::theme::tabular())
            .mt(px(8.))
            .flex()
            .flex_wrap()
            .justify_between()
            .gap_x(px(12.))
            .text_size(theme.text(12.))
            .text_color(theme.text_secondary)
            .child(div().min_w_0().child(caption_left.unwrap_or_default()))
            .child(
                div()
                    .flex_none()
                    .child(audio.map(a11y::clock).unwrap_or_default()),
            );
        let steps = phase.steps().map(|steps| self.step_row(steps, &theme, at));
        let border = if failed {
            mix(
                theme.card_border,
                theme.attention_border,
                ease_standard(elapsed(t.failed_at, motion::BASE)),
            )
        } else {
            theme.card_border
        };
        let breathing = loops
            && (transcribing
                || phase
                    .steps()
                    .is_some_and(|s| s.iter().any(|s| s.glyph == StepGlyph::Dots)));
        let changing = !still && self.timeline.any_running();
        if (breathing || changing) && window.visibility().is_visible() {
            window.request_animation_frame();
        }
        a11y::node("processing-card", Role::Group, "Processing")
            .flex()
            .flex_col()
            .pt(px(18.))
            .px(px(20.))
            .pb(px(16.))
            .rounded(px(14.))
            .bg(theme.card_bg)
            .border_1()
            .border_color(border)
            .child(headline_row)
            .children(chip.map(|chip| chip.mt(px(10.))))
            .children(fix.map(|fix| div().mt(px(14.)).flex().child(fix)))
            .child(wave)
            .child(caption)
            .children(steps)
            .into_any_element()
    }

    fn step_row(&self, steps: [Step; 3], theme: &Theme, at: Option<u64>) -> impl IntoElement {
        let row = a11y::node("card-steps", Role::List, "Progress")
            .mt(px(14.))
            .pt(px(12.))
            .border_t_1()
            .border_color(theme.card_border)
            .flex()
            .flex_wrap()
            .gap_x(px(28.))
            .gap_y(px(8.));
        row.children(steps.into_iter().enumerate().map(|(ix, step)| {
            let color = match step.glyph {
                StepGlyph::Dots => theme.accent_text,
                StepGlyph::Warning => theme.attention,
                _ => theme.text_secondary,
            };
            let fade = self.timeline.steps[ix]
                .filter(|(glyph, _)| *glyph == step.glyph)
                .map_or(1.0, |(_, since)| {
                    ease_standard(elapsed(Some(since), motion::FAST))
                });
            a11y::node(
                format!("card-step-{ix}"),
                Role::ListItem,
                format!("{}, {}", step.name, step.word),
            )
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(theme.text(13.))
            .child(glyphs::step(step.glyph, color, 14., at).opacity(fade))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(step.name),
            )
            .child(div().text_color(theme.text_secondary).child(step.word))
        }))
    }
}

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.player.read(cx).recording().clone();
        let phase = self.app.read(cx).phase_of(&id);
        let has_levels = self.player.read(cx).waveform().is_some();
        if let Some(phase) = &phase {
            self.observe(phase, has_levels, window, cx);
        }
        match phase {
            Some(phase) if phase.is_processing() => self.card(&phase, window, cx),
            _ => self.player(window, cx),
        }
    }
}
