//! About Dory: the license text reads as a document, not as Markdown, its list of credits stays
//! inside the window with a scrollbar, and the window's top edge carries the theme's color.

mod common;

use dory_gpui::about;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Pixels, TestAppContext, test::TestWindowExt as _,
};

use common::open;

fn open_about(cx: &mut TestAppContext) -> AnyWindowHandle {
    let (main, _) = open(cx);
    let before = cx.update(|cx| cx.windows());
    cx.update(about::open);
    cx.run_until_parked();
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| *w != main && !before.contains(w))
        .expect("the About window opened");
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    window
}

fn bounds_of(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<Bounds<Pixels>> {
    cx.update_window(window, |_, window, _| {
        window.try_find(id.to_owned()).map(|e| e.bounds())
    })
    .unwrap()
}

#[gpui_kit::test]
fn a_rule_in_the_license_is_a_divider_not_three_hyphens(cx: &mut TestAppContext) {
    let window = open_about(cx);
    let lucide = about::credits().remove(0);
    let paragraphs = about::paragraphs(lucide.license);
    let rule = paragraphs
        .iter()
        .position(|p| about::is_rule(p))
        .expect("Lucide's license has a `---` line between its two licenses");
    assert!(
        bounds_of(window, &format!("acknowledgement-lucide-{rule}"), cx).is_none(),
        "the `---` line is drawn as text"
    );
    assert!(
        bounds_of(window, &format!("acknowledgement-lucide-{rule}-rule"), cx).is_some(),
        "the `---` line has no divider"
    );
    assert!(
        bounds_of(window, &format!("acknowledgement-lucide-{}", rule + 1), cx).is_some(),
        "the paragraph after the rule is still text"
    );
}

#[gpui_kit::test]
fn the_credits_scroll_inside_the_window_under_a_scrollbar(cx: &mut TestAppContext) {
    let window = open_about(cx);
    let height = cx
        .update_window(window, |_, window, _| window.viewport_size().height)
        .unwrap();
    let body = bounds_of(window, "about-body", cx).expect("the credits sit in a scroll area");
    assert!(
        body.bottom() <= height + Pixels::from(0.5),
        "the scroll area ends at {:?} in a window {height:?} tall",
        body.bottom()
    );
    let last = about::paragraphs(about::credits().last().unwrap().license).len() - 1;
    let end = bounds_of(window, &format!("acknowledgement-literata-{last}"), cx)
        .expect("the last paragraph is laid out");
    assert!(
        end.bottom() > height,
        "the credits fit the window at {:?}: nothing scrolls, so this checks nothing",
        end.bottom()
    );
}

#[gpui_kit::test]
fn the_title_bar_is_the_themes_own_color(cx: &mut TestAppContext) {
    let titlebar = about::titlebar();
    assert!(
        titlebar.appears_transparent,
        "a native title bar paints Kit's pure white over Book's paper"
    );
    assert!(titlebar.traffic_light_position.is_some());
    let window = open_about(cx);
    let strip = bounds_of(window, "about-titlebar", cx).expect("a strip for the traffic lights");
    assert_eq!(
        f32::from(strip.top()),
        0.,
        "the strip starts at the top edge"
    );
    assert!(f32::from(strip.size.height) >= 28., "{strip:?}");
}
