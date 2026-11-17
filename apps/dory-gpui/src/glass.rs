//! Window glass (UX-25, A11Y-6, `docs/design/settings-icons-blur.md` section 5).
//!
//! The main and Settings windows blur the desktop behind them. The sidebar column, its 8 pt
//! margins included, lets the blur through in `glass_tint`, the sidebar panel adds
//! `sidebar_tint`, and the content pane stays opaque in `window_bg`. Popovers, menus, toasts, and
//! the docked player keep their solid fills: GPUI cannot blur one element over another.
//!
//! Reduce Transparency or Increase Contrast turns all of it off: the window goes opaque, the
//! column takes `window_bg`, and the panel solid `sidebar_bg`, as before glass. `system_prefs`
//! reports Reduce Transparency here; Increase Contrast arrives through the theme.

use std::collections::HashMap;

use gpui_kit::{App, Global, Hsla, Window, WindowBackgroundAppearance, WindowId};

use crate::theme::Theme;

#[derive(Default)]
struct Transparency {
    reduce: bool,
}

impl Global for Transparency {}

/// The appearance [`follow`] last gave each window, so it changes the window only on a change.
#[derive(Default)]
struct Applied(HashMap<WindowId, WindowBackgroundAppearance>);

impl Global for Applied {}

/// Called by `system_prefs` at launch and on every change of the setting.
pub fn set_reduce_transparency(reduce: bool, cx: &mut App) {
    if cx.try_global::<Transparency>().map(|t| t.reduce) != Some(reduce) {
        cx.set_global(Transparency { reduce });
    }
}

fn translucent(high_contrast: bool, cx: &App) -> bool {
    !high_contrast && !cx.try_global::<Transparency>().is_some_and(|t| t.reduce)
}

pub fn window_background(cx: &App) -> WindowBackgroundAppearance {
    if translucent(
        cx.try_global::<Theme>().is_some_and(|t| t.high_contrast),
        cx,
    ) {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

pub fn sidebar_column(theme: &Theme, cx: &App) -> Hsla {
    if translucent(theme.high_contrast, cx) {
        theme.glass_tint
    } else {
        theme.window_bg
    }
}

pub fn sidebar_panel(theme: &Theme, cx: &App) -> Hsla {
    if translucent(theme.high_contrast, cx) {
        theme.sidebar_tint
    } else {
        theme.sidebar_bg
    }
}

/// Keeps `window`'s background in step with Reduce Transparency and Increase Contrast for as long
/// as the window is open. Call it once, when the window opens.
pub fn follow(window: &mut Window, cx: &mut App) {
    sync(window, cx);
    window.observe_global::<Transparency>(cx, sync).detach();
    window.observe_global::<Theme>(cx, sync).detach();
}

fn sync(window: &mut Window, cx: &mut App) {
    let want = window_background(cx);
    let id = window.window_handle().window_id();
    if cx.default_global::<Applied>().0.insert(id, want) != Some(want) {
        window.set_background_appearance(want);
    }
}

/// The background [`follow`] last set on `window`, for tests: the test platform reports every
/// window as opaque.
pub fn applied(window: &Window, cx: &App) -> Option<WindowBackgroundAppearance> {
    cx.try_global::<Applied>()?
        .0
        .get(&window.window_handle().window_id())
        .copied()
}
