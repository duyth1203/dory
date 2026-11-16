//! The two AppKit calls GPUI does not make for us: reading the user's accent color and posting a
//! VoiceOver announcement. Everything else in the app is platform-free.

use gpui_kit::Hsla;

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
