//! The "Copied" flash every Copy button shows (UX-23).
//!
//! A Copy button names what it copies with a key. [`copy`] writes the text, says so to
//! VoiceOver, and marks that key copied for [`COPIED_FOR`]. The button reads [`is`] when it
//! renders and draws a check and "Copied" in place of its usual look. One copy at a time is
//! marked, so copying something else moves the flash to the new button. A copy from a menu or a
//! shortcut flashes the button in view too, because the key names the text, not the button.

use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AsyncApp, ClipboardItem, Global, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, TestSupportExt as _, div,
};

use crate::a11y;
use crate::icons::Icon;
use crate::theme::Theme;

/// Settings › Assistants › Copy Setup.
pub const CONNECT_SETUP: &str = "connect-setup";

/// How long a Copy button shows that it copied.
pub const COPIED_FOR: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Flash {
    key: Option<SharedString>,
    /// Counts copies, so only the latest one's timer clears the flash.
    generation: u64,
}

impl Global for Flash {}

/// Puts `text` on the clipboard and flashes `key`'s Copy button.
pub fn copy(key: impl Into<SharedString>, text: String, said: &'static str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    a11y::announce(said);
    flash(key.into(), cx);
}

/// Whether `key`'s Copy button should show that it just copied.
pub fn is(key: &str, cx: &App) -> bool {
    cx.try_global::<Flash>()
        .and_then(|flash| flash.key.as_deref())
        .is_some_and(|copied| copied == key)
}

/// A Copy button's icon and label: the copy icon and `label` at rest, a check and "Copied" just
/// after it copied.
pub fn face(key: &str, label: &'static str, theme: &Theme, cx: &App) -> [AnyElement; 2] {
    let (id, icon, color, label) = if is(key, cx) {
        ("copy-icon-check", Icon::Check, theme.accent, "Copied")
    } else {
        ("copy-icon-copy", Icon::Copy, theme.text_secondary, label)
    };
    [
        div()
            .id(id)
            .test_support()
            .child(icon.view(13., color))
            .into_any_element(),
        label.into_any_element(),
    ]
}

fn flash(key: SharedString, cx: &mut App) {
    let state = cx.default_global::<Flash>();
    state.key = Some(key);
    state.generation += 1;
    let generation = state.generation;
    cx.refresh_windows();
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(COPIED_FOR).await;
        cx.update(|cx| {
            let state = cx.default_global::<Flash>();
            if state.generation == generation {
                state.key = None;
                cx.refresh_windows();
            }
        });
    })
    .detach();
}
