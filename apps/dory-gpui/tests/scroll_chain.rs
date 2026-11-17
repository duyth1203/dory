//! UX-31 in the main window (`docs/design/settings-icons-blur.md` section 4.2.3): the long
//! transcript list is a scroll area inside the page. It takes the wheel while it can move, the
//! page gets the rest, and a trackpad gesture stays with whichever one it began in. The same
//! rule holds for `a11y::nested_scroll` on a plain page.

mod common;

use common::{dory_on, id_of, open_with, sample_store};
use dory_core::domain::{Channel, Ms, Revision, Segment, SpeakerId, TimeRange, Transcript};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store as _};
use dory_gpui::a11y;
use dory_gpui::app::DetailTab;
use dory_gpui::library::Selection;
use dory_gpui::transcript_view::PAGED_TURNS;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, InputEvent as _, IntoElement, ParentElement as _,
    Pixels, Point, Render, Role, ScrollDelta, ScrollEvent, ScrollSource,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase, Window, div, point,
    px, test::TestWindowExt as _,
};

const LONG: &str = "Roadmap review";
const TURNS: usize = 150;
const STEPS: usize = 400;

fn open_long(cx: &mut TestAppContext) -> AnyWindowHandle {
    const { assert!(TURNS > PAGED_TURNS) };
    let store = sample_store();
    let segments = (0..TURNS)
        .map(|i| {
            let start = i as u64 * 10_000;
            Segment {
                range: TimeRange::new(Ms(start), Ms(start + 9_000)),
                speaker: SpeakerId(1 + (i % 2) as u16),
                channel: Channel::System,
                text: format!("Line {i}."),
            }
        })
        .collect();
    store
        .apply(vec![Change::TranscriptReplaced {
            id: id_of(LONG),
            transcript: Transcript {
                revision: Revision(2),
                segments,
                detected_languages: Vec::new(),
                produced_by: ModelRef::parse("fake/sample").unwrap(),
            },
        }])
        .expect("a long transcript");
    let (window, app) = open_with(cx, dory_on(store));
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of(LONG)), cx);
        app.set_tab(DetailTab::Transcript, cx);
    });
    cx.run_until_parked();
    window
}

/// Where the page and the list have scrolled: the list's top in the window, which moves with
/// the page, and the first turn the list draws with its top relative to the list's top.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scrolled {
    page: Pixels,
    list: (usize, Pixels),
}

fn scrolled(window: &mut Window) -> Scrolled {
    let list = window.find("transcript").bounds();
    let (ix, turn) = (0..TURNS)
        .find_map(|ix| {
            window
                .try_find(format!("turn-{ix}"))
                .map(|t| (ix, t.bounds()))
        })
        .expect("the list draws a turn");
    Scrolled {
        page: list.top(),
        list: (ix, turn.top() - list.top()),
    }
}

/// A point over the part of the list on screen, below the toolbar and the sticky tab row.
fn over_list(window: &mut Window) -> Point<Pixels> {
    let list = window.find("transcript").bounds();
    point(list.center().x, list.top().max(px(150.)) + px(100.))
}

/// A point over the recording's header, above the list, while the page is at its top.
fn over_header(window: &mut Window) -> Point<Pixels> {
    let list = window.find("transcript").bounds();
    point(list.center().x, list.top() - px(40.))
}

fn send(
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
    at: fn(&mut Window) -> Point<Pixels>,
    delta: ScrollDelta,
    phase: TouchPhase,
) -> Scrolled {
    let after = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let position = at(window);
            window.dispatch_event(
                ScrollEvent {
                    position,
                    delta,
                    modifiers: Default::default(),
                    touch_phase: phase,
                    source: ScrollSource::Pointer,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            scrolled(window)
        })
        .unwrap();
    cx.run_until_parked();
    after
}

/// Mouse wheel notches, positive toward the top.
fn wheel(window: AnyWindowHandle, cx: &mut TestAppContext, lines: f32) -> Scrolled {
    send(
        window,
        cx,
        over_list,
        ScrollDelta::Lines(point(0., lines)),
        TouchPhase::Moved,
    )
}

/// One trackpad event over the list, in points, positive toward the top.
fn pad(window: AnyWindowHandle, cx: &mut TestAppContext, dy: f32, phase: TouchPhase) -> Scrolled {
    send(
        window,
        cx,
        over_list,
        ScrollDelta::Pixels(point(px(0.), px(dy))),
        phase,
    )
}

fn now(window: AnyWindowHandle, cx: &mut TestAppContext) -> Scrolled {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        scrolled(window)
    })
    .unwrap()
}

/// Scrolls the page to its bottom from the header, which leaves the list at its top.
fn page_to_bottom(window: AnyWindowHandle, cx: &mut TestAppContext) -> Scrolled {
    let start = now(window, cx);
    let after = send(
        window,
        cx,
        over_header,
        ScrollDelta::Pixels(point(px(0.), px(-5_000.))),
        TouchPhase::Moved,
    );
    assert!(after.page < start.page, "the page scrolled: {after:?}");
    assert_eq!(after.list, start.list, "the list stayed at its top");
    after
}

