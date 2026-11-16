//! Accessible building blocks GPUI Kit 0.9.1 lacks, ported from the P0 spike
//! (`chore/gpui-a11y-spike`, `spikes/gpui-a11y/RESULT.md`, findings 1 to 4), plus the spoken
//! forms of times. Every element the app builds takes an element id, an AX identifier, a role,
//! and a label; [`node`] sets all four at once so none is forgotten.
//!
//! Kit gaps these close:
//! - [`tab`]: Kit's `Tab` has no focus handle, so Tab skipped the tabs (finding 1).
//! - [`slider`]: Kit's `Slider` has no keys and no value text (finding 2, A11Y-3).
//! - [`markdown_blocks`] with [`with_markdown_children`]: Kit's `TextView` emits no AX nodes
//!   (finding 3).
//! - [`focusable`]: GPUI draws no focus ring, and Kit's own is a faint halo (lane 4).

use dory_core::domain::Ms;
use gpui_kit::{
    AccessibleAction, App, Div, ElementId, FocusHandle, InteractiveElement as _, KeyDownEvent,
    ParentElement as _, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Window, div, prelude::FluentBuilder as _, px, transparent_black,
};

use crate::theme::Theme;

/// A div that Kit's `gpui_kit::test` snapshots can find in tests, and plain `Stateful<Div>`
/// otherwise.
pub type Node = gpui_kit::base::ObservedElement<Stateful<Div>>;

/// A div with its element id, a matching AX identifier, a role, and a label.
pub fn node(id: impl Into<SharedString>, role: Role, label: impl Into<SharedString>) -> Node {
    let id: SharedString = id.into();
    div()
        .id(ElementId::Name(id.clone()))
        .test_support()
        .accessibility_id(id)
        .role(role)
        .aria_label(label)
}

/// Makes `el` a Tab stop with the direction C ring: 2 pt, `accent`, following the corner radius.
/// `on_accent_fill` swaps the ring to `on_accent` on an accent-filled element.
pub fn focusable(el: Node, focus: &FocusHandle, theme: &Theme, on_accent_fill: bool) -> Node {
    let ring = theme.focus_ring(on_accent_fill);
    el.track_focus(&focus.clone().tab_stop(true))
        .border_2()
        .border_color(transparent_black())
        .focus_visible(move |style| style.border_color(ring))
}

/// An underline tab (UX-2) that takes focus and activates with a click, Return, or Space (GPUI
/// turns either key on a focused element with a click handler into a click). The
/// selected tab is semibold over a 2 pt `accent` underline, a shape as well as a color, and at
/// least 28 pt tall however small the text.
pub fn tab(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    focus: &FocusHandle,
    theme: &Theme,
    on_select: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> Node {
    let label: SharedString = label.into();
    focusable(node(id, Role::Tab, label.clone()), focus, theme, false)
        .aria_selected(selected)
        .on_click(move |_, window, cx| on_select(window, cx))
        .min_h(px(28.))
        .min_w(px(24.))
        .px(px(8.))
        .flex()
        .flex_col()
        .justify_center()
        .rounded(px(6.))
        .text_size(theme.text(14.))
        .when(selected, |tab| {
            tab.font_weight(gpui_kit::FontWeight::SEMIBOLD)
        })
        .child(div().pt(px(4.)).child(label))
        .child(
            div()
                .mt(px(4.))
                .h(px(2.))
                .w_full()
                .rounded(px(1.))
                .when(selected, |line| line.bg(theme.accent)),
        )
}

/// A11Y-3: the player slider. Arrows move 5 s, Page Up and Page Down 1 min, and VoiceOver's
/// increment and decrement do the same as the arrows. `on_step` gets a signed delta in ms.
pub fn slider(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    position: Ms,
    total: Ms,
    focus: &FocusHandle,
    theme: &Theme,
    on_step: impl Fn(i64, &mut Window, &mut App) + Clone + 'static,
) -> Node {
    const STEP: i64 = 5_000;
    const PAGE: i64 = 60_000;
    let (inc, dec, key) = (on_step.clone(), on_step.clone(), on_step);
    focusable(node(id, Role::Slider, label), focus, theme, false)
        .aria_value(spoken_position(position, total))
        .aria_numeric_value(position.0 as f64 / 1000.0)
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(total.0 as f64 / 1000.0)
        .aria_numeric_value_step((STEP / 1000) as f64)
        .aria_orientation(gpui_kit::accesskit::Orientation::Horizontal)
        .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
            inc(STEP, window, cx)
        })
        .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
            dec(-STEP, window, cx)
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            let delta = match event.keystroke.key.as_str() {
                "right" | "up" => STEP,
                "left" | "down" => -STEP,
                "pageup" => PAGE,
                "pagedown" => -PAGE,
                _ => return,
            };
            key(delta, window, cx);
            cx.stop_propagation();
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub role: Role,
    pub level: Option<usize>,
    pub text: String,
}

/// The headings, list items, and paragraphs of a Markdown summary, as VoiceOver should read
/// them. The spike's line parser: enough for the three-section summaries the pipeline writes.
/// The summary view may replace it with a walk of `TextView`'s own AST, keeping this signature.
pub fn markdown_blocks(markdown: &str) -> Vec<Block> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let heading = line.chars().take_while(|c| *c == '#').count();
            let numbered = line
                .split_once(". ")
                .filter(|(n, _)| n.parse::<u32>().is_ok());
            let (role, level, text) = if heading > 0 {
                (Role::Heading, Some(heading), line[heading..].trim())
            } else if let Some(rest) = line.strip_prefix("- ").or(line.strip_prefix("* ")) {
                (Role::ListItem, None, rest)
            } else if let Some((_, rest)) = numbered {
                (Role::ListItem, None, rest)
            } else {
                (Role::Paragraph, None, line)
            };
            Block {
                role,
                level,
                text: text.replace("**", ""),
            }
        })
        .collect()
}

