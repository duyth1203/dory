//! The Transcript tab (UX-3, A11Y-10). Stub for agent 2, who owns this file together with
//! `player_view.rs`.
//!
//! Agent 2 builds, without editing any other file:
//! - One list item per `Detail.turns` entry: a 30 pt initial circle, the label (14 semibold),
//!   the timestamp as a button (12, `accent_text`, AX label "Play from 12:24"), and the text
//!   (15/1.6). VoiceOver reads a turn as "Sam, 12 minutes 30 seconds, then the text".
//! - Click-to-seek: the timestamp, and Return on a focused turn, call
//!   `session.read(cx).player().update(cx, |p, cx| p.play_from(start, cx))`. Turns with no range
//!   (legacy transcripts) show no timestamp and do not seek.
//! - The turn under the playhead gets `playing_bg`, a speaker glyph, and the word "Playing".
//!   Observe the session's `PlayerHandle` for the position.
//! - Speaker rename: the label becomes a 150 pt field with the hint "Renames Speaker 3 in all 14
//!   turns. Return to save, Escape to cancel." Save with `Session::rename_speaker`. Do not patch
//!   the turns: the session reloads and the new labels arrive in the next `Detail`.
//! - Copy Transcript and Export sit in the tab row, which the detail pane draws.
//!
//! The detail pane creates this view once per session and places it under the Transcript tab.

use gpui_kit::{
    Context, Entity, IntoElement, ParentElement as _, Render, Role, Styled as _, Window, div, px,
};

use crate::a11y;
use crate::app::Session;
use crate::theme::Theme;

pub struct TranscriptView {
    session: Entity<Session>,
}

impl TranscriptView {
    pub fn new(session: Entity<Session>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        Self { session }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let turns = self
            .session
            .read(cx)
            .detail()
            .map(|d| d.turns.clone())
            .unwrap_or_default();
        a11y::node("transcript", Role::List, "Transcript")
            .flex()
            .flex_col()
            .gap(px(16.))
            .text_size(theme.text(15.))
            .children(turns.into_iter().enumerate().map(|(ix, turn)| {
                a11y::node(
                    format!("turn-{ix}"),
                    Role::ListItem,
                    format!("{}, {}", turn.label, turn.text),
                )
                .child(
                    div()
                        .text_size(theme.text(14.))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(turn.label.clone()),
                )
                .child(div().child(turn.text))
            }))
    }
}