#[gpui_kit::test]
fn the_wheel_reaches_the_lists_edge_before_the_page_scrolls(cx: &mut TestAppContext) {
    let window = open_long(cx);
    let top = now(window, cx);
    assert_eq!(top.list.0, 0, "the list starts at its top");

    let mut at = top;
    let mut steps = 0;
    loop {
        let next = wheel(window, cx, -5.);
        steps += 1;
        assert!(steps < STEPS, "the wheel never reached the list's bottom");
        if next.list == at.list {
            assert!(next.page < at.page, "at the list's bottom the page scrolls");
            at = next;
            break;
        }
        assert_eq!(next.page, at.page, "the page waits while the list moves");
        at = next;
    }
    let page_down = loop {
        let next = wheel(window, cx, -5.);
        if next.page == at.page {
            break next;
        }
        at = next;
    };

    at = page_down;
    let mut steps = 0;
    while at.list != top.list {
        let next = wheel(window, cx, 5.);
        steps += 1;
        assert!(
            steps < STEPS,
            "the wheel never brought the list back to its top"
        );
        assert!(next.list != at.list, "the list moves up while it can");
        assert_eq!(
            next.page, page_down.page,
            "the page stays while the list moves"
        );
        at = next;
    }
    let past = wheel(window, cx, 5.);
    assert_eq!(past.list, top.list);
    assert!(
        past.page > page_down.page,
        "past the list's top the page scrolls: {past:?} from {page_down:?}"
    );
}

#[gpui_kit::test]
fn a_gesture_that_starts_at_the_lists_edge_goes_to_the_page(cx: &mut TestAppContext) {
    let window = open_long(cx);
    let down = page_to_bottom(window, cx);

    pad(window, cx, 0., TouchPhase::Started);
    let up = pad(window, cx, 60., TouchPhase::Moved);
    assert_eq!(up.list, down.list, "the list at its top stays");
    assert!(up.page > down.page, "the page scrolls up: {up:?}");

    let back = pad(window, cx, -30., TouchPhase::Moved);
    assert_eq!(
        back.list, down.list,
        "turning back mid-gesture still moves the page, not the list"
    );
    assert!(
        back.page < up.page,
        "the page follows the gesture: {back:?}"
    );
    pad(window, cx, 0., TouchPhase::Ended);
}

#[gpui_kit::test]
fn within_one_gesture_the_page_waits_while_the_list_can_move(cx: &mut TestAppContext) {
    let window = open_long(cx);
    let down = page_to_bottom(window, cx);

    pad(window, cx, 0., TouchPhase::Started);
    let mut at = down;
    for _ in 0..5 {
        let next = pad(window, cx, -60., TouchPhase::Moved);
        assert!(next.list != at.list, "the list scrolls down");
        assert_eq!(next.page, down.page, "the page waits");
        at = next;
    }
    for _ in 0..20 {
        at = pad(window, cx, 60., TouchPhase::Moved);
        assert_eq!(
            at.page, down.page,
            "the gesture stays with the list, even past its top"
        );
    }
    assert_eq!(at.list, down.list, "the list is back at its top");
    pad(window, cx, 0., TouchPhase::Ended);
    let momentum = pad(window, cx, 30., TouchPhase::Moved);
    assert_eq!(momentum.page, down.page, "momentum stays with the list");

    pad(window, cx, 0., TouchPhase::Started);
    let next_gesture = pad(window, cx, 60., TouchPhase::Moved);
    assert_eq!(next_gesture.list, down.list);
    assert!(
        next_gesture.page > down.page,
        "the next gesture, at the list's top, scrolls the page"
    );
}

/// A page 600 pt tall: a header, a nested area showing 200 pt of its 1,200, and a long footer.
struct Page;

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..40).map(|ix| {
            a11y::node(format!("row-{ix}"), Role::ListItem, "Row")
                .flex_none()
                .h(px(30.))
        });
        a11y::node("page", Role::Main, "Page")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(div().flex_none().h(px(300.)))
            .child(
                a11y::node("box", Role::Group, "Box").flex_none().child(
                    a11y::nested_scroll("inner", div().flex().flex_col().children(rows))
                        .max_h(px(200.)),
                ),
            )
            .child(div().flex_none().h(px(800.)))
    }
}

/// The box's top, which moves with the page, and how far the area has scrolled.
fn page_and_area(window: &mut Window) -> (Pixels, Pixels) {
    let area = window.find("box").bounds();
    (area.top(), area.top() - window.find("row-0").bounds().top())
}

fn wheel_over_box(
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
    lines: f32,
) -> (Pixels, Pixels) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let position = window.find("box").bounds().center();
        window.dispatch_event(
            ScrollEvent {
                position,
                delta: ScrollDelta::Lines(point(0., lines)),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        page_and_area(window)
    })
    .unwrap()
}

#[gpui_kit::test]
fn a_nested_scroll_area_hands_the_page_what_it_cannot_use(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.add_window(|_, _| Page).into();
    let (page_top, _) = wheel_over_box(window, cx, 0.);

    let mut at = (page_top, px(0.));
    let mut steps = 0;
    while at.1 < px(1_000.) {
        let next = wheel_over_box(window, cx, -3.);
        steps += 1;
        assert!(steps < STEPS, "the area never reached its bottom: {next:?}");
        assert!(next.1 > at.1, "the area moves down while it can");
        assert_eq!(next.0, page_top, "the page waits while the area moves");
        at = next;
    }
    assert_eq!(at.1, px(1_000.), "the area stops at its bottom");
    let past = wheel_over_box(window, cx, -3.);
    assert_eq!(past.1, px(1_000.));
    assert!(past.0 < page_top, "past the area's bottom the page scrolls");

    let mut at = past;
    while at.1 > px(0.) {
        let next = wheel_over_box(window, cx, 3.);
        assert!(next.1 < at.1, "the area moves up while it can");
        assert_eq!(next.0, past.0, "the page waits while the area moves");
        at = next;
    }
    let top = wheel_over_box(window, cx, 3.);
    assert_eq!(top.1, px(0.));
    assert!(top.0 > past.0, "past the area's top the page scrolls");
}
