//! The three macOS display settings Dory follows (A11Y-5, A11Y-6): Reduce motion, Increase
//! contrast, and Differentiate without color, from `NSWorkspace`, read at launch and again on
//! every `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`.
//!
//! Reduce motion goes to GPUI's `set_reduce_motion`, which the level meter, the docked player's
//! fade, and Kit's own transitions read. The other two rebuild the theme through
//! `theme::set_display`. All of the app's AppKit observation lives here.

use gpui_kit::{App, AsyncApp};

use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemPrefs {
    pub reduce_motion: bool,
    pub increase_contrast: bool,
    pub differentiate_without_color: bool,
}

/// Applies `prefs` to GPUI and the theme.
pub fn apply(prefs: SystemPrefs, cx: &mut App) {
    if cx.reduce_motion() != prefs.reduce_motion {
        cx.set_reduce_motion(prefs.reduce_motion);
    }
    theme::set_display(
        |display| {
            display.increase_contrast = prefs.increase_contrast;
            display.differentiate_without_color = prefs.differentiate_without_color;
        },
        cx,
    );
    eprintln!(
        "system-prefs: reduce motion {}, increase contrast {}, differentiate without color {}",
        prefs.reduce_motion, prefs.increase_contrast, prefs.differentiate_without_color
    );
}

/// Reads the settings now, applies them, and follows every later change for the life of the
/// app. Called once by `main`.
pub fn install(cx: &mut App) {
    apply(read(), cx);
    let changes = observe();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while changes.recv().await.is_ok() {
            cx.update(|cx| apply(read(), cx));
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
pub fn read() -> SystemPrefs {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    SystemPrefs {
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        increase_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
        differentiate_without_color: workspace
            .accessibilityDisplayShouldDifferentiateWithoutColor(),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read() -> SystemPrefs {
    SystemPrefs::default()
}

/// A channel that receives one message per display-options change. The observer is never
/// removed: the app follows the setting until it quits.
#[cfg(target_os = "macos")]
fn observe() -> async_channel::Receiver<()> {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::{NSNotification, NSOperationQueue};

    let (tx, rx) = async_channel::unbounded();
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
        let _ = tx.try_send(());
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the name is AppKit's immutable notification constant, the object filter is nil,
    // the main queue runs the block on the main thread, and the block only sends on a channel,
    // which is safe from any thread. The returned token is leaked on purpose so the observation
    // lasts for the life of the process.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    };
    std::mem::forget(token);
    rx
}

#[cfg(not(target_os = "macos"))]
fn observe() -> async_channel::Receiver<()> {
    async_channel::unbounded().1
}
