//! The menu-bar extra (P5): "Dory" at rest and "● 12:34" while recording, with Start Recording
//! or Stop Recording and Show Dory in its menu. GPUI has no tray API, so the `NSStatusItem` is
//! built here with `objc2-app-kit`, ported from the P0 spike.
//!
//! The title and the menu wording are pure functions of the recorder status, so they are tested
//! without AppKit. The recorder's status event fires on a phase change only, so [`Clock`] adds
//! the wall time since the phase began, and a one-second tick runs only while recording.

use std::time::Instant;

use dory_core::app::RecorderStatus;
use dory_core::domain::Ms;
use gpui_kit::{App, Entity};

use crate::a11y;
use crate::app::AppState;

pub const IDLE_TITLE: &str = "Dory";

/// "Dory", "● 12:34" while recording, "Paused 12:34" while paused. Stopping a take leaves the
/// grace period, which is no longer a recording, so it reads "Dory".
pub fn title(status: &RecorderStatus, elapsed: Ms) -> String {
    match status {
        RecorderStatus::Recording { .. } => format!("● {}", a11y::clock(elapsed)),
        RecorderStatus::Paused { .. } => format!("Paused {}", a11y::clock(elapsed)),
        RecorderStatus::Idle | RecorderStatus::Grace { .. } => IDLE_TITLE.to_owned(),
    }
}

/// What VoiceOver reads for the menu-bar item (A11Y-1): the app's name and the recorder's state
/// in words, where the title has only a glyph and a clock.
pub fn spoken(status: &RecorderStatus, elapsed: Ms) -> String {
    match status {
        RecorderStatus::Recording { .. } => {
            format!("Dory, recording, {}", a11y::spoken_duration(elapsed))
        }
        RecorderStatus::Paused { .. } => {
            format!("Dory, paused, {}", a11y::spoken_duration(elapsed))
        }
        RecorderStatus::Idle | RecorderStatus::Grace { .. } => IDLE_TITLE.to_owned(),
    }
}

pub fn is_recording(status: &RecorderStatus) -> bool {
    matches!(
        status,
        RecorderStatus::Recording { .. } | RecorderStatus::Paused { .. }
    )
}

pub fn toggle_label(status: &RecorderStatus) -> &'static str {
    if is_recording(status) {
        "Stop Recording"
    } else {
        "Start Recording"
    }
}

/// Turns the status event's `recorded` into a live elapsed time between events.
#[derive(Default)]
pub struct Clock {
    seen: Option<(RecorderStatus, Instant)>,
}

impl Clock {
    pub fn elapsed(&mut self, status: &RecorderStatus, now: Instant) -> Ms {
        let (RecorderStatus::Recording { recorded, .. }
        | RecorderStatus::Paused { recorded, .. }
        | RecorderStatus::Grace { recorded, .. }) = status
        else {
            self.seen = None;
            return Ms(0);
        };
        let since = match &self.seen {
            Some((seen, at)) if seen == status => *at,
            _ => {
                self.seen = Some((status.clone(), now));
                now
            }
        };
        match status {
            RecorderStatus::Recording { .. } => {
                let live = now.saturating_duration_since(since).as_millis() as u64;
                Ms(recorded.0.saturating_add(live))
            }
            _ => *recorded,
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &Entity<AppState>, _cx: &mut App) {}

#[cfg(target_os = "macos")]
pub fn install(app: &Entity<AppState>, cx: &mut App) {
    appkit::install(app, cx);
}

#[cfg(target_os = "macos")]
mod appkit {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use gpui_kit::{App, Entity, Task};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAccessibility as _, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
        NSVariableStatusItemLength,
    };
    use objc2_foundation::{NSObject, NSString, ns_string};

    use super::{Clock, IDLE_TITLE, is_recording, spoken, title, toggle_label};
    use crate::actions::{NewRecording, StopRecording};
    use crate::app::AppState;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Command {
        Toggle,
        Show,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "DoryStatusMenuTarget"]
        #[ivars = async_channel::Sender<Command>]
        struct MenuTarget;

        impl MenuTarget {
            #[unsafe(method(toggleRecording:))]
            fn toggle_recording(&self, _sender: Option<&AnyObject>) {
                let _ = self.ivars().try_send(Command::Toggle);
            }

            #[unsafe(method(showWindow:))]
            fn show_window(&self, _sender: Option<&AnyObject>) {
                let _ = self.ivars().try_send(Command::Show);
            }
        }
    );

