//! The toolbar record control (UX-7, UX-8, UX-10) and the Undo toast (UX-9).
//!
//! Idle, it is the bordered "Record" pill. While a take is live it is the red pill with a white
//! dot, the elapsed time, and a 5-bar level meter, then round Pause and Stop buttons. With 5
//! minutes left the pill turns `attention` and reads "N min left". The pill repaints from
//! [`Live`], which `AppState` updates on every `Levels` event, so the meter never repaints the
//! window.
//!
//! Every button calls an `AppState` recorder method, the same one the menu items and shortcuts
//! reach through `crate::actions`.

use dory_core::app::RecorderStatus;
use dory_core::domain::Ms;
use gpui_kit::{
    Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, Render, Role,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::app::AppState;
use crate::icons::Icon;
use crate::theme::Theme;

/// What the pill shows between recorder phase changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub recorded: Ms,
    /// The loudest channel's latest peak, `0.0..=1.0`.
    pub level: f32,
    /// Recorded time at which the cap falls. Known once the core warns (UX-10).
    pub cap_at: Option<Ms>,
    /// Whether this take's first `Levels` arrived, for the `first-sample` log line.
    pub sampled: bool,
}

impl Live {
    pub fn left(&self) -> Option<Ms> {
        self.cap_at
            .map(|cap| Ms(cap.0.saturating_sub(self.recorded.0)))
    }
}

/// The pill's visible text: the elapsed time, "Paused, 12:04", or "4 min left".
pub fn pill_text(status: &RecorderStatus, live: &Live) -> String {
    let time = a11y::clock(live.recorded);
    match (status, live.left()) {
        (RecorderStatus::Paused { .. }, _) => format!("Paused, {time}"),
        (_, Some(left)) => format!("{} min left", left.0.div_ceil(60_000)),
        _ => time,
    }
}

/// The pill's VoiceOver label: "Recording, 12 minutes 4 seconds".
pub fn pill_label(status: &RecorderStatus, live: &Live) -> String {
    let phase = match status {
        RecorderStatus::Paused { .. } => "Paused",
        _ => "Recording",
    };
    let mut label = format!("{phase}, {}", a11y::spoken_duration(live.recorded));
    if let Some(left) = live.left() {
        label.push_str(&format!(", {} left", a11y::spoken_duration(left)));
    }
    label
}

/// Lit bars of 5 for a peak, on a 50 dB scale so speech moves the meter.
pub fn lit_bars(level: f32) -> usize {
    if level <= 0.0 {
        return 0;
    }
    let db = 20.0 * level.min(1.0).log10();
    (((db + 50.0) / 10.0).ceil().clamp(0.0, 5.0)) as usize
}

pub struct RecordControl {
    app: Entity<AppState>,
    record_focus: FocusHandle,
    pause_focus: FocusHandle,
    stop_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RecordControl {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let live = app.read(cx).live().clone();
        let subscriptions = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.observe(&live, |_, _, cx| cx.notify()),
        ];
        Self {
            app,
            record_focus: cx.focus_handle(),
            pause_focus: cx.focus_handle(),
            stop_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Where F6 lands in the toolbar: Record while idle, else Pause.
    pub fn first_focus(&self, cx: &gpui_kit::App) -> FocusHandle {
        match self.app.read(cx).recorder() {
            RecorderStatus::Recording { .. } | RecorderStatus::Paused { .. } => {
                self.pause_focus.clone()
            }
            _ => self.record_focus.clone(),
        }
    }

    fn record_button(&self, theme: &Theme) -> a11y::Node {
        let app = self.app.clone();
        a11y::focusable(
            a11y::node("record", Role::Button, "Start recording"),
            &self.record_focus,
            theme,
            false,
        )
        .aria_keyshortcuts("Command-Shift-R")
        .on_click(move |_, _, cx| app.update(cx, |app, cx| app.start_recording(cx)))
        .min_h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(14.))
        .bg(theme.control_bg)
        .when(theme.shadows, |b| b.shadow_xs())
        .when(theme.high_contrast, |b| {
            b.border_color(theme.control_border)
        })
        .text_size(theme.text(13.))
        .font_weight(FontWeight::MEDIUM)
        .child(div().size(px(10.)).rounded_full().bg(theme.record_red))
        .child("Record")
    }

