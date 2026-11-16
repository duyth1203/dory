//! The Processing Queue popover (UX-15): every running, queued, and blocked job with its stage
//! and percent, and **Move to top** and **Cancel** per job. Cancel asks nothing. It opens as a
//! dialog layer on the window, so Escape closes it and Tab stays inside it (A11Y-2).

use std::collections::HashMap;

use dory_core::domain::{Job, JobId, JobState, RecordingId, Stage};
use gpui_kit::component::{
    WindowExt as _,
    dialog::{Confirm, DialogFooter},
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::app::AppState;
use crate::library::{self, Percent, Progress};
use crate::theme::Theme;

/// A pill button that takes focus and activates with a click, Return, or Space. Return is also
/// the dialog's Confirm key, so the button answers `Confirm` itself or the dialog would eat it.
pub(crate) fn pill_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    text: &'static str,
    focus: &FocusHandle,
    theme: &Theme,
    press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let on_key = press.clone();
    let on_confirm = press.clone();
    a11y::focusable(a11y::node(id, Role::Button, label), focus, theme, false)
        .on_click(move |_, window, cx| press(window, cx))
        .on_action(move |_: &Confirm, window, cx| {
            on_confirm(window, cx);
            cx.stop_propagation();
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "space" {
                on_key(window, cx);
                cx.stop_propagation();
            }
        })
        .h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded(px(14.))
        .bg(theme.control_bg)
        .text_size(theme.text(13.))
        .child(text)
}

/// One row of the popover, already worded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub job: JobId,
    pub title: String,
    pub status: String,
    pub running: bool,
    /// False for the running job and for the next one to run, which Move to top cannot improve.
    pub can_promote: bool,
}

fn status(state: &JobState, progress: Option<Progress>) -> String {
    match state {
        JobState::Running {
            stage: Stage::Transcribe,
        } => match progress.filter(|p| p.stage == Stage::Transcribe) {
            Some(p) => format!("Transcribing, {}%", Percent::of(p.fraction).get()),
            None => "Transcribing".into(),
        },
        JobState::Running {
            stage: Stage::Summarize,
        } => "Summarizing".into(),
        JobState::Queued => "Queued".into(),
        JobState::Blocked { .. } => "Needs attention".into(),
        JobState::Failed { .. } => "Failed".into(),
        JobState::Succeeded => "Done".into(),
        JobState::Cancelled => "Cancelled".into(),
    }
}

/// `jobs` arrive in queue order, so the first queued job is the one the worker takes next.
pub fn lines(
    jobs: &[Job],
    title_of: impl Fn(&RecordingId) -> String,
    progress_of: impl Fn(&RecordingId) -> Option<Progress>,
) -> Vec<Line> {
    let mut seen_queued = false;
    jobs.iter()
        .map(|job| {
            let queued = job.state == JobState::Queued;
            let can_promote = queued && seen_queued;
            seen_queued |= queued;
            Line {
                job: job.id,
                title: title_of(&job.recording),
                status: status(&job.state, progress_of(&job.recording)),
                running: matches!(job.state, JobState::Running { .. }),
                can_promote,
            }
        })
        .collect()
}

pub fn open(app: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let panel = cx.new(|cx| QueuePanel::new(app.clone(), cx));
    let close = cx.focus_handle();
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = Theme::get(cx);
        let done = pill_button(
            "queue-done",
            "Done",
            "Done",
            &close,
            &theme,
            |window, cx| window.close_dialog(cx),
        );
        dialog
            .title("Processing Queue")
            .w(px(520.))
            .child(panel.clone())
            .footer(DialogFooter::new().child(done))
    });
}

enum Jobs {
    Loading,
    Ready(Vec<Job>),
    Failed(SharedString),
}

struct QueuePanel {
    app: Entity<AppState>,
    jobs: Jobs,
    focus: HashMap<(JobId, bool), FocusHandle>,
    generation: u64,
    _refresh: Task<()>,
    _observe: Subscription,
}

impl QueuePanel {
    fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&app, |this, _, cx| this.refresh(cx));
        let mut this = Self {
            app,
            jobs: Jobs::Loading,
            focus: HashMap::new(),
            generation: 0,
            _refresh: Task::ready(()),
            _observe: observe,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let read = self.app.update(cx, |app, cx| app.queue(cx));
        self._refresh = cx.spawn(async move |this, cx| {
            let jobs = read.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match jobs {
                    Ok(jobs) => {
                        this.focus
                            .retain(|(id, _), _| jobs.iter().any(|job| job.id == *id));
                        this.jobs = Jobs::Ready(jobs);
                    }
                    Err(error) => this.jobs = Jobs::Failed(error.into()),
                }
                cx.notify();
            });
        });
    }

    fn focus_for(&mut self, job: JobId, promote: bool, cx: &mut Context<Self>) -> FocusHandle {
        self.focus
            .entry((job, promote))
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
}

