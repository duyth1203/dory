//! System notifications through `UNUserNotificationCenter` (UX-10, UX-14).
//!
//! Dory posts when its window can't tell you itself, and each kind of notification is a setting
//! in Settings › General › Notifications ([`NoteKind`]). Whether one posts is [`should_post`], a
//! pure function of its kind, the main window's state, and those settings. Clicking a
//! notification about a recording opens it, reopening the window if it was closed. Authorization
//! is requested at the first post, never at launch. The center exists only for a bundled app:
//! unbundled (`cargo run`, tests) every call is a no-op, because `currentNotificationCenter`
//! raises an exception there.

use dory_core::domain::RecordingId;
use gpui_kit::{App, Entity, Global};

use crate::app::AppState;
use crate::prefs::Notifications;

const RECORDING_PREFIX: &str = "recording:";
const SENT_KEPT: usize = 32;

/// The System Settings page of Dory's notifications.
pub const SYSTEM_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.dory.app";

/// Each kind of notification, one setting each (UX-14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NoteKind {
    Recording,
    TimeLimit,
    Transcript,
    Summary,
    Attention,
    Download,
}

/// When a kind posts, by the main window's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    WindowClosed,
    WindowNotKey,
    Always,
}

impl NoteKind {
    pub const ALL: [NoteKind; 6] = [
        NoteKind::Recording,
        NoteKind::TimeLimit,
        NoteKind::Transcript,
        NoteKind::Summary,
        NoteKind::Attention,
        NoteKind::Download,
    ];

    /// The setting's label, its one-line effect, its stored slug, and when it posts.
    const fn row(self) -> (&'static str, &'static str, &'static str, When) {
        match self {
            NoteKind::Recording => (
                "Recording started and saved",
                "When a recording starts or is saved while Dory's window is closed.",
                "recording",
                When::WindowClosed,
            ),
            NoteKind::TimeLimit => (
                "Recording time limit",
                "Five minutes before the 2-hour limit, and when Dory saves at the limit.",
                "time-limit",
                When::Always,
            ),
            NoteKind::Transcript => (
                "Transcript ready",
                "When a transcript is ready and Dory's window isn't in front.",
                "transcript",
                When::WindowNotKey,
            ),
            NoteKind::Summary => (
                "Summary ready",
                "When a summary is ready and Dory's window isn't in front.",
                "summary",
                When::WindowNotKey,
            ),
            NoteKind::Attention => (
                "Needs attention",
                "When a recording needs a fix before Dory can finish it, with what to do.",
                "attention",
                When::WindowNotKey,
            ),
            NoteKind::Download => (
                "Model downloaded",
                "When a speech or summary model finishes downloading and Dory's window isn't in front.",
                "download",
                When::WindowNotKey,
            ),
        }
    }

    pub fn label(self) -> &'static str {
        self.row().0
    }

    pub fn effect(self) -> &'static str {
        self.row().1
    }

    pub fn slug(self) -> &'static str {
        self.row().2
    }

    pub fn parse(slug: &str) -> Option<NoteKind> {
        NoteKind::ALL.into_iter().find(|kind| kind.slug() == slug)
    }
}

/// The main window, as a notification sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Closed,
    /// Open, but another app or another Dory window is in front.
    Behind,
    Key,
}

/// UX-14: a notification for something the window already shows is noise, and each kind can be
/// turned off.
pub fn should_post(kind: NoteKind, window: WindowState, settings: &Notifications) -> bool {
    let when = match kind.row().3 {
        When::WindowClosed => window == WindowState::Closed,
        When::WindowNotKey => window != WindowState::Key,
        When::Always => true,
    };
    when && settings.allows(kind)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Posting again with the same identifier replaces the earlier banner.
    pub identifier: String,
    pub title: String,
    pub body: String,
}

impl Note {
    fn about(id: &RecordingId, title: impl Into<String>, body: impl Into<String>) -> Note {
        Note {
            identifier: format!("{RECORDING_PREFIX}{id}"),
            title: title.into(),
            body: body.into(),
        }
    }

