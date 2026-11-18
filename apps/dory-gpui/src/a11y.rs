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

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use dory_core::domain::{LangTag, Ms};
use gpui_kit::base::ScrollbarHandle;
use gpui_kit::component::input::InputState;
use gpui_kit::{
    AccessibleAction, AnyElement, App, DispatchPhase, Div, ElementId, Entity, FocusHandle,
    Focusable as _, Global, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent,
    ListState, ParentElement as _, Pixels, Point, RenderOnce, Role, ScrollEvent, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _,
    TouchPhase, ViewElement, WeakFocusHandle, Window, canvas, div, prelude::FluentBuilder as _, px,
    transparent_black,
};

use crate::theme::Theme;

/// A div that Kit's `gpui_kit::test` snapshots can find in tests, and plain `Stateful<Div>`
/// otherwise.
pub type Node = gpui_kit::base::ObservedElement<Stateful<Div>>;

/// A div with its element id, a matching AX identifier, a role, and a label.
/// The text fields someone may be typing in. GPUI cannot say whether a text field has focus, so
/// each field registers here when it is made, and code that moves focus on its own asks
/// [`typing`] first: a failure that arrives mid-rename must not pull focus out of the field.
#[derive(Default)]
struct TextFields(Vec<WeakFocusHandle>);

impl Global for TextFields {}

pub fn register_text_field(field: &Entity<InputState>, cx: &mut App) {
    let handle = field.read(cx).focus_handle(cx).downgrade();
    let fields = cx.default_global::<TextFields>();
    fields.0.retain(|h| h.upgrade().is_some());
    fields.0.push(handle);
}

/// True while focus is in a registered text field.
pub fn typing(window: &Window, cx: &App) -> bool {
    cx.try_global::<TextFields>().is_some_and(|fields| {
        fields
            .0
            .iter()
            .filter_map(WeakFocusHandle::upgrade)
            .any(|h| h.contains_focused(window, cx))
    })
}

/// UX-31: a scroll area inside the page that passes leftover scrolling to the page. The returned
/// div is the scroll area itself, and `child` is its content: give the div its height or
/// `max_h`, and do not make `child` scroll on its own.
///
/// GPUI hands each wheel event to every scroll area under the pointer, innermost first, so
/// without this the page and the inner area move together. Here the inner area takes the event
/// while it can move that way and leaves it to the page at its edge. A trackpad gesture stays
/// with the area it began in until the next gesture begins, momentum included, as in AppKit.
pub fn nested_scroll(id: impl Into<SharedString>, child: impl IntoElement) -> Stateful<Div> {
    nested(id.into(), None, child.into_any_element())
}

/// [`nested_scroll`] driven by the caller's `handle`, for a caller that scrolls the area itself,
/// such as a list that keeps its highlighted row in view. A `track_scroll` on the returned div
/// would track the outer wrapper, which never scrolls.
pub fn nested_scroll_tracked(
    id: impl Into<SharedString>,
    handle: &ScrollHandle,
    child: impl IntoElement,
) -> Stateful<Div> {
    nested(id.into(), Some(handle.clone()), child.into_any_element())
}

fn nested(key: SharedString, handle: Option<ScrollHandle>, content: AnyElement) -> Stateful<Div> {
    div()
        .id(ElementId::Name(key.clone()))
        .flex()
        .flex_col()
        .child(NestedArea {
            key,
            handle,
            content,
        })
}

/// [`nested_scroll`] for a virtualized GPUI `list`, which scrolls itself through `state`.
pub fn nested_list(
    id: impl Into<SharedString>,
    state: &ListState,
    list: impl IntoElement,
) -> Stateful<Div> {
    chain(id.into(), state.clone(), list)
}

/// How far a scroll area has scrolled down, and how far it can, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Travel {
    pub offset: f32,
    pub max: f32,
}

impl Travel {
    fn of(handle: &impl ScrollbarHandle) -> Travel {
        let max = (handle.content_size().height - handle.viewport_bounds().size.height)
            .max(Pixels::ZERO)
            .as_f32();
        Travel {
            offset: (-handle.offset().y.as_f32()).clamp(0., max),
            max,
        }
    }