impl Render for QueuePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let list = a11y::node("queue", Role::List, "Processing Queue")
            .flex()
            .flex_col()
            .gap(px(4.));
        let jobs = match &self.jobs {
            Jobs::Ready(jobs) => jobs.clone(),
            Jobs::Loading => {
                return list.child(
                    div()
                        .text_size(theme.text(13.))
                        .text_color(theme.text_secondary)
                        .child("Loading"),
                );
            }
            Jobs::Failed(error) => {
                return list.child(
                    div()
                        .text_size(theme.text(13.))
                        .text_color(theme.attention)
                        .child(error.clone()),
                );
            }
        };
        let app = self.app.clone();
        let rows = lines(
            &jobs,
            |id| {
                app.read(cx)
                    .recordings
                    .iter()
                    .find(|row| &row.recording.id == id)
                    .map_or_else(
                        || "Recording".to_owned(),
                        |row| library::title(row, &chrono::Local),
                    )
            },
            |id| app.read(cx).progress_of(id),
        );
        if rows.is_empty() {
            return list.child(
                div()
                    .py(px(8.))
                    .text_size(theme.text(13.))
                    .text_color(theme.text_secondary)
                    .child("Nothing is processing."),
            );
        }
        let mut list = list;
        for line in rows {
            let job = line.job;
            let title = line.title.clone();
            let promote_focus = self.focus_for(job, true, cx);
            let cancel_focus = self.focus_for(job, false, cx);
            let on_promote = {
                let app = self.app.clone();
                move |_: &mut Window, cx: &mut App| {
                    app.update(cx, |app, cx| app.promote_job(job, cx))
                }
            };
            let on_cancel = {
                let app = self.app.clone();
                move |_: &mut Window, cx: &mut App| {
                    app.update(cx, |app, cx| app.cancel_job(job, cx))
                }
            };
            list = list.child(
                a11y::node(
                    format!("queue-job-{}", job.0),
                    Role::ListItem,
                    format!("{}, {}", line.title, line.status),
                )
                .flex()
                .items_center()
                .gap(px(8.))
                .py(px(6.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(theme.text(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(line.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(theme.text(12.))
                                .text_color(theme.text_secondary)
                                .child(line.status.clone()),
                        ),
                )
                .when(line.can_promote, |row| {
                    row.child(pill_button(
                        format!("queue-promote-{}", job.0),
                        format!("Move {title} to top"),
                        "Move to top",
                        &promote_focus,
                        &theme,
                        on_promote,
                    ))
                })
                .child(pill_button(
                    format!("queue-cancel-{}", job.0),
                    format!("Cancel {}", line.title),
                    "Cancel",
                    &cancel_focus,
                    &theme,
                    on_cancel,
                )),
            );
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dory_core::domain::{Fraction, Goal, Priority, Timestamp};

    fn job(id: u64, state: JobState) -> Job {
        let recording = RecordingId::parse(&format!("r{id}")).unwrap();
        let mut job = Job::new(
            JobId(id),
            recording,
            Goal::ALL,
            Priority::Normal,
            Timestamp(0),
        );
        job.state = state;
        job
    }

    #[test]
    fn lines_word_each_state_and_offer_move_to_top_after_the_first_queued() {
        let jobs = [
            job(
                1,
                JobState::Running {
                    stage: Stage::Transcribe,
                },
            ),
            job(2, JobState::Queued),
            job(3, JobState::Queued),
        ];
        let progress = |id: &RecordingId| {
            (id.as_str() == "r1").then(|| Progress {
                stage: Stage::Transcribe,
                fraction: Fraction::new(0.42),
            })
        };
        let lines = lines(&jobs, |id| id.as_str().to_owned(), progress);
        let got: Vec<_> = lines
            .iter()
            .map(|l| (l.status.as_str(), l.running, l.can_promote))
            .collect();
        assert_eq!(
            got,
            [
                ("Transcribing, 42%", true, false),
                ("Queued", false, false),
                ("Queued", false, true),
            ]
        );
    }
}
