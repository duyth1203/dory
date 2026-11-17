//! Dory › About Dory: the app's name and version, and Acknowledgements with the license of every
//! asset Dory ships that someone else made. Today that is Lucide's icons, whose license (ISC, and
//! MIT for the icons that came from Feather) asks for its text to travel with them.

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, FontWeight, Global, IntoElement,
    ParentElement as _, Render, Role, ScrollHandle, StatefulInteractiveElement as _, Styled as _,
    Window, WindowBounds, WindowOptions, div, px, relative, size,
};

use crate::a11y;
use crate::theme::Theme;

pub const WIDTH: f32 = 440.;
pub const HEIGHT: f32 = 640.;

const LUCIDE_VERSION: &str = include_str!("../assets/icons/VERSION");
/// The app icon at 128 pt, rendered at 2x from `Dory.icns`, so it keeps the icon grid and the
/// transparent corners that `scripts/make-icon.sh` cuts from `icon/source.png`.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/about/dory-icon.png");
pub const ICON_SIZE: f32 = 128.;
const LUCIDE_LICENSE: &str = include_str!("../assets/icons/LICENSE");

/// One acknowledged work: what it is and its license, as the license file wrote it.
pub struct Credit {
    pub id: &'static str,
    pub name: String,
    pub license: &'static str,
}

pub fn credits() -> Vec<Credit> {
    vec![Credit {
        id: "lucide",
        name: format!("Lucide icons {}", LUCIDE_VERSION.trim()),
        license: LUCIDE_LICENSE,
    }]
}

/// The license's paragraphs, with the file's hard line breaks inside each one undone so the text
/// wraps to the window.
pub fn paragraphs(license: &str) -> Vec<String> {
    license
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

struct Open(AnyWindowHandle);

impl Global for Open {}

/// Opens the About window, or brings it forward when it is open.
pub fn open(cx: &mut App) {
    if let Some(handle) = cx.try_global::<Open>().map(|open| open.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(gpui_kit::TitlebarOptions {
            title: Some("About Dory".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(WIDTH), px(HEIGHT)),
            cx,
        ))),
        window_min_size: Some(size(px(WIDTH), px(320.))),
        ..Default::default()
    };
    if let Ok((handle, _)) = gpui_kit::open_window(options, cx, |window, cx| {
        crate::platform::print_window_id(window);
        cx.new(|_| About {
            scroll: ScrollHandle::new(),
        })
    }) {
        cx.set_global(Open(handle));
    }
}

pub struct About {
    scroll: ScrollHandle,
}

impl Render for About {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
        let credits = credits().into_iter().map(|credit| {
            let paragraphs = paragraphs(credit.license);
            a11y::node(
                format!("acknowledgement-{}", credit.id),
                Role::Group,
                credit.name.clone(),
            )
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                a11y::node(
                    format!("acknowledgement-{}-name", credit.id),
                    Role::Heading,
                    credit.name.clone(),
                )
                .aria_level(3)
                .text_size(theme.text(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(credit.name),
            )
            .children(paragraphs.into_iter().enumerate().map(|(ix, text)| {
                a11y::node(
                    format!("acknowledgement-{}-{ix}", credit.id),
                    Role::Label,
                    text.clone(),
                )
                .text_size(theme.text(11.))
                .line_height(relative(1.45))
                .text_color(theme.text_secondary)
                .child(text)
            }))
        });
        a11y::node("dory-about", Role::Document, "About Dory")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(theme.window_bg)
            .text_color(theme.text)
            .px(px(28.))
            .py(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        a11y::node("dory-icon", Role::Image, "Dory app icon")
                            .size(px(ICON_SIZE))
                            .mb(px(8.))
                            .child(
                                gpui_kit::img(std::sync::Arc::new(gpui_kit::Image::from_bytes(
                                    gpui_kit::ImageFormat::Png,
                                    ICON_PNG.to_vec(),
                                )))
                                .size_full(),
                            ),
                    )
                    .child(
                        a11y::node("dory-name", Role::Heading, "Dory")
                            .aria_level(1)
                            .text_size(theme.text(20.))
                            .font_weight(FontWeight::BOLD)
                            .child("Dory"),
                    )
                    .child(
                        a11y::node("dory-version", Role::Label, version.clone())
                            .text_size(theme.text(12.))
                            .text_color(theme.text_secondary)
                            .child(version),
                    ),
            )
            .child(
                a11y::node("acknowledgements", Role::Heading, "Acknowledgements")
                    .aria_level(2)
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(theme.divider)
                    .text_size(theme.text(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Acknowledgements"),
            )
            .children(credits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lucide_license_ships_whole_with_its_version() {
        let lucide = &credits()[0];
        assert_eq!(
            lucide.name,
            format!("Lucide icons {}", LUCIDE_VERSION.trim())
        );
        let text = paragraphs(lucide.license).join("\n");
        assert!(text.starts_with("ISC License"), "{text}");
        assert!(text.contains("Lucide Icons and Contributors"));
        assert!(text.contains("derived from the Feather project"));
        assert!(text.contains("The MIT License (MIT)"));
        assert!(text.contains("Copyright (c) 2013-present Cole Bemis"));
    }

    #[test]
    fn paragraphs_undo_hard_line_breaks() {
        assert_eq!(
            paragraphs("One\nline.\n\n\nTwo  lines\nhere.\n"),
            vec!["One line.", "Two lines here."]
        );
    }
}