    /// GPUI's sign: a positive `dy` moves the content down, toward the top.
    pub fn can_move(self, dy: f32) -> bool {
        const SLACK: f32 = 0.5;
        (dy > 0. && self.offset > SLACK) || (dy < 0. && self.offset < self.max - SLACK)
    }
}

/// One wheel event, as the chaining rule reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheel {
    /// Vertical points, positive toward the top.
    pub dy: f32,
    pub phase: TouchPhase,
    /// A trackpad or another device with pixel deltas. A mouse wheel's notches are not, and
    /// carry no gesture.
    pub precise: bool,
}

/// Who owns the gesture under way, as one nested area sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Latch {
    /// No gesture: each event goes to the inner area if it can move that way.
    #[default]
    Free,
    /// A gesture began and has not moved yet.
    Deciding,
    /// The gesture began over this area while it could move, so it keeps it to the end.
    Inner,
    /// The gesture began somewhere else, or at this area's edge.
    Parent,
}

/// What happens to one event over a nested area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The inner area moves and the page does not.
    Inner,
    /// The inner area stays and the page gets the event.
    Parent,
    /// Neither moves: the gesture belongs to the inner area, which is at its edge.
    Neither,
}

/// UX-31's rule. Every nested area sees every wheel event in its window, so it can tell where
/// each gesture began; `over` says whether this one is under the pointer. Returns the area's
/// latch after the event and, when `over`, what the event does.
pub fn route(latch: Latch, wheel: Wheel, over: bool, travel: Travel) -> (Latch, Route) {
    let latch = match wheel.phase {
        TouchPhase::Started => Latch::Deciding,
        TouchPhase::Cancelled => Latch::Free,
        _ if !wheel.precise => Latch::Free,
        _ => latch,
    };
    if wheel.dy == 0. {
        return (latch, Route::Parent);
    }
    let moves = over && travel.can_move(wheel.dy);
    match latch {
        Latch::Free if moves => (latch, Route::Inner),
        Latch::Free => (latch, Route::Parent),
        Latch::Deciding if moves => (Latch::Inner, Route::Inner),
        Latch::Deciding | Latch::Parent => (Latch::Parent, Route::Parent),
        Latch::Inner if moves => (latch, Route::Inner),
        Latch::Inner if over => (latch, Route::Neither),
        Latch::Inner => (latch, Route::Parent),
    }
}

/// Each nested area's [`Latch`], by its id.
#[derive(Default)]
struct Latches(HashMap<SharedString, Latch>);

impl Global for Latches {}

/// [`nested_scroll`]'s area, which keeps its scroll handle across frames.
struct NestedArea {
    key: SharedString,
    handle: Option<ScrollHandle>,
    content: AnyElement,
}

impl IntoElement for NestedArea {
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl RenderOnce for NestedArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = match self.handle {
            Some(handle) => handle,
            None => window
                .use_keyed_state("nested-scroll-handle", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone(),
        };
        let area = div()
            .id("nested-scroll-area")
            .flex_auto()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .child(self.content);
        chain(self.key, handle, area)
            .flex_auto()
            .min_h(px(0.))
            .flex()
            .flex_col()
    }
}

/// The probe's decision for the event in flight, and the area's offset before it moved.
type Pending = Rc<Cell<Option<(Route, Point<Pixels>)>>>;

