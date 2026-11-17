//! Quit (UX-8): the only real exit, since closing the window keeps Dory in the menu bar.
//!
//! **Quit Dory** and ⌘Q are handled app-wide, so they work from Settings, from About, with
//! nothing focused, and with the main window closed. With nothing in progress Dory quits at once.
//! Otherwise the main window opens, if it was closed, with a "Quit Dory?" dialog that says what
//! quitting interrupts ([`prompt`]), with Cancel focused so Return never quits by accident. Every
//! quit shuts the core down first, which saves the take; jobs and downloads resume at the next
//! launch.
//!
//! The Dock's Quit, logout, and restart go through `NSApp terminate:`, which GPUI cannot cancel,
//! so they cannot ask. [`install`]'s quit hook still shuts the core down on those paths.

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Global, IntoElement, ParentElement as _,
    Render, Role, Styled as _, Window, div, px,
};

use crate::a11y;
use crate::actions::Quit;
use crate::app::AppState;
use crate::in_progress::{InProgress, TakeState};
use crate::surface::{self, Look};
use crate::theme::Theme;

pub const TITLE: &str = "Quit Dory?";

/// What the dialog says: one sentence per thing in progress, and the destructive button's label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub lines: Vec<String>,
    pub confirm: &'static str,
}

/// `None` when nothing is in progress, so ⌘Q quits at once.
pub fn prompt(state: &InProgress) -> Option<Prompt> {
    if state.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    if let Some(take) = &state.take {
        let title = &take.title;
        lines.push(match take.state {
            TakeState::Recording => format!(
                "Dory is recording “{title}”. Quitting stops the recording and saves what was recorded."
            ),
            TakeState::Paused => format!(
                "“{title}” is paused. Quitting stops the recording and saves what was recorded."
            ),
            TakeState::Stopped => format!(
                "“{title}” has just stopped. Quitting saves it, and Undo Stop is no longer possible."
            ),
        });
    }
    match state.processing() {
        0 => {}
        1 => lines.push(
            "1 recording is still being processed. Dory picks up where it left off the next time you open it."
                .into(),
        ),
        n => lines.push(format!(
            "{n} recordings are still being processed. Dory picks up where it left off the next time you open it."
        )),
    }
    let names: Vec<&str> = state.downloads.iter().map(|d| d.name.as_str()).collect();
    match names.as_slice() {
        [] => {}
        [one] => lines.push(format!(
            "{one} is downloading. The download resumes the next time you open Dory."
        )),
        [rest @ .., last] => lines.push(format!(
            "{} and {last} are downloading. The downloads resume the next time you open Dory.",
            rest.join(", ")
        )),
    }
    let recording = state
        .take
        .as_ref()
        .is_some_and(|take| take.state != TakeState::Stopped);
    Some(Prompt {
        lines,
        confirm: if recording { "Stop and Quit" } else { "Quit" },
    })
}

/// How many times Dory asked the platform to quit. The test platform's quit does nothing, so the
/// tests read this instead.
#[derive(Default)]
pub struct Exits(pub usize);

impl Global for Exits {}

pub fn exits(cx: &App) -> usize {
    cx.try_global::<Exits>().map_or(0, |exits| exits.0)
}

/// Registers Quit app-wide, and the quit hook that saves the take on every path out.
pub fn install(app: &Entity<AppState>, cx: &mut App) {
    cx.on_action({
        let app = app.clone();
        move |_: &Quit, cx| request(&app, cx)
    });
    cx.on_app_quit({
        let app = app.clone();
        move |cx| {
            app.update(cx, |app, _| app.shut_down());
            async {}
        }
    })
    .detach();
}

/// Quit Dory and ⌘Q: quits now, or asks first when something is in progress.
pub fn request(app: &Entity<AppState>, cx: &mut App) {
    let Some(prompt) = prompt(&app.read(cx).in_progress(cx)) else {
        quit_now(app, cx);
        return;
    };
    let Some(window) = crate::show_main_window(cx) else {
        return;
    };
    let app = app.clone();
    // Deferred: ⌘Q in the main window runs this while that window is being updated.
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| ask(prompt, app, window, cx));
    });
}

/// Shuts the core down off the main thread, since sealing a long take takes a moment, then quits.
pub fn quit_now(app: &Entity<AppState>, cx: &mut App) {
    let done = app.update(cx, |app, cx| app.shut_down_in_background(cx));
    cx.spawn(async move |cx| {
        done.await;
        cx.update(|cx| {
            cx.default_global::<Exits>().0 += 1;
            cx.quit();
        });
    })
    .detach();
}

/// The "Quit Dory?" dialog over `window`, on Dory's dialog host, with Cancel focused. Asking
/// again while it is up replaces it with the same dialog.
pub fn ask(prompt: Prompt, app: Entity<AppState>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| Asking {
        prompt,
        app,
        cancel: cx.focus_handle(),
        quit: cx.focus_handle(),
    });
    let first = dialog.read(cx).cancel.clone();
    surface::open_dialog(TITLE, WIDTH, dialog.into(), &first, window, cx);
}