    fn round_button(&self, round: Round, focus: &FocusHandle, theme: &Theme) -> a11y::Node {
        let app = self.app.clone();
        let act = round.act;
        a11y::focusable(
            a11y::node(round.id, Role::Button, round.label),
            focus,
            theme,
            false,
        )
        .aria_keyshortcuts(round.keys)
        .on_click(move |_, _, cx| app.update(cx, act))
        .min_w(px(28.))
        .min_h(px(28.))
        .px(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.control_bg)
        .border_color(theme.control_border)
        .child(round.icon.view(14., theme.text))
    }
}

struct Round {
    id: &'static str,
    label: &'static str,
    keys: &'static str,
    icon: Icon,
    act: fn(&mut AppState, &mut Context<AppState>),
}

impl Render for RecordControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let app = self.app.read(cx);
        let status = app.recorder().clone();
        let live = app.live().read(cx).clone();
        if !matches!(
            status,
            RecorderStatus::Recording { .. } | RecorderStatus::Paused { .. }
        ) {
            return div().child(self.record_button(&theme));
        }
        let paused = matches!(status, RecorderStatus::Paused { .. });
        let (fill, on_fill) = if live.cap_at.is_some() {
            (theme.attention, theme.on_attention)
        } else {
            (theme.record_red, theme.on_record)
        };
        let off = if live.cap_at.is_some() {
            crate::theme::blend(on_fill, fill, 0.4)
        } else {
            theme.meter_off
        };
        let lit = if paused || cx.reduce_motion() {
            0
        } else {
            lit_bars(live.level)
        };
        let meter = div()
            .flex()
            .items_end()
            .gap(px(2.))
            .h(px(12.))
            .children((0..5).map(|bar| {
                div()
                    .w(px(3.))
                    .h(px(4. + 2. * bar as f32))
                    .rounded(px(1.))
                    .bg(if bar < lit { on_fill } else { off })
            }));
        let text = pill_text(&status, &live);
        let text = if theme.differentiate_without_color && !paused {
            format!("Recording, {text}")
        } else {
            text
        };
        let pill = a11y::node("record-pill", Role::Status, pill_label(&status, &live))
            .min_h(px(28.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(14.))
            .bg(fill)
            .text_color(on_fill)
            .text_size(theme.text(13.))
            .font_weight(FontWeight::MEDIUM)
            .child(div().size(px(8.)).rounded_full().bg(on_fill))
            .child(text)
            .child(meter);
        let (pause_label, pause_icon) = if paused {
            ("Resume recording", Icon::Play)
        } else {
            ("Pause recording", Icon::Pause)
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(pill)
            .child(self.round_button(
                Round {
                    id: "record-pause",
                    label: pause_label,
                    keys: "Command-Shift-P",
                    icon: pause_icon,
                    act: AppState::toggle_pause,
                },
                &self.pause_focus.clone(),
                &theme,
            ))
            .child(self.round_button(
                Round {
                    id: "record-stop",
                    label: "Stop recording",
                    keys: "Command-Period",
                    icon: Icon::Square,
                    act: AppState::stop_recording,
                },
                &self.stop_focus.clone(),
                &theme,
            ))
    }
}