/// Wraps `area`, a scroll area whose state `handle` reads, in the chaining rule.
///
/// GPUI runs bubble-phase scroll listeners in reverse paint order. The probe is painted after
/// `area`, so it sees each event before `area` moves and records the decision and the offset;
/// the wrapper's own listener was registered before `area`, so it runs after, and either stops
/// the event there or puts `area` back where it was and lets the page have it.
fn chain<H: ScrollbarHandle + Clone>(
    key: SharedString,
    handle: H,
    area: impl IntoElement,
) -> Stateful<Div> {
    let pending = Pending::default();
    let settle = {
        let (pending, handle) = (pending.clone(), handle.clone());
        move |_: &ScrollEvent, _: &mut Window, cx: &mut App| {
            let Some((route, before)) = pending.take() else {
                return;
            };
            if route != Route::Inner && handle.offset() != before {
                handle.set_offset(before);
            }
            if route != Route::Parent {
                cx.stop_propagation();
            }
        }
    };
    let probe_key = key.clone();
    let probe = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let over = hitbox.should_handle_scroll(window);
                let wheel = Wheel {
                    dy: event.delta.pixel_delta(window.line_height()).y.as_f32(),
                    phase: event.touch_phase,
                    precise: event.delta.precise(),
                };
                let latches = &mut cx.default_global::<Latches>().0;
                let latch = latches.get(&probe_key).copied().unwrap_or_default();
                let (latch, route) = route(latch, wheel, over, Travel::of(&handle));
                latches.insert(probe_key.clone(), latch);
                pending.set(over.then(|| (route, handle.offset())));
            });
        },
    )
    .absolute()
    .inset_0();
    div()
        .id(ElementId::Name(format!("{key}-chain").into()))
        .relative()
        .on_scroll(settle)
        .child(area)
        .child(probe)
}

/// A plain div the layout tests can find by `id`. It says nothing to VoiceOver, for a part of a
/// control that already speaks.
pub fn probe(id: impl Into<SharedString>) -> Node {
    div().id(ElementId::Name(id.into())).test_support()
}

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
    tab_with(id, label, None, selected, focus, theme, on_select)
}

