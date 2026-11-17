//! A matched word drawn the one way everywhere a search marks text (UX-32, A11Y-8): the sidebar's
//! titles and snippets, the detail pane's title, and the transcript's turns.

use std::ops::Range;

use gpui_kit::{
    AnyElement, FontWeight, HighlightStyle, IntoElement as _, StyledText, UnderlineStyle, px,
};

use crate::theme::Theme;

/// `text` with each of `marks` (bytes of `text`) semibold in `weight` on the `search_match` fill.
/// On the accent-filled row there is no fill to read against, so a mark is underlined.
pub fn marked(
    text: String,
    marks: &[Range<usize>],
    weight: FontWeight,
    on_accent_fill: bool,
    theme: &Theme,
) -> AnyElement {
    if marks.is_empty() {
        return text.into_any_element();
    }
    let style = if on_accent_fill {
        HighlightStyle {
            color: Some(theme.on_accent),
            font_weight: Some(weight),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme.on_accent),
                wavy: false,
            }),
            ..HighlightStyle::default()
        }
    } else {
        HighlightStyle {
            color: Some(theme.text),
            font_weight: Some(weight),
            background_color: Some(theme.search_match),
            ..HighlightStyle::default()
        }
    };
    StyledText::new(text)
        .with_highlights(marks.iter().map(|mark| (mark.clone(), style)))
        .into_any_element()
}