const WIDTH: f32 = 440.;

struct Asking {
    prompt: Prompt,
    app: Entity<AppState>,
    cancel: FocusHandle,
    quit: FocusHandle,
}

impl Render for Asking {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let mut body = div().px(px(20.)).pb(px(16.)).flex().flex_col().gap(px(8.));
        for (ix, line) in self.prompt.lines.iter().enumerate() {
            body = body.child(
                a11y::node(format!("quit-line-{ix}"), Role::Label, line.clone())
                    .text_size(theme.text(13.))
                    .text_color(theme.text)
                    .child(line.clone()),
            );
        }
        let cancel = surface::button(
            "quit-cancel",
            "Cancel",
            "Cancel",
            Look::Secondary,
            &self.cancel,
            &theme,
            surface::close_dialog,
        );
        let app = self.app.clone();
        let quit = surface::button(
            "quit-confirm",
            self.prompt.confirm,
            self.prompt.confirm,
            Look::Primary,
            &self.quit,
            &theme,
            move |window, cx| {
                surface::close_dialog(window, cx);
                quit_now(&app, cx);
            },
        )
        .bg(theme.record_red)
        .text_color(theme.on_record);
        div()
            .flex()
            .flex_col()
            .min_h_0()
            .child(surface::header("quit", TITLE, None, None, &theme))
            .child(body)
            .child(surface::footer(None, vec![cancel, quit], &theme))
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{Ms, Stage};

    use super::*;
    use crate::in_progress::{Downloading, Running, Take};

    fn take(state: TakeState) -> Option<Take> {
        Some(Take {
            title: "Weekly sync".into(),
            state,
            elapsed: Ms(754_000),
        })
    }

    fn running() -> Option<Running> {
        Some(Running {
            title: "Design review".into(),
            stage: Stage::Transcribe,
            percent: Some(40),
        })
    }

    fn download(name: &str) -> Downloading {
        Downloading {
            name: name.into(),
            percent: Some(30),
        }
    }

    const RECORDING: &str = "Dory is recording “Weekly sync”. Quitting stops the recording and saves what was recorded.";
    const PAUSED: &str =
        "“Weekly sync” is paused. Quitting stops the recording and saves what was recorded.";
    const STOPPED: &str =
        "“Weekly sync” has just stopped. Quitting saves it, and Undo Stop is no longer possible.";
    const ONE_JOB: &str = "1 recording is still being processed. Dory picks up where it left off the next time you open it.";
    const TWO_JOBS: &str = "2 recordings are still being processed. Dory picks up where it left off the next time you open it.";
    const PARAKEET: &str =
        "Parakeet is downloading. The download resumes the next time you open Dory.";

    #[test]
    fn nothing_in_progress_quits_without_asking() {
        assert_eq!(prompt(&InProgress::default()), None);
    }

    #[test]
    fn each_take_state_has_its_sentence_and_button() {
        for (state, line, confirm) in [
            (TakeState::Recording, RECORDING, "Stop and Quit"),
            (TakeState::Paused, PAUSED, "Stop and Quit"),
            (TakeState::Stopped, STOPPED, "Quit"),
        ] {
            let state = InProgress {
                take: take(state),
                ..InProgress::default()
            };
            assert_eq!(
                prompt(&state),
                Some(Prompt {
                    lines: vec![line.into()],
                    confirm,
                })
            );
        }
    }

    #[test]
    fn processing_counts_the_running_job_and_the_waiting_ones() {
        let cases = [
            (running(), 0, ONE_JOB),
            (None, 1, ONE_JOB),
            (running(), 1, TWO_JOBS),
            (None, 2, TWO_JOBS),
        ];
        for (running, queued, line) in cases {
            let state = InProgress {
                running,
                queued,
                ..InProgress::default()
            };
            assert_eq!(
                prompt(&state),
                Some(Prompt {
                    lines: vec![line.into()],
                    confirm: "Quit",
                }),
                "queued {queued}"
            );
        }
    }

    #[test]
    fn downloads_are_named_in_one_sentence() {
        let one = InProgress {
            downloads: vec![download("Parakeet")],
            ..InProgress::default()
        };
        assert_eq!(prompt(&one).unwrap().lines, vec![PARAKEET.to_owned()]);
        let three = InProgress {
            downloads: vec![
                download("Parakeet"),
                download("Pyannote"),
                download("Gemma"),
            ],
            ..InProgress::default()
        };
        assert_eq!(
            prompt(&three).unwrap().lines,
            vec![
                "Parakeet, Pyannote and Gemma are downloading. The downloads resume the next time you open Dory."
                    .to_owned()
            ]
        );
    }

    #[test]
    fn everything_at_once_lists_the_take_then_jobs_then_downloads() {
        let state = InProgress {
            take: take(TakeState::Recording),
            running: running(),
            queued: 1,
            downloads: vec![download("Parakeet")],
        };
        assert_eq!(
            prompt(&state),
            Some(Prompt {
                lines: vec![RECORDING.into(), TWO_JOBS.into(), PARAKEET.into()],
                confirm: "Stop and Quit",
            })
        );
    }
}