/// A [`tab`] with a glyph before its label, such as the dots while the summary is written.
pub fn tab_with(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    leading: Option<AnyElement>,
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
        .child(match leading {
            Some(leading) => div()
                .pt(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(leading)
                .child(label),
            None => div().pt(px(4.)).child(label),
        })
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

/// Text VoiceOver reads in `language`'s voice: a label whose AccessKit text runs carry the
/// BCP 47 tag. AccessKit hands macOS a language only as `AXLanguage` in a label's attributed
/// text, and a label has attributed text only when it has text runs, so a tag on a plain label
/// never reaches VoiceOver. Without a `language` the runs carry none.
pub fn text(id: impl Into<SharedString>, text: String, language: Option<LangTag>) -> Node {
    let id: SharedString = id.into();
    div()
        .id(ElementId::Name(id.clone()))
        .test_support()
        .accessibility_id(id)
        .role(Role::Label)
        .a11y_synthetic_children(move |builder| {
            let bounds = builder.parent_node().bounds();
            for (ix, mut run) in text_runs(&text, language.as_ref()).into_iter().enumerate() {
                if let Some(bounds) = bounds {
                    run.set_bounds(bounds);
                }
                let id = builder.synthetic_node_id(ix);
                builder.push_child(id, run);
            }
        })
}

/// The most characters one AccessKit text run holds: its word starts are `u8` indices.
const RUN_CHARS: usize = u8::MAX as usize;

/// `text` as AccessKit text runs of at most [`RUN_CHARS`] characters, split between words, each
/// tagged with `language` when known.
pub fn text_runs(text: &str, language: Option<&LangTag>) -> Vec<gpui_kit::accesskit::Node> {
    #[derive(Default)]
    struct Run {
        value: String,
        lengths: Vec<u8>,
        starts: Vec<u8>,
    }
    let finish = |run: Run| {
        let mut node = gpui_kit::accesskit::Node::new(Role::TextRun);
        node.set_value(run.value);
        node.set_character_lengths(run.lengths);
        node.set_word_starts(run.starts);
        if let Some(language) = language {
            node.set_language(language.as_str());
        }
        node
    };
    let mut runs = Vec::new();
    let mut run = Run::default();
    for word in words(text) {
        if !run.lengths.is_empty() && run.lengths.len() + word.chars().count() > RUN_CHARS {
            runs.push(finish(std::mem::take(&mut run)));
        }
        for (ix, ch) in word.chars().enumerate() {
            if run.lengths.len() == RUN_CHARS {
                runs.push(finish(std::mem::take(&mut run)));
            }
            if ix == 0 {
                run.starts.push(run.lengths.len() as u8);
            }
            run.value.push(ch);
            run.lengths.push(ch.len_utf8() as u8);
        }
    }
    if !run.lengths.is_empty() {
        runs.push(finish(run));
    }
    runs
}

/// `text`'s words as AccessKit counts them: each with the whitespace after it.
fn words(text: &str) -> Vec<&str> {
    let mut after_space = false;
    let starts: Vec<usize> = text
        .char_indices()
        .filter_map(|(i, ch)| {
            let start = i == 0 || (after_space && !ch.is_whitespace());
            after_space = ch.is_whitespace();
            start.then_some(i)
        })
        .chain([text.len()])
        .collect();
    starts.windows(2).map(|w| &text[w[0]..w[1]]).collect()
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

    const TOP: Travel = Travel {
        offset: 0.,
        max: 500.,
    };
    const MIDDLE: Travel = Travel {
        offset: 200.,
        max: 500.,
    };
    const BOTTOM: Travel = Travel {
        offset: 500.,
        max: 500.,
    };
    const UP: f32 = 40.;
    const DOWN: f32 = -40.;

    fn wheel(dy: f32) -> Wheel {
        Wheel {
            dy,
            phase: TouchPhase::Moved,
            precise: false,
        }
    }

    fn pad(dy: f32, phase: TouchPhase) -> Wheel {
        Wheel {
            dy,
            phase,
            precise: true,
        }
    }

    /// Runs `events` over one area from a fresh latch and returns each event's route.
    fn routes(events: &[(Wheel, bool, Travel)]) -> Vec<Route> {
        let mut latch = Latch::default();
        events
            .iter()
            .map(|&(wheel, over, travel)| {
                let (next, route) = route(latch, wheel, over, travel);
                latch = next;
                route
            })
            .collect()
    }

    #[test]
    fn a_mouse_wheel_moves_the_inner_area_until_its_edge_then_the_page() {
        use Route::*;
        assert_eq!(
            routes(&[
                (wheel(UP), true, MIDDLE),
                (wheel(UP), true, TOP),
                (wheel(DOWN), true, TOP),
                (wheel(DOWN), true, BOTTOM),
                (wheel(UP), true, BOTTOM),
            ]),
            [Inner, Parent, Inner, Parent, Inner]
        );
    }

    #[test]
    fn a_gesture_that_begins_inside_stays_inside_momentum_and_all() {
        use Route::*;
        use TouchPhase::*;
        assert_eq!(
            routes(&[
                (pad(0., Started), true, MIDDLE),
                (pad(UP, Moved), true, MIDDLE),
                (pad(UP, Moved), true, TOP),
                (pad(0., Ended), true, TOP),
                (pad(UP, Moved), true, TOP),
                (pad(DOWN, Moved), true, TOP),
            ]),
            [Parent, Inner, Neither, Parent, Neither, Inner],
            "at the top mid-gesture neither moves, and momentum after the lift stays inside"
        );
    }

    #[test]
    fn a_gesture_that_begins_at_the_edge_goes_to_the_page_even_if_it_turns_back() {
        use Route::*;
        use TouchPhase::*;
        assert_eq!(
            routes(&[
                (pad(0., Started), true, TOP),
                (pad(UP, Moved), true, TOP),
                (pad(DOWN, Moved), true, TOP),
                (pad(DOWN, Moved), true, MIDDLE),
            ]),
            [Parent, Parent, Parent, Parent]
        );
    }

    #[test]
    fn a_gesture_that_begins_outside_never_moves_the_area_it_slides_under() {
        use Route::*;
        use TouchPhase::*;
        assert_eq!(
            routes(&[
                (pad(0., Started), false, MIDDLE),
                (pad(DOWN, Moved), false, MIDDLE),
                (pad(DOWN, Moved), true, MIDDLE),
                (pad(0., Started), true, MIDDLE),
                (pad(DOWN, Moved), true, MIDDLE),
            ]),
            [Parent, Parent, Parent, Parent, Inner],
            "only the next gesture moves the area"
        );
    }

    #[test]
    fn a_mouse_wheel_ends_any_latch() {
        use Route::*;
        use TouchPhase::*;
        assert_eq!(
            routes(&[
                (pad(0., Started), true, TOP),
                (pad(UP, Moved), true, TOP),
                (wheel(DOWN), true, TOP),
            ]),
            [Parent, Parent, Inner]
        );
    }

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
