//! Icons and status glyphs grow with the text (View > Bigger), as the Processing Queue's badge
//! does, so they keep their weight next to words that doubled. Below 100% they stay put, as
//! Kit's controls do (A11Y-9).

mod common;

use dory_gpui::icons::Icon;
use dory_gpui::library::Glyph;
use dory_gpui::prefs::Prefs;
use dory_gpui::processing::StepGlyph;
use dory_gpui::theme::Theme;
use dory_gpui::{a11y, glyphs};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, div, px, test::TestWindowExt as _,
};

use common::{dory_on, open_with_prefs, past_first_run, sample_store};

struct Specimens;

impl Render for Specimens {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color = Theme::get(cx).text;
        div()
            .flex()
            .items_start()
            .gap(px(4.))
            .child(a11y::probe("icon").child(Icon::Search.view(14., color)))
            .child(a11y::probe("row-glyph").child(glyphs::row(Glyph::Check, color, 12., None)))
            .child(a11y::probe("dots").child(glyphs::dots(color, 12., None)))
            .child(a11y::probe("step").child(glyphs::step(StepGlyph::Pending, color, 14., None)))
    }
}

fn sizes(text_scale: f32, cx: &mut TestAppContext) -> Vec<(&'static str, f32)> {
    let prefs = Prefs {
        text_scale,
        ..past_first_run()
    };
    let _ = open_with_prefs(cx, dory_on(sample_store()), prefs);
    let window: AnyWindowHandle = cx
        .update(|cx| gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Specimens)))
        .expect("open the specimens")
        .0;
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        ["icon", "row-glyph", "dots", "step"]
            .into_iter()
            .map(|id| {
                let b = window.find(id).bounds();
                assert_eq!(b.size.width, b.size.height, "{id} stays square");
                (id, f32::from(b.size.width))
            })
            .collect()
    })
    .unwrap()
}

#[gpui_kit::test]
fn icons_and_glyphs_double_at_200_percent_text(cx: &mut TestAppContext) {
    let got = sizes(2.0, cx);
    for (id, at_100) in [
        ("icon", 14.),
        ("row-glyph", 12.),
        ("dots", 12.),
        ("step", 14.),
    ] {
        let size = got.iter().find(|(i, _)| *i == id).unwrap().1;
        assert!(
            (size - at_100 * 2.).abs() < 0.5,
            "{id} is {size} pt at 200% text, want {}",
            at_100 * 2.
        );
    }
}

#[gpui_kit::test]
fn icons_and_glyphs_keep_their_size_below_100_percent_text(cx: &mut TestAppContext) {
    let got = sizes(0.85, cx);
    for (id, at_100) in [
        ("icon", 14.),
        ("row-glyph", 12.),
        ("dots", 12.),
        ("step", 14.),
    ] {
        let size = got.iter().find(|(i, _)| *i == id).unwrap().1;
        assert!((size - at_100).abs() < 0.5, "{id} is {size} pt at 85% text");
    }
}
