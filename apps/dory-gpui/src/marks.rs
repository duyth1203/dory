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
    let style = style(weight, on_accent_fill, theme);
    StyledText::new(text)
        .with_highlights(marks.iter().map(|mark| (mark.clone(), style)))
        .into_any_element()
}

/// How one mark is drawn. Under Increase Contrast a mark on the paper also gets a 2 pt `text`
/// underline: `HighlightStyle` has no border, and an underline is the nearest line that stays
/// with the word through wraps. With the stronger fill and the weight, the mark no longer rests
/// on a pale tint (`docs/plans/search.md` S4 lane 9).
pub fn style(weight: FontWeight, on_accent_fill: bool, theme: &Theme) -> HighlightStyle {
    let underline = |thickness: f32, color| UnderlineStyle {
        thickness: px(thickness),
        color: Some(color),
        wavy: false,
    };
    if on_accent_fill {
        HighlightStyle {
            color: Some(theme.on_accent),
            font_weight: Some(weight),
            underline: Some(underline(1., theme.on_accent)),
            ..HighlightStyle::default()
        }
    } else {
        HighlightStyle {
            color: Some(theme.text),
            font_weight: Some(weight),
            background_color: Some(theme.search_match),
            underline: theme.high_contrast.then(|| underline(2., theme.text)),
            ..HighlightStyle::default()
        }
    }
}
