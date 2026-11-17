//! The menu-bar extra (UX-8): "Dory" at rest, "Dory ·" while recordings are processed, and
//! "● 12:34" while recording. Its menu starts with what is in progress, then Start or Stop
//! Recording, Pause or Resume, Show Dory, Processing Queue…, and Quit Dory. It records without
//! opening the window, and keeps working with the window closed. GPUI has no tray API, so the
//! `NSStatusItem` is built here with `objc2-app-kit`, and all of its `unsafe` stays in this file.
//!
//! The title, the menu's words, and what each command does are plain functions, tested without
//! AppKit. The recorder's status event fires on a phase change only, so [`Clock`] adds the wall
//! time since the phase began, and a one-second tick runs only while recording.

use std::time::Instant;

use dory_core::app::RecorderStatus;
use dory_core::domain::{Ms, Stage};
use gpui_kit::{App, Entity};

use crate::a11y;
use crate::app::AppState;
use crate::in_progress::{InProgress, TakeState};

pub const IDLE_TITLE: &str = "Dory";
pub const BUSY_TITLE: &str = "Dory ·";

/// "Dory", "Dory ·" while recordings are processed, "● 12:34" while recording, "Paused 12:34"
/// while paused. Stopping a take leaves the grace period, which is no longer a recording.
pub fn title(status: &RecorderStatus, elapsed: Ms, busy: bool) -> String {
    match status {
        RecorderStatus::Recording { .. } => format!("● {}", a11y::clock(elapsed)),
        RecorderStatus::Paused { .. } => format!("Paused {}", a11y::clock(elapsed)),
        RecorderStatus::Idle | RecorderStatus::Grace { .. } if busy => BUSY_TITLE.to_owned(),
        RecorderStatus::Idle | RecorderStatus::Grace { .. } => IDLE_TITLE.to_owned(),
    }
}

/// What VoiceOver reads for the menu-bar item (A11Y-1): the app's name and its state in words,
/// where the title has only a glyph and a clock.
pub fn spoken(status: &RecorderStatus, elapsed: Ms, busy: bool) -> String {
    match status {
        RecorderStatus::Recording { .. } => {
            format!("Dory, recording, {}", a11y::spoken_duration(elapsed))
        }
        RecorderStatus::Paused { .. } => {
            format!("Dory, paused, {}", a11y::spoken_duration(elapsed))
        }
        RecorderStatus::Idle | RecorderStatus::Grace { .. } if busy => "Dory, processing".into(),
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

/// Pause or Resume, only while a take is live.
pub fn pause_label(status: &RecorderStatus) -> Option<&'static str> {
    match status {
        RecorderStatus::Recording { .. } => Some("Pause Recording"),
        RecorderStatus::Paused { .. } => Some("Resume Recording"),
        RecorderStatus::Idle | RecorderStatus::Grace { .. } => None,
    }
}

/// One informational line at the top of the menu: what it shows, and what VoiceOver reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub title: String,
    pub spoken: String,
}

/// The menu's informational lines, from the same model the quit confirmation reads. Nothing in
/// progress gives no lines.
pub fn lines(state: &InProgress) -> Vec<Line> {
    let mut lines = Vec::new();
    if let Some(take) = &state.take {
        let (title, at) = (&take.title, a11y::clock(take.elapsed));
        let spoken_at = a11y::spoken_duration(take.elapsed);
        lines.push(match take.state {
            TakeState::Recording => Line {
                title: format!("Recording “{title}” · {at}"),
                spoken: format!("Recording {title}, {spoken_at}"),
            },
            TakeState::Paused => Line {
                title: format!("Paused “{title}” at {at}"),
                spoken: format!("Paused {title} at {spoken_at}"),
            },
            TakeState::Stopped => Line {
                title: format!("Stopped “{title}” at {at}"),
                spoken: format!("Stopped {title} at {spoken_at}"),
            },
        });
    }
    if let Some(running) = &state.running {
        let verb = match running.stage {
            Stage::Transcribe => "Transcribing",
            Stage::Summarize => "Summarizing",
        };
        let title = &running.title;
        lines.push(match running.percent {
            Some(percent) => Line {
                title: format!("{verb} “{title}” · {percent}%"),
                spoken: format!("{verb} {title}, {percent} percent"),
            },
            None => Line {
                title: format!("{verb} “{title}”"),
                spoken: format!("{verb} {title}"),
            },
        });
    }
    if state.queued > 0 {
        let text = match (state.running.is_some(), state.queued) {
            (true, n) => format!("{n} more waiting"),
            (false, 1) => "1 recording waiting".to_owned(),
            (false, n) => format!("{n} recordings waiting"),
        };
        lines.push(Line {
            title: text.clone(),
            spoken: text,
        });
    }
    for download in &state.downloads {
        let name = &download.name;
        lines.push(match download.percent {
            Some(percent) => Line {
                title: format!("Downloading {name} · {percent}%"),
                spoken: format!("Downloading {name}, {percent} percent"),
            },
            None => Line {
                title: format!("Downloading {name}"),
                spoken: format!("Downloading {name}"),
            },
        });
    }
    lines
}