    impl MenuTarget {
        fn new(mtm: MainThreadMarker, commands: async_channel::Sender<Command>) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(commands);
            // SAFETY: `init` on an allocated NSObject subclass with its ivars set.
            unsafe { msg_send![super(this), init] }
        }
    }

    struct StatusItem {
        item: Retained<NSStatusItem>,
        toggle: Retained<NSMenuItem>,
        mtm: MainThreadMarker,
        _menu: Retained<NSMenu>,
        _target: Retained<MenuTarget>,
    }

    impl StatusItem {
        fn new(mtm: MainThreadMarker, commands: async_channel::Sender<Command>) -> Self {
            let item =
                NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
            let target = MenuTarget::new(mtm, commands);
            let menu = NSMenu::new(mtm);
            let entry = |title: &NSString, action| {
                // SAFETY: `action` is a selector `MenuTarget` implements, and the entry's target
                // is that same live object, which `StatusItem` retains for the menu's lifetime.
                let entry = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        title,
                        Some(action),
                        ns_string!(""),
                    )
                };
                // SAFETY: as above, and the target outlives the entry.
                unsafe { entry.setTarget(Some(&target)) };
                menu.addItem(&entry);
                entry
            };
            let toggle = entry(ns_string!("Start Recording"), sel!(toggleRecording:));
            entry(ns_string!("Show Dory"), sel!(showWindow:));
            item.setMenu(Some(&menu));
            Self {
                item,
                toggle,
                mtm,
                _menu: menu,
                _target: target,
            }
        }

        fn set(&self, title: &str, spoken: &str, toggle: &str) {
            if let Some(button) = self.item.button(self.mtm) {
                button.setTitle(&NSString::from_str(title));
                button.setAccessibilityLabel(Some(&NSString::from_str(spoken)));
            }
            self.toggle.setTitle(&NSString::from_str(toggle));
        }
    }

    struct Shared {
        item: StatusItem,
        clock: Clock,
        tick: Option<Task<()>>,
    }

    fn refresh(app: &Entity<AppState>, shared: &Rc<RefCell<Shared>>, cx: &mut App) {
        let status = app.read(cx).recorder().clone();
        let mut shared = shared.borrow_mut();
        let elapsed = shared.clock.elapsed(&status, Instant::now());
        shared.item.set(
            &title(&status, elapsed),
            &spoken(&status, elapsed),
            toggle_label(&status),
        );
    }

    fn dispatch(action: &dyn gpui_kit::Action, cx: &mut App) {
        // Not `cx.dispatch_action`: with the app in the background there is no active window,
        // and the action would reach only global handlers, never the shell's.
        if let Some(window) = cx.windows().first().copied() {
            let _ = window.update(cx, |_, window, cx| {
                window.dispatch_action(action.boxed_clone(), cx)
            });
        }
    }

    pub fn install(app: &Entity<AppState>, cx: &mut App) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let (commands, inbox) = async_channel::unbounded();
        let shared = Rc::new(RefCell::new(Shared {
            item: StatusItem::new(mtm, commands),
            clock: Clock::default(),
            tick: None,
        }));
        shared
            .borrow()
            .item
            .set(IDLE_TITLE, IDLE_TITLE, "Start Recording");
        eprintln!("status-item: created");
        refresh(app, &shared, cx);

        cx.observe(app, {
            let shared = shared.clone();
            move |app, cx| {
                refresh(&app, &shared, cx);
                let recording = is_recording(app.read(cx).recorder());
                let mut state = shared.borrow_mut();
                if !recording {
                    state.tick = None;
                } else if state.tick.is_none() {
                    let app = app.downgrade();
                    let shared = shared.clone();
                    state.tick = Some(cx.spawn(async move |cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_secs(1)).await;
                            let Some(app) = app.upgrade() else { break };
                            cx.update(|cx| refresh(&app, &shared, cx));
                        }
                    }));
                }
            }
        })
        .detach();

        let app = app.clone();
        cx.spawn(async move |cx| {
            while let Ok(command) = inbox.recv().await {
                cx.update(|cx| match command {
                    Command::Toggle => {
                        if is_recording(app.read(cx).recorder()) {
                            dispatch(&StopRecording, cx);
                        } else {
                            dispatch(&NewRecording, cx);
                        }
                    }
                    Command::Show => {
                        cx.activate(true);
                        if let Some(window) = cx.windows().first().copied() {
                            let _ = window.update(cx, |_, window, _| window.activate_window());
                        }
                    }
                });
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use dory_core::domain::RecordingId;

    use super::*;

    fn id() -> RecordingId {
        RecordingId::parse("r1").unwrap()
    }

    #[test]
    fn the_title_is_dory_at_rest_and_a_dot_with_the_clock_while_recording() {
        let recording = RecorderStatus::Recording {
            id: id(),
            recorded: Ms(0),
        };
        let paused = RecorderStatus::Paused {
            id: id(),
            recorded: Ms(0),
        };
        let grace = RecorderStatus::Grace {
            id: id(),
            recorded: Ms(0),
        };
        let cases = [
            (RecorderStatus::Idle, Ms(0), "Dory"),
            (recording.clone(), Ms(754_000), "● 12:34"),
            (recording, Ms(3_723_000), "● 1:02:03"),
            (paused, Ms(65_000), "Paused 1:05"),
            (grace, Ms(65_000), "Dory"),
        ];
        for (status, elapsed, want) in cases {
            assert_eq!(title(&status, elapsed), want, "{status:?}");
        }
    }

    #[test]
    fn the_menu_offers_stop_while_recording_or_paused_and_start_otherwise() {
        let live = |recorded| RecorderStatus::Recording { id: id(), recorded };
        assert_eq!(toggle_label(&RecorderStatus::Idle), "Start Recording");
        assert_eq!(toggle_label(&live(Ms(0))), "Stop Recording");
        assert_eq!(
            toggle_label(&RecorderStatus::Paused {
                id: id(),
                recorded: Ms(0)
            }),
            "Stop Recording"
        );
        assert_eq!(
            toggle_label(&RecorderStatus::Grace {
                id: id(),
                recorded: Ms(0)
            }),
            "Start Recording"
        );
    }

    #[test]
    fn the_clock_runs_between_events_while_recording_and_freezes_otherwise() {
        let start = Instant::now();
        let mut clock = Clock::default();
        let recording = RecorderStatus::Recording {
            id: id(),
            recorded: Ms(10_000),
        };
        assert_eq!(clock.elapsed(&recording, start), Ms(10_000));
        let later = start + Duration::from_secs(5);
        assert_eq!(clock.elapsed(&recording, later), Ms(15_000));
        let paused = RecorderStatus::Paused {
            id: id(),
            recorded: Ms(15_000),
        };
        assert_eq!(
            clock.elapsed(&paused, later + Duration::from_secs(30)),
            Ms(15_000)
        );
        assert_eq!(
            clock.elapsed(&RecorderStatus::Idle, later),
            Ms(0),
            "idle resets"
        );
    }
}
