//! The four macOS display settings Dory follows (A11Y-5, A11Y-6): Reduce motion, Reduce
//! transparency, Increase contrast, and Differentiate without color, from `NSWorkspace`, read at
//! launch and again on every `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`, and
//! the accent color from System Settings › Appearance, again on every
//! `NSSystemColorsDidChangeNotification`.
//!
//! Reduce motion goes to GPUI's `set_reduce_motion`, which the level meter, the docked player's
//! fade, and Kit's own transitions read. Reduce transparency is kept for the scrim, which goes
//! opaque (`surface`, `theme::sync`). The other two and the accent rebuild the theme through
//! `theme::set_display`. All of the app's AppKit observation lives here.

use gpui_kit::{App, AsyncApp, Global};

use crate::theme;

#[derive(Default)]
struct Transparency {
    reduce: bool,
}

impl Global for Transparency {}

fn set_reduce_transparency(reduce: bool, cx: &mut App) {
    if cx.try_global::<Transparency>().map(|t| t.reduce) != Some(reduce) {
        cx.set_global(Transparency { reduce });
    }
}

/// macOS Reduce Transparency, as [`apply`] last reported it.
pub fn reduces_transparency(cx: &App) -> bool {
    cx.try_global::<Transparency>().is_some_and(|t| t.reduce)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemPrefs {
    /// `0xRRGGBB`, or `None` under Multicolor ([`theme::Display::accent`]).
    pub accent: Option<u32>,
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub differentiate_without_color: bool,
}

/// Applies `prefs` to GPUI and the theme. Tests call it to stand in for a
/// change in System Settings.
pub fn apply(prefs: SystemPrefs, cx: &mut App) {
    if cx.reduce_motion() != prefs.reduce_motion {
        cx.set_reduce_motion(prefs.reduce_motion);
    }
    set_reduce_transparency(prefs.reduce_transparency, cx);
    theme::set_display(
        |display| {
            display.accent = prefs.accent;
            display.increase_contrast = prefs.increase_contrast;
            display.differentiate_without_color = prefs.differentiate_without_color;
        },
        cx,
    );
    eprintln!(
        "system-prefs: accent {}, reduce motion {}, reduce transparency {}, increase contrast {}, differentiate without color {}",
        prefs
            .accent
            .map_or("multicolor".to_owned(), |hex| format!("#{hex:06X}")),
        prefs.reduce_motion,
        prefs.reduce_transparency,
        prefs.increase_contrast,
        prefs.differentiate_without_color
    );
}

/// Reads the settings now, applies them, and follows every later change for the life of the
/// app. Called once by `main`. `pretend` (`DORY_SYSTEM_PREFS`) replaces the Mac's settings, and
/// then nothing is followed.
pub fn install(pretend: Option<SystemPrefs>, cx: &mut App) {
    if let Some(prefs) = pretend {
        apply(prefs, cx);
        return;
    }
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
        accent: chosen_accent(),
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        reduce_transparency: workspace.accessibilityDisplayShouldReduceTransparency(),
        increase_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
        differentiate_without_color: workspace
            .accessibilityDisplayShouldDifferentiateWithoutColor(),
    }
}

/// The accent picked in System Settings, or `None` under Multicolor. Multicolor leaves
/// `AppleAccentColor` unset, and `controlAccentColor` then reports plain blue, the same as
/// choosing Blue, so only the default tells the two apart.
#[cfg(target_os = "macos")]
fn chosen_accent() -> Option<u32> {
    use objc2_app_kit::{NSColor, NSColorSpace};
    use objc2_foundation::{NSUserDefaults, ns_string};

    NSUserDefaults::standardUserDefaults().objectForKey(ns_string!("AppleAccentColor"))?;
    let srgb =
        NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    Some(
        byte(srgb.redComponent()) << 16
            | byte(srgb.greenComponent()) << 8
            | byte(srgb.blueComponent()),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn read() -> SystemPrefs {
    SystemPrefs::default()
}

/// A channel that receives one message per display-options or system-colors change. The
/// observers are never removed: the app follows the settings until it quits.
#[cfg(target_os = "macos")]
fn observe() -> async_channel::Receiver<()> {
    use block2::RcBlock;
    use objc2_app_kit::{
        NSSystemColorsDidChangeNotification, NSWorkspace,
        NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
    };
    use objc2_foundation::{
        NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue,
    };

    let (tx, rx) = async_channel::unbounded();
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
        let _ = tx.try_send(());
    });
    let follow = |center: &NSNotificationCenter, name: &NSNotificationName| {
        // SAFETY: the object filter is nil, the main queue runs the block on the main thread,
        // and the block only sends on a channel, which is safe from any thread. The returned
        // token is leaked on purpose so the observation lasts for the life of the process.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        std::mem::forget(token);
    };
    // SAFETY: both names are AppKit's immutable notification constants.
    let (display_options, system_colors) = unsafe {
        (
            NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
            NSSystemColorsDidChangeNotification,
        )
    };
    follow(
        &NSWorkspace::sharedWorkspace().notificationCenter(),
        display_options,
    );
    follow(&NSNotificationCenter::defaultCenter(), system_colors);
    rx
}

#[cfg(not(target_os = "macos"))]
fn observe() -> async_channel::Receiver<()> {
    async_channel::unbounded().1
}