/// What the menu shows, worked out in full so an unchanged menu is not rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub spoken: String,
    pub lines: Vec<Line>,
    pub toggle: &'static str,
    pub pause: Option<&'static str>,
}

impl Menu {
    pub fn of(status: &RecorderStatus, elapsed: Ms, state: &InProgress) -> Menu {
        let busy = state.is_processing();
        Menu {
            title: title(status, elapsed, busy),
            spoken: spoken(status, elapsed, busy),
            lines: lines(state),
            toggle: toggle_label(status),
            pause: pause_label(status),
        }
    }
}

/// A menu command. None of them opens the window except Show Dory, Processing Queue…, and a
/// Quit that has to ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Pause,
    Show,
    Queue,
    Quit,
}

pub fn run(command: Command, app: &Entity<AppState>, cx: &mut App) {
    match command {
        Command::Toggle => app.update(cx, |app, cx| {
            if is_recording(app.recorder()) {
                app.stop_recording(cx);
            } else {
                app.start_recording(cx);
            }
        }),
        Command::Pause => app.update(cx, |app, cx| app.toggle_pause(cx)),
        Command::Show => {
            crate::show_main_window(cx);
        }
        Command::Queue => crate::processing_window::open(app, cx),
        Command::Quit => crate::quit::request(app, cx),
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
    use objc2::runtime::{AnyObject, Sel};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSAccessibility as _, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
        NSVariableStatusItemLength,
    };
    use objc2_foundation::{NSObject, NSString, ns_string};

    use super::{Clock, Command, IDLE_TITLE, Menu, is_recording, run};
    use crate::app::AppState;

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

            #[unsafe(method(pauseRecording:))]
            fn pause_recording(&self, _sender: Option<&AnyObject>) {
                let _ = self.ivars().try_send(Command::Pause);
            }

            #[unsafe(method(showWindow:))]
            fn show_window(&self, _sender: Option<&AnyObject>) {
                let _ = self.ivars().try_send(Command::Show);
            }

            #[unsafe(method(showQueue:))]
            fn show_queue(&self, _sender: Option<&AnyObject>) {
                let _ = self.ivars().try_send(Command::Queue);
            }

            #[unsafe(method(quitDory:))]
            fn quit_dory(&self, _sender: Option<&AnyObject>) {
                let _ = self.ivars().try_send(Command::Quit);
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
        menu: Retained<NSMenu>,
        target: Retained<MenuTarget>,
        mtm: MainThreadMarker,
        shown: Option<Menu>,
    }

    impl StatusItem {
        fn new(mtm: MainThreadMarker, commands: async_channel::Sender<Command>) -> Self {
            let item =
                NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
            let menu = NSMenu::new(mtm);
            // The informational lines are disabled by hand; AppKit would enable any item whose
            // target answers its action.
            menu.setAutoenablesItems(false);
            item.setMenu(Some(&menu));
            Self {
                item,
                menu,
                target: MenuTarget::new(mtm, commands),
                mtm,
                shown: None,
            }
        }

        fn add(&self, title: &str, spoken: &str, action: Option<Sel>) {
            let title = NSString::from_str(title);
            // SAFETY: `action` is `None` or a selector `MenuTarget` implements, and the entry's
            // target is that same object, which `StatusItem` retains as long as the menu.
            let entry = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &title,
                    action,
                    ns_string!(""),
                )
            };
            if action.is_some() {
                // SAFETY: as above, the target outlives the entry.
                unsafe { entry.setTarget(Some(&self.target)) };
            } else {
                entry.setEnabled(false);
            }
            entry.setAccessibilityLabel(Some(&NSString::from_str(spoken)));
            self.menu.addItem(&entry);
        }

        /// Rebuilds the menu only when what it shows changed, so an open menu is left alone
        /// between ticks that change nothing.
        fn show(&mut self, menu: Menu) {
            if self.shown.as_ref() == Some(&menu) {
                return;
            }
            if let Some(button) = self.item.button(self.mtm) {
                button.setTitle(&NSString::from_str(&menu.title));
                button.setAccessibilityLabel(Some(&NSString::from_str(&menu.spoken)));
            }
            self.menu.removeAllItems();
            for line in &menu.lines {
                self.add(&line.title, &line.spoken, None);
            }
            if !menu.lines.is_empty() {
                self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
            }
            self.add(menu.toggle, menu.toggle, Some(sel!(toggleRecording:)));
            if let Some(pause) = menu.pause {
                self.add(pause, pause, Some(sel!(pauseRecording:)));
            }
            self.add("Show Dory", "Show Dory", Some(sel!(showWindow:)));
            self.add(
                "Processing Queue…",
                "Processing Queue",
                Some(sel!(showQueue:)),
            );
            self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
            self.add("Quit Dory", "Quit Dory", Some(sel!(quitDory:)));
            self.shown = Some(menu);
        }
    }

    struct Shared {
        item: StatusItem,
        clock: Clock,
        tick: Option<Task<()>>,
    }

    fn refresh(app: &Entity<AppState>, shared: &Rc<RefCell<Shared>>, cx: &mut App) {
        let state = app.read(cx);
        let status = state.recorder().clone();
        let mut in_progress = state.in_progress(cx);
        let mut shared = shared.borrow_mut();
        let elapsed = shared.clock.elapsed(&status, Instant::now());
        if let Some(take) = &mut in_progress.take {
            take.elapsed = elapsed;
        }
        shared.item.show(Menu::of(&status, elapsed, &in_progress));
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
        eprintln!("status-item: created {IDLE_TITLE}");
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
                cx.update(|cx| run(command, &app, cx));
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
    use crate::in_progress::{Downloading, Running, Take};

    fn id() -> RecordingId {
        RecordingId::parse("r1").unwrap()
    }

    #[test]
    fn the_title_is_dory_at_rest_a_dot_while_processing_and_the_clock_while_recording() {
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
            (RecorderStatus::Idle, Ms(0), false, "Dory"),
            (RecorderStatus::Idle, Ms(0), true, "Dory ·"),
            (recording.clone(), Ms(754_000), false, "● 12:34"),
            (recording.clone(), Ms(754_000), true, "● 12:34"),
            (recording, Ms(3_723_000), false, "● 1:02:03"),
            (paused, Ms(65_000), false, "Paused 1:05"),
            (grace.clone(), Ms(65_000), false, "Dory"),
            (grace, Ms(65_000), true, "Dory ·"),
        ];
        for (status, elapsed, busy, want) in cases {
            assert_eq!(
                title(&status, elapsed, busy),
                want,
                "{status:?} busy={busy}"
            );
        }
        assert_eq!(
            spoken(&RecorderStatus::Idle, Ms(0), true),
            "Dory, processing"
        );
    }

    #[test]
    fn the_menu_offers_stop_and_pause_while_recording_and_start_otherwise() {
        let live = RecorderStatus::Recording {
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
            (RecorderStatus::Idle, "Start Recording", None),
            (live, "Stop Recording", Some("Pause Recording")),
            (paused, "Stop Recording", Some("Resume Recording")),
            (grace, "Start Recording", None),
        ];
        for (status, toggle, pause) in cases {
            assert_eq!(toggle_label(&status), toggle, "{status:?}");
            assert_eq!(pause_label(&status), pause, "{status:?}");
        }
    }

    fn titles(state: &InProgress) -> Vec<String> {
        lines(state).into_iter().map(|line| line.title).collect()
    }

    fn take(state: TakeState) -> Option<Take> {
        Some(Take {
            title: "Weekly sync".into(),
            state,
            elapsed: Ms(754_000),
        })
    }

    #[test]
    fn nothing_in_progress_has_no_lines() {
        assert_eq!(lines(&InProgress::default()), vec![]);
    }

    #[test]
    fn the_take_reads_its_title_and_time() {
        for (state, want) in [
            (TakeState::Recording, "Recording “Weekly sync” · 12:34"),
            (TakeState::Paused, "Paused “Weekly sync” at 12:34"),
            (TakeState::Stopped, "Stopped “Weekly sync” at 12:34"),
        ] {
            let state = InProgress {
                take: take(state),
                ..InProgress::default()
            };
            assert_eq!(titles(&state), vec![want]);
        }
        let recording = InProgress {
            take: take(TakeState::Recording),
            ..InProgress::default()
        };
        assert_eq!(
            lines(&recording)[0].spoken,
            "Recording Weekly sync, 12 minutes 34 seconds",
            "VoiceOver reads words, not quotes and a dot"
        );
    }

    #[test]
    fn the_running_job_then_the_waiting_count_then_each_download() {
        let state = InProgress {
            take: None,
            running: Some(Running {
                title: "Design review".into(),
                stage: Stage::Transcribe,
                percent: Some(40),
            }),
            queued: 2,
            downloads: vec![
                Downloading {
                    name: "Parakeet".into(),
                    percent: Some(30),
                },
                Downloading {
                    name: "Pyannote".into(),
                    percent: None,
                },
            ],
        };
        assert_eq!(
            titles(&state),
            vec![
                "Transcribing “Design review” · 40%",
                "2 more waiting",
                "Downloading Parakeet · 30%",
                "Downloading Pyannote",
            ]
        );
        assert_eq!(
            lines(&state)[0].spoken,
            "Transcribing Design review, 40 percent"
        );
    }

    #[test]
    fn a_summary_without_progress_and_waiting_jobs_with_none_running() {
        let summarizing = InProgress {
            running: Some(Running {
                title: "Design review".into(),
                stage: Stage::Summarize,
                percent: None,
            }),
            ..InProgress::default()
        };
        assert_eq!(titles(&summarizing), vec!["Summarizing “Design review”"]);
        for (queued, want) in [(1, "1 recording waiting"), (3, "3 recordings waiting")] {
            let waiting = InProgress {
                queued,
                ..InProgress::default()
            };
            assert_eq!(titles(&waiting), vec![want]);
        }
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
