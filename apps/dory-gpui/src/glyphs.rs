//! Status glyphs for the sidebar row, the card's step row, onboarding's permission rows, and the
//! Summary tab: Lucide icons, and the drawn dots for work under way. Each sits next to a word that
//! carries the state (A11Y-8), so the glyph itself is not announced. Glyphs grow with the text
//! above 100% ([`scaled`]), so they keep their weight beside words that doubled.
//!
//! The dots glyph replaces the spinning ring: rotation is outside the allowed motion, so its three
//! dots breathe in opacity instead. `at` is [`crate::motion::clock_ms`] while it may animate and
//! `None` for the Reduce Motion stand-in, three dots at full opacity.

use gpui_kit::{Div, Hsla, IntoElement as _, ParentElement as _, Styled as _, div};

use crate::icons::{Icon, scaled};
use crate::library::Glyph;
use crate::motion;
use crate::processing::StepGlyph;

fn frame(size: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(scaled(size))
        .h(scaled(size))
}

fn icon(icon: Icon, color: Hsla, size: f32) -> Div {
    frame(size).child(icon.view(size, color))
}

/// Three dots that breathe one after another.
pub fn dots(color: Hsla, size: f32, at: Option<u64>) -> Div {
    let dot = size * 0.22;
    frame(size)
        .gap(scaled((size * 0.1).max(1.)))
        .children((0..3).map(|i| {
            div()
                .flex_none()
                .w(scaled(dot))
                .h(scaled(dot))
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
                .w(scaled(size * 0.6))
                .h(scaled(size * 0.6))
                .rounded_full()
                .bg(color),
        ),
        Glyph::Doc => icon(Icon::FileText, color, size),
        Glyph::StopCircle => icon(Icon::CircleStop, color, size),
        Glyph::DotsCircle => frame(size)
            .relative()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Icon::Circle.view(size, color)),
            )
            .child(dots(color, size * 0.6, at)),
        Glyph::Clock => icon(Icon::Clock, color, size),
        Glyph::Dots => dots(color, size, at),
        Glyph::Check => icon(Icon::CircleCheck, color, size),
        Glyph::Triangle => icon(Icon::TriangleAlert, color, size),
    }
}

/// A step's glyph in the card's step row, and a permission's status in onboarding.
pub fn step(glyph: StepGlyph, color: Hsla, size: f32, at: Option<u64>) -> Div {
    match glyph {
        StepGlyph::Stop => icon(Icon::Square, color, size),
        StepGlyph::Dots => dots(color, size, at),
        StepGlyph::Check => icon(Icon::CircleCheck, color, size),
        StepGlyph::Clock => icon(Icon::Clock, color, size),
        StepGlyph::Pending => icon(Icon::CircleDashed, color, size),
        StepGlyph::Warning => icon(Icon::TriangleAlert, color, size),
    }
}
