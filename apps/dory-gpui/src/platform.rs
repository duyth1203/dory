//! The AppKit calls GPUI does not make for us: reading the user's accent color, posting a
//! VoiceOver announcement, and, for the visual pass's launch state (`show`), forcing Dory's
//! appearance and naming a window's number. Everything else in the app is platform-free.

use gpui_kit::{Hsla, Window};

use crate::show::Appearance;

/// Forces `appearance` on `NSApp`, so every Dory window and the theme read it while the rest of
/// the Mac keeps its own. Call it before the first window opens.
#[cfg(target_os = "macos")]
pub fn force_appearance(appearance: Appearance) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: both are immutable AppKit string constants.
    let name = unsafe {
        match appearance {
            Appearance::Light => NSAppearanceNameAqua,
            Appearance::Dark => NSAppearanceNameDarkAqua,
        }
    };
    let forced = NSAppearance::appearanceNamed(name);
    NSApplication::sharedApplication(mtm).setAppearance(forced.as_deref());
    eprintln!("appearance: forced {appearance:?}");
}

#[cfg(not(target_os = "macos"))]
pub fn force_appearance(_: Appearance) {}

/// Prints `window-id: <NSWindow windowNumber>`, so `scripts/visual-pass.sh` can capture this
/// window alone with `screencapture -l`.
#[cfg(target_os = "macos")]
pub fn print_window_id(window: &Window) {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points at the window's content `NSView`, which lives as long
    // as `window`, and this runs on the main thread, where GPUI calls window code.
    let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
    if let Some(ns_window) = view.window() {
        eprintln!("window-id: {}", ns_window.windowNumber());
    }
}

#[cfg(not(target_os = "macos"))]
pub fn print_window_id(_: &Window) {}

#[cfg(target_os = "macos")]
pub fn system_accent() -> Option<Hsla> {
    use objc2_app_kit::{NSColor, NSColorSpace};
    let accent = NSColor::controlAccentColor();
    let srgb = accent.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(
        gpui_kit::Rgba {
            r: srgb.redComponent() as f32,
            g: srgb.greenComponent() as f32,
            b: srgb.blueComponent() as f32,
            a: 1.0,
        }
        .into(),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn system_accent() -> Option<Hsla> {
    None
}

/// Posts `NSAccessibilityAnnouncementRequestedNotification` on `NSApp` at high priority, so
/// VoiceOver speaks `message` even while focus is elsewhere. Off the main thread it only logs.
#[cfg(target_os = "macos")]
pub fn announce(message: &str) {
    use objc2::MainThreadMarker;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSAccessibilityAnnouncementKey, NSAccessibilityAnnouncementRequestedNotification,
        NSAccessibilityNotificationUserInfoKey, NSAccessibilityPostNotificationWithUserInfo,
        NSAccessibilityPriorityKey, NSAccessibilityPriorityLevel, NSApplication,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString};

    eprintln!("announce: {message}");
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let text = NSString::from_str(message);
    let priority = NSNumber::new_isize(NSAccessibilityPriorityLevel::High.0);
    // SAFETY: both are immutable AppKit string constants.
    let keys: [&NSAccessibilityNotificationUserInfoKey; 2] =
        unsafe { [NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey] };
    let values: [&AnyObject; 2] = [&text, &priority];
    let info = NSDictionary::from_slices(&keys, &values);
    // SAFETY: `app` is the live shared application and `info` holds the keys AppKit documents
    // for this notification.
    unsafe {
        NSAccessibilityPostNotificationWithUserInfo(
            &app,
            NSAccessibilityAnnouncementRequestedNotification,
            Some(&info),
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn announce(message: &str) {
    eprintln!("announce: {message}");
}