/// Adds one AX node per Markdown block under `el`, which should be a `Role::Document` node.
pub fn with_markdown_children(el: Node, markdown: &str) -> Node {
    let blocks = markdown_blocks(markdown);
    el.a11y_synthetic_children(move |builder| {
        for (ix, block) in blocks.into_iter().enumerate() {
            let mut node = gpui_kit::accesskit::Node::new(block.role);
            node.set_label(block.text);
            if let Some(level) = block.level {
                node.set_level(level);
            }
            let id = builder.synthetic_node_id(ix);
            builder.push_child(id, node);
        }
    })
}

/// Asks VoiceOver to speak `message` now (A11Y-4). A no-op where the platform has no bridge.
/// The recorder and the pipeline speak through [`Announcement`] instead, so their words live in
/// one table.
pub fn announce(message: &str) {
    crate::platform::announce(message);
}

/// Every state change A11Y-4 names, and the few more the recorder makes. One table, so the
/// words VoiceOver says and the tests that check them never drift apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Announcement {
    RecordingStarted,
    RecordingPaused,
    RecordingResumed,
    /// Stop saves at once (UX-9), so VoiceOver says what happened to the audio.
    RecordingSaved,
    TimeLeft(Ms),
    CapReached,
    Finished {
        title: String,
    },
    Failed {
        title: String,
    },
}

impl Announcement {
    pub fn text(&self) -> String {
        match self {
            Announcement::RecordingStarted => "Recording started".into(),
            Announcement::RecordingPaused => "Recording paused".into(),
            Announcement::RecordingResumed => "Recording resumed".into(),
            Announcement::RecordingSaved => "Recording saved".into(),
            Announcement::TimeLeft(left) => {
                format!("{} of recording left", spoken_duration(*left))
            }
            Announcement::CapReached => "The recording reached its time limit and was saved".into(),
            Announcement::Finished { title } => format!("Finished processing {title}"),
            Announcement::Failed { title } => format!("{title} needs attention"),
        }
    }

    pub fn post(&self) {
        announce(&self.text());
    }

    /// What VoiceOver says when the recorder moves from `from` to `to`, if anything. The cap and
    /// a failed start say their own words from the events that carry the reason.
    pub fn of_recorder(
        from: &dory_core::app::RecorderStatus,
        to: &dory_core::app::RecorderStatus,
    ) -> Option<Announcement> {
        use dory_core::app::RecorderStatus::*;
        match (from, to) {
            (Idle, Recording { .. }) => Some(Announcement::RecordingStarted),
            (Recording { .. }, Paused { .. }) => Some(Announcement::RecordingPaused),
            (Paused { .. } | Grace { .. }, Recording { .. }) => {
                Some(Announcement::RecordingResumed)
            }
            (Recording { .. } | Paused { .. }, Grace { .. }) => Some(Announcement::RecordingSaved),
            _ => None,
        }
    }
}

/// "12:30", or "1:02:03" from an hour on. Rounds down, so it never runs ahead of the audio.
pub fn clock(at: Ms) -> String {
    let secs = at.0 / 1000;
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "12 minutes 30 seconds", "1 hour 2 minutes", "45 seconds".
pub fn spoken_duration(at: Ms) -> String {
    let secs = at.0 / 1000;
    let unit = |n: u64, word: &str| {
        if n == 1 {
            format!("1 {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    let parts: Vec<String> = [
        (secs / 3600, "hour"),
        (secs % 3600 / 60, "minute"),
        (secs % 60, "second"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, word)| unit(n, word))
    .collect();
    if parts.is_empty() {
        unit(0, "second")
    } else {
        parts.join(" ")
    }
}

/// A11Y-3's slider value: "12 minutes 30 seconds of 48 minutes".
pub fn spoken_position(position: Ms, total: Ms) -> String {
    format!(
        "{} of {}",
        spoken_duration(position),
        spoken_duration(total)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_times_read_as_a11y_3_writes_them() {
        assert_eq!(
            spoken_position(Ms(750_000), Ms(48 * 60_000)),
            "12 minutes 30 seconds of 48 minutes"
        );
        assert_eq!(spoken_duration(Ms(0)), "0 seconds");
        assert_eq!(spoken_duration(Ms(61_000)), "1 minute 1 second");
        assert_eq!(spoken_duration(Ms(3_720_000)), "1 hour 2 minutes");
        assert_eq!(clock(Ms(744_900)), "12:24");
        assert_eq!(clock(Ms(3_723_000)), "1:02:03");
    }

    #[test]
    fn markdown_reads_as_headings_and_list_items() {
        let blocks =
            markdown_blocks("## Key points\n\n- Ship the **native** window.\n1. Book it.\nPlain.");
        let shape: Vec<(Role, Option<usize>, &str)> = blocks
            .iter()
            .map(|b| (b.role, b.level, b.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                (Role::Heading, Some(2), "Key points"),
                (Role::ListItem, None, "Ship the native window."),
                (Role::ListItem, None, "Book it."),
                (Role::Paragraph, None, "Plain."),
            ]
        );
    }
}