/// The Undo toast while the recorder is in grace (UX-9): "Recording saved", Undo, and the
/// seconds left. `None` outside grace.
pub fn undo_toast(
    app: &Entity<AppState>,
    focus: &FocusHandle,
    theme: &Theme,
    cx: &gpui_kit::App,
) -> Option<a11y::Node> {
    let seconds = app.read(cx).grace_left()?;
    let target = app.clone();
    let label = format!("Recording saved. Undo, {seconds} seconds left");
    Some(
        a11y::node("undo-stop-toast", Role::Status, label)
            .flex()
            .items_center()
            .gap(px(12.))
            .min_h(px(36.))
            .px(px(14.))
            .py(px(4.))
            .rounded(px(10.))
            .bg(theme.toast_bg)
            .text_color(theme.toast_text)
            .text_size(theme.text(13.))
            .when(theme.shadows, |toast| toast.shadow_lg())
            .when(theme.high_contrast, |toast| {
                toast.border_1().border_color(theme.toast_text)
            })
            .child("Recording saved")
            .child(
                a11y::focusable(
                    a11y::node("undo-stop", Role::Button, "Undo stop"),
                    focus,
                    theme,
                    false,
                )
                .on_click(move |_, _, cx| target.update(cx, |app, cx| app.undo_stop(cx)))
                .min_h(px(28.))
                .min_w(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .px(px(8.))
                .rounded(px(6.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Undo"),
            )
            .child(
                div()
                    .text_color(theme.toast_text_secondary)
                    .child(format!("{seconds} s")),
            ),
    )
}

#[cfg(test)]
mod tests {
    use dory_core::domain::RecordingId;

    use super::*;

    #[test]
    fn the_pill_reads_the_time_the_pause_and_the_time_left() {
        let id = RecordingId::parse("rec-1").unwrap();
        let recording = RecorderStatus::Recording {
            id: id.clone(),
            recorded: Ms(0),
        };
        let paused = RecorderStatus::Paused {
            id,
            recorded: Ms(0),
        };
        let mut live = Live {
            recorded: Ms(724_000),
            ..Live::default()
        };
        assert_eq!(pill_text(&recording, &live), "12:04");
        assert_eq!(
            pill_label(&recording, &live),
            "Recording, 12 minutes 4 seconds"
        );
        assert_eq!(pill_text(&paused, &live), "Paused, 12:04");
        live.cap_at = Some(Ms(724_000 + 4 * 60_000 + 30_000));
        assert_eq!(pill_text(&recording, &live), "5 min left");
        assert_eq!(
            pill_label(&recording, &live),
            "Recording, 12 minutes 4 seconds, 4 minutes 30 seconds left"
        );
    }

    #[test]
    fn voiceover_hears_start_pause_resume_and_save() {
        let id = RecordingId::parse("rec-1").unwrap();
        let at = |phase: fn(RecordingId, Ms) -> RecorderStatus| phase(id.clone(), Ms(0));
        let recording = at(|id, recorded| RecorderStatus::Recording { id, recorded });
        let paused = at(|id, recorded| RecorderStatus::Paused { id, recorded });
        let grace = at(|id, recorded| RecorderStatus::Grace { id, recorded });
        let idle = RecorderStatus::Idle;
        let said = |from, to| a11y::Announcement::of_recorder(from, to).map(|a| a.text());
        assert_eq!(
            said(&idle, &recording).as_deref(),
            Some("Recording started")
        );
        assert_eq!(
            said(&recording, &paused).as_deref(),
            Some("Recording paused")
        );
        assert_eq!(
            said(&paused, &recording).as_deref(),
            Some("Recording resumed")
        );
        assert_eq!(said(&recording, &grace).as_deref(), Some("Recording saved"));
        assert_eq!(
            said(&grace, &recording).as_deref(),
            Some("Recording resumed")
        );
        assert_eq!(said(&grace, &idle), None);
    }

    #[test]
    fn the_meter_lights_more_bars_for_louder_audio() {
        assert_eq!(lit_bars(0.0), 0);
        assert_eq!(lit_bars(0.001), 0);
        assert_eq!(lit_bars(0.01), 1);
        assert_eq!(lit_bars(0.1), 3);
        assert_eq!(lit_bars(1.0), 5);
    }
}
