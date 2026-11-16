//! System notifications through `UNUserNotificationCenter` (UX-10, UX-14).
//!
//! A finished job posts only when no window of the app is key. Clicking a finished-job
//! notification selects that recording. Authorization is requested at the first post, never at
//! launch. The center exists only for a bundled app: unbundled (`cargo run`, tests) every call is
//! a no-op, because `currentNotificationCenter` raises an exception there.

use dory_core::domain::RecordingId;
use gpui_kit::{App, Entity};

use crate::app::AppState;

const RECORDING_PREFIX: &str = "recording:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Posting again with the same identifier replaces the earlier banner.
    pub identifier: String,
    pub title: String,
    pub body: String,
}

impl Note {
    pub fn job_finished(id: &RecordingId, title: &str) -> Note {
        Note {
            identifier: format!("{RECORDING_PREFIX}{id}"),
            title: title.to_owned(),
            body: "The transcript and summary are ready.".into(),
        }
    }

    pub fn time_warning() -> Note {
        Note {
            identifier: "time-warning".into(),
            title: "Recording ends in 5 minutes".into(),
            body: "Dory stops and saves at the 2-hour limit.".into(),
        }
    }

    pub fn cap_reached() -> Note {
        Note {
            identifier: "cap-reached".into(),
            title: "Recording saved".into(),
            body: "Dory stopped at the 2-hour limit and saved your recording.".into(),
        }
    }
}

/// The recording a notification identifier names, if it is a finished-job notification.
pub fn recording_of(identifier: &str) -> Option<RecordingId> {
    RecordingId::parse(identifier.strip_prefix(RECORDING_PREFIX)?).ok()
}

/// UX-14: a notification for work the user is looking at is noise.
pub fn should_post(window_is_key: bool) -> bool {
    !window_is_key
}

fn deliver_unless_key(note: Note, window_is_key: bool, post: impl FnOnce(Note)) -> bool {
    let wanted = should_post(window_is_key);
    if wanted {
        post(note);
    }
    wanted
}

fn window_is_key(cx: &mut App) -> bool {
    cx.windows().into_iter().any(|window| {
        matches!(
            window.update(cx, |_, window, _| window.is_window_active()),
            Ok(true)
        )
    })
}

/// A job finished. Posts only when the window is not key.
pub fn job_finished(id: &RecordingId, title: &str, cx: &mut App) {
    let key = window_is_key(cx);
    deliver_unless_key(Note::job_finished(id, title), key, |note| post(&note));
}

/// Five minutes of the recording's cap remain.
pub fn time_warning(_cx: &mut App) {
    post(&Note::time_warning());
}

/// The cap stopped and saved the recording.
pub fn cap_reached(_cx: &mut App) {
    post(&Note::cap_reached());
}

/// Routes a click on a finished-job notification to selecting that recording, and requests no
/// authorization yet.
pub fn install(app: &Entity<AppState>, cx: &mut App) {
    #[cfg(target_os = "macos")]
    center::install(app, cx);
    #[cfg(not(target_os = "macos"))]
    let _ = (app, cx);
}

fn post(note: &Note) {
    #[cfg(target_os = "macos")]
    center::post(note);
    #[cfg(not(target_os = "macos"))]
    let _ = note;
}

#[cfg(target_os = "macos")]
mod center {
    use std::sync::OnceLock;

    use block2::RcBlock;
    use gpui_kit::{App, Entity, Global};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, DefinedClass, define_class};
    use objc2_foundation::{NSBundle, NSError, NSObject, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::{Note, recording_of};
    use crate::app::AppState;
    use crate::library::Selection;

    /// True for a process launched from a `.app`. Without a bundle,
    /// `UNUserNotificationCenter.currentNotificationCenter` raises an Objective-C exception.
    fn bundled() -> bool {
        static BUNDLED: OnceLock<bool> = OnceLock::new();
        *BUNDLED.get_or_init(|| {
            let bundle = NSBundle::mainBundle();
            bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
        })
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "DoryNotificationDelegate"]
        #[ivars = async_channel::Sender<String>]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                handler: &block2::DynBlock<dyn Fn()>,
            ) {
                let identifier = response.notification().request().identifier().to_string();
                let _ = self.ivars().try_send(identifier);
                handler.call(());
            }
        }
    );

    struct Installed {
        _delegate: Retained<Delegate>,
    }

    impl Global for Installed {}

    pub fn install(app: &Entity<AppState>, cx: &mut App) {
        if !bundled() {
            eprintln!("notify: not a bundled app, notifications are off");
            return;
        }
        let (clicks, inbox) = async_channel::unbounded();
        let allocated = Delegate::alloc().set_ivars(clicks);
        // SAFETY: `init` on a freshly allocated NSObject subclass whose ivars are set.
        let delegate: Retained<Delegate> = unsafe { objc2::msg_send![super(allocated), init] };
        UNUserNotificationCenter::currentNotificationCenter()
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The center keeps its delegate weakly, so the global keeps it alive.
        cx.set_global(Installed {
            _delegate: delegate,
        });
        let app = app.clone();
        cx.spawn(async move |cx| {
            while let Ok(identifier) = inbox.recv().await {
                cx.update(|cx| {
                    cx.activate(true);
                    if let Some(id) = recording_of(&identifier) {
                        app.update(cx, |app, cx| app.select(Selection::One(id), cx));
                    }
                });
            }
        })
        .detach();
    }

    pub fn post(note: &Note) {
        if !bundled() {
            return;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&note.title));
        content.setBody(&NSString::from_str(&note.body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&note.identifier),
            &content,
            None,
        );
        // The first call shows the system prompt and later calls answer from the saved choice,
        // so asking at every post needs no state here.
        // SAFETY: UserNotifications documents its center and requests as thread-safe, and the
        // completion handler only touches the retained request and the shared center.
        let on_answer = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            if granted.as_bool() {
                UNUserNotificationCenter::currentNotificationCenter()
                    .addNotificationRequest_withCompletionHandler(&request, None);
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &on_answer,
            );
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn id() -> RecordingId {
        RecordingId::parse("2026-11-16T093000-ab12").unwrap()
    }

    #[test]
    fn a_finished_job_posts_only_when_no_window_is_key() {
        for (key, posts) in [(false, true), (true, false)] {
            let posted = RefCell::new(Vec::new());
            let note = Note::job_finished(&id(), "Design review");
            let did = deliver_unless_key(note.clone(), key, |n| posted.borrow_mut().push(n));
            assert_eq!(did, posts, "key={key}");
            assert_eq!(*posted.borrow(), if posts { vec![note] } else { vec![] });
        }
    }

    #[test]
    fn the_identifier_names_the_recording_a_click_selects() {
        let note = Note::job_finished(&id(), "Design review");
        assert_eq!(recording_of(&note.identifier), Some(id()));
        assert_eq!(recording_of(&Note::time_warning().identifier), None);
        assert_eq!(recording_of("recording:"), None);
    }

    #[test]
    fn posting_without_a_bundle_does_nothing_and_does_not_crash() {
        post(&Note::cap_reached());
        post(&Note::job_finished(&id(), "Design review"));
    }
}