    pub fn recording_started(id: &RecordingId, title: &str) -> Note {
        Note::about(
            id,
            "Dory is recording",
            format!("{title}. Stop it from the menu bar."),
        )
    }

    pub fn recording_saved(id: &RecordingId, title: &str) -> Note {
        Note::about(id, "Recording saved", format!("{title} is saved."))
    }

    pub fn transcript_ready(id: &RecordingId, title: &str) -> Note {
        Note::about(id, title, "The transcript is ready.")
    }

    pub fn summary_ready(id: &RecordingId, title: &str) -> Note {
        Note::about(id, title, "The transcript and summary are ready.")
    }

    /// `cause` is UX-13's one sentence.
    pub fn needs_attention(id: &RecordingId, title: &str, cause: &str) -> Note {
        Note::about(id, format!("{title} needs attention"), cause)
    }

    pub fn model_downloaded(name: &str) -> Note {
        Note {
            identifier: format!("download:{name}"),
            title: format!("{name} is downloaded"),
            body: "Dory can use it now.".into(),
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

/// The recording a notification identifier names, if it is about one.
pub fn recording_of(identifier: &str) -> Option<RecordingId> {
    RecordingId::parse(identifier.strip_prefix(RECORDING_PREFIX)?).ok()
}

pub fn window_state(cx: &App) -> WindowState {
    match crate::main_window(cx) {
        None => WindowState::Closed,
        Some(window) if cx.active_window() == Some(window) => WindowState::Key,
        Some(_) => WindowState::Behind,
    }
}

/// Posts `note` when [`should_post`] allows it. True when it posted.
pub fn send(kind: NoteKind, note: Note, settings: &Notifications, cx: &mut App) -> bool {
    let wanted = should_post(kind, window_state(cx), settings);
    if wanted {
        let sent = &mut cx.default_global::<Sent>().0;
        if sent.len() == SENT_KEPT {
            sent.remove(0);
        }
        sent.push((kind, note.clone()));
        post(&note);
    }
    wanted
}

/// The latest notes [`send`] posted, for the tests: unbundled, nothing reaches the system.
#[derive(Default)]
pub struct Sent(pub Vec<(NoteKind, Note)>);

impl Global for Sent {}

/// Whether macOS lets Dory post, as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    /// Not read yet, not asked yet, or not a bundled app.
    #[default]
    Unknown,
    Allowed,
    /// Turned off for Dory in System Settings.
    Off,
}

#[derive(Default)]
struct Known(Access);

impl Global for Known {}

pub fn access(cx: &App) -> Access {
    cx.try_global::<Known>()
        .map_or(Access::Unknown, |known| known.0)
}

/// Sets the access Settings shows, for the tests and for the answer from macOS.
pub fn set_access(access: Access, cx: &mut App) {
    cx.set_global(Known(access));
    cx.refresh_windows();
}

/// Reads the access again: when Settings opens, since the user may have just changed it.
pub fn refresh_access(cx: &mut App) {
    #[cfg(target_os = "macos")]
    center::refresh_access(cx);
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// Routes a click on a notification about a recording to opening it, and requests no
/// authorization yet.
pub fn install(app: &Entity<AppState>, cx: &mut App) {
    #[cfg(target_os = "macos")]
    center::install(app, cx);
    #[cfg(not(target_os = "macos"))]
    let _ = (app, cx);
}

/// Opens the recording a notification named, reopening the main window if it was closed.
pub fn open(app: &Entity<AppState>, identifier: &str, cx: &mut App) {
    crate::show_main_window(cx);
    if let Some(id) = recording_of(identifier) {
        app.update(cx, |app, cx| {
            app.select(crate::library::Selection::One(id), cx)
        });
    }
}

fn post(note: &Note) {
    #[cfg(target_os = "macos")]
    center::post(note);
    #[cfg(not(target_os = "macos"))]
    let _ = note;
}

#[cfg(target_os = "macos")]
mod center {
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use block2::RcBlock;
    use gpui_kit::{App, Entity, Global};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, DefinedClass, define_class};
    use objc2_foundation::{NSBundle, NSError, NSObject, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotification, UNNotificationPresentationOptions, UNNotificationRequest,
        UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::{Access, Note};
    use crate::app::AppState;

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
                cx.update(|cx| super::open(&app, &identifier, cx));
            }
        })
        .detach();
    }

    pub fn refresh_access(cx: &mut App) {
        if !bundled() {
            return;
        }
        let (answer, inbox) = async_channel::bounded(1);
        // SAFETY: UserNotifications calls the handler once on its own queue with live settings,
        // and the handler only reads them and sends a plain value.
        let on_settings = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let access = match status {
                UNAuthorizationStatus::Denied => Access::Off,
                UNAuthorizationStatus::NotDetermined => Access::Unknown,
                _ => Access::Allowed,
            };
            let _ = answer.try_send(access);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&on_settings);
        cx.spawn(async move |cx| {
            if let Ok(access) = inbox.recv().await {
                cx.update(|cx| super::set_access(access, cx));
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
    use super::*;

    fn id() -> RecordingId {
        RecordingId::parse("2026-11-16T093000-ab12").unwrap()
    }

    #[test]
    fn each_kind_posts_only_in_its_window_states() {
        use WindowState::{Behind, Closed, Key};
        let all_on = Notifications::default();
        let table = [
            (NoteKind::Recording, [true, false, false]),
            (NoteKind::TimeLimit, [true, true, true]),
            (NoteKind::Transcript, [true, true, false]),
            (NoteKind::Summary, [true, true, false]),
            (NoteKind::Attention, [true, true, false]),
            (NoteKind::Download, [true, true, false]),
        ];
        assert_eq!(table.len(), NoteKind::ALL.len());
        for (kind, posts) in table {
            for (window, posts) in [Closed, Behind, Key].into_iter().zip(posts) {
                assert_eq!(
                    should_post(kind, window, &all_on),
                    posts,
                    "{kind:?} with the window {window:?}"
                );
            }
        }
    }

    #[test]
    fn a_kind_turned_off_never_posts_and_leaves_the_others_on() {
        for off in NoteKind::ALL {
            let mut settings = Notifications::default();
            settings.set(off, false);
            for kind in NoteKind::ALL {
                assert_eq!(
                    should_post(kind, WindowState::Closed, &settings),
                    kind != off,
                    "{kind:?} with {off:?} off"
                );
            }
        }
    }

    #[test]
    fn every_kind_has_a_label_an_effect_and_a_slug_that_parses_back() {
        for kind in NoteKind::ALL {
            assert!(!kind.label().is_empty() && kind.effect().ends_with('.'));
            assert_eq!(NoteKind::parse(kind.slug()), Some(kind));
        }
        assert_eq!(NoteKind::parse("nope"), None);
    }

    #[test]
    fn notes_about_a_recording_name_it_for_the_click() {
        for note in [
            Note::recording_started(&id(), "Design review"),
            Note::recording_saved(&id(), "Design review"),
            Note::transcript_ready(&id(), "Design review"),
            Note::summary_ready(&id(), "Design review"),
            Note::needs_attention(&id(), "Design review", "Summaries aren't set up."),
        ] {
            assert_eq!(recording_of(&note.identifier), Some(id()), "{note:?}");
        }
        assert_eq!(recording_of(&Note::time_warning().identifier), None);
        assert_eq!(
            recording_of(&Note::model_downloaded("Parakeet").identifier),
            None
        );
        assert_eq!(recording_of("recording:"), None);
    }

    #[test]
    fn needs_attention_carries_the_one_sentence_cause() {
        let note = Note::needs_attention(&id(), "Hiring debrief", "Summaries aren't set up yet.");
        assert_eq!(note.title, "Hiring debrief needs attention");
        assert_eq!(note.body, "Summaries aren't set up yet.");
    }

    #[test]
    fn posting_without_a_bundle_does_nothing_and_does_not_crash() {
        post(&Note::cap_reached());
        post(&Note::summary_ready(&id(), "Design review"));
    }
}
