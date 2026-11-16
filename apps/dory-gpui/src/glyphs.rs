//! Status glyphs for the sidebar row, the card's step row, and the Summary tab. Each sits next to
//! a word that carries the state (A11Y-8), so the glyph itself is not announced. Glyphs keep their
//! size at every text scale (`docs/design/processing-view.md` section 2).
//!
//! The dots glyph replaces the spinning ring: rotation is outside the allowed motion, so its three
//! dots breathe in opacity instead. `at` is [`crate::motion::clock_ms`] while it may animate and
//! `None` for the Reduce Motion stand-in, three dots at full opacity.

use gpui_kit::{Div, Hsla, IntoElement as _, ParentElement as _, Styled as _, div, px};

use crate::library::Glyph;
use crate::motion;
use crate::processing::StepGlyph;

fn frame(size: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(size))
        .h(px(size))
}

fn text(symbol: &'static str, color: Hsla, size: f32) -> Div {
    frame(size).child(
        div()
            .text_size(px(size))
            .line_height(px(size))
            .text_color(color)
            .child(symbol),
    )
}

fn ring(color: Hsla, size: f32) -> Div {
    frame(size)
        .rounded_full()
        .border(px(1.5))
        .border_color(color)
}

/// Three dots that breathe one after another.
pub fn dots(color: Hsla, size: f32, at: Option<u64>) -> Div {
    let dot = size * 0.22;
    frame(size)
        .gap(px((size * 0.1).max(1.)))
        .children((0..3).map(|i| {
            div()
                .flex_none()
                .w(px(dot))
                .h(px(dot))
                .rounded_full()
                .bg(color)
                .opacity(at.map_or(1.0, |at| motion::dot(i, at)))
                .into_any_element()
        }))
}

/// A sidebar row's glyph.
pub fn row(glyph: Glyph, color: Hsla, size: f32, at: Option<u64>) -> Div {
    match glyph {
        Glyph::Dot => frame(size).child(
            div()
                .w(px(size * 0.6))
                .h(px(size * 0.6))
                .rounded_full()
                .bg(color),
        ),
        Glyph::Doc => text("▤", color, size),
        Glyph::StopCircle => ring(color, size).child(
            div()
                .w(px(size * 0.34))
                .h(px(size * 0.34))
                .rounded(px(1.))
                .bg(color),
        ),
        Glyph::DotsCircle => ring(color, size).child(dots(color, size * 0.7, at)),
        Glyph::Clock => text("◷", color, size),
        Glyph::Dots => dots(color, size, at),
        Glyph::Check => ring(color, size).child(
            div()
                .text_size(px(size * 0.7))
                .line_height(px(size * 0.7))
                .text_color(color)
                .child("✓"),
        ),
        Glyph::Triangle => text("⚠︎", color, size),
    }
}

/// A step's glyph in the card's step row.
pub fn step(glyph: StepGlyph, color: Hsla, size: f32, at: Option<u64>) -> Div {
    match glyph {
        StepGlyph::Stop => frame(size).child(
            div()
                .w(px(size * 0.6))
                .h(px(size * 0.6))
                .rounded(px(2.))
                .bg(color),
        ),
        StepGlyph::Dots => dots(color, size, at),
        StepGlyph::Check => text("✓", color, size),
        StepGlyph::Clock => text("◷", color, size),
        StepGlyph::Pending => text("◌", color, size),
        StepGlyph::Warning => text("⚠︎", color, size),
    }
}
