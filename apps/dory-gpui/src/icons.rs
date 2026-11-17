//! The Lucide icons Dory draws (`docs/design/settings-icons-blur.md` section 2, UX-25), vendored
//! into `assets/icons/` by `scripts/vendor-lucide.sh` and compiled into the binary.
//!
//! [`Icon::view`] draws one at a fixed size, tinted by its color: 12 in rows and status, 14 in
//! buttons and tabs, 16 by default, 20 in the transport. The `Transport` icons are 1.5-stroke
//! copies for that size. An icon is a graphic with no accessibility node, so VoiceOver reads the
//! word beside it, or the label of the button it sits in, and never the icon.
//!
//! [`Assets`] serves the same files to the GPUI Kit components that draw a Lucide icon by path,
//! such as a menu's check mark.

use std::borrow::Cow;

use gpui_kit::{
    AssetSource, Div, Hsla, IntoElement, ParentElement as _, Pixels, Result, SharedString,
    Styled as _, Svg, div, px, svg,
};

macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant,)*
        }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$variant,)*];

            /// The file's name in `assets/icons/`.
            pub fn file(self) -> &'static str {
                match self {
                    $(Icon::$variant => $file,)*
                }
            }

            pub fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$variant => include_bytes!(concat!("../assets/icons/", $file)),)*
                }
            }
        }
    };
}

icons! {
    BatteryMedium => "battery-medium.svg",
    Bot => "bot.svg",
    Captions => "captions.svg",
    Check => "check.svg",
    ChevronDown => "chevron-down.svg",
    ChevronRight => "chevron-right.svg",
    ChevronsUpDown => "chevrons-up-down.svg",
    Circle => "circle.svg",
    CircleCheck => "circle-check.svg",
    CircleDashed => "circle-dashed.svg",
    CircleStop => "circle-stop.svg",
    CircleX => "circle-x.svg",
    Clock => "clock.svg",
    Copy => "copy.svg",
    Ellipsis => "ellipsis.svg",
    FileText => "file-text.svg",
    GripVertical => "grip-vertical.svg",
    HardDrive => "hard-drive.svg",
    Info => "info.svg",
    Mic => "mic.svg",
    Pause => "pause.svg",
    Play => "play.svg",
    Plus => "plus.svg",
    Search => "search.svg",
    Settings2 => "settings-2.svg",
    Square => "square.svg",
    TransportPause => "transport-pause.svg",
    TransportPlay => "transport-play.svg",
    TransportRotateCcw => "transport-rotate-ccw.svg",
    TransportRotateCw => "transport-rotate-cw.svg",
    TriangleAlert => "triangle-alert.svg",
    X => "x.svg",
}

impl Icon {
    pub fn view(self, size: f32, color: Hsla) -> Svg {
        svg()
            .data(self.bytes())
            .flex_none()
            .size(px(size))
            .text_color(color)
    }

    /// The icon a GPUI Kit component asks for at `path`, such as `icons/check.svg`.
    pub fn at_path(path: &str) -> Option<Icon> {
        let file = path.strip_prefix("icons/")?;
        // Kit's dialog close button keeps a pre-Lucide name for Lucide's `x`.
        let file = if file == "close.svg" { "x.svg" } else { file };
        Icon::ALL.iter().copied().find(|icon| icon.file() == file)
    }
}

/// `icon` in `color` on the first line of `text`, which wraps under itself: an inline warning, or
/// the Playing marker. `line` is the text's line height.
pub fn beside(icon: Icon, size: f32, color: Hsla, line: Pixels, text: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(4.))
        .line_height(line)
        .child(
            div()
                .flex_none()
                .h(line)
                .flex()
                .items_center()
                .child(icon.view(size, color)),
        )
        .child(div().min_w_0().child(text))
}

/// The icons as GPUI's asset source, for `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::at_path(path).map(|icon| Cow::Borrowed(icon.bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .map(|icon| format!("icons/{}", icon.file()))
            .filter(|p| p.starts_with(path))
            .map(Into::into)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_components_find_the_lucide_files_by_path() {
        let load = |path| Assets.load(path).unwrap().map(|b| b.into_owned());
        assert_eq!(load("icons/check.svg"), Some(Icon::Check.bytes().to_vec()));
        assert_eq!(
            load("icons/chevron-right.svg"),
            Some(Icon::ChevronRight.bytes().to_vec())
        );
        assert_eq!(load("icons/close.svg"), Some(Icon::X.bytes().to_vec()));
        assert_eq!(load("icons/github.svg"), None);
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
    }
}
