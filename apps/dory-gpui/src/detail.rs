//! The detail pane for a selected recording (UX-2 to UX-4).
//!
//! Inside the shell's centered 720 pt column, top to bottom: the editable title (30 bold, saved
//! with `Session::set_title`), the metadata line (date, duration, layout, detected languages; 14
//! secondary), the player, then [`Body`]: the tab row and tab content, or the state that takes
//! their place. The page scrolls as one surface, the shell's.
//!
//! The player card docks to a slim bar 14 pt above the column's bottom edge once it scrolls out
//! of view, and the tab row sticks to the top once it would scroll off, showing the title on its
//! left. Both measure their slot during prepaint against the scroll area's clip, so the shell
//! keeps the one scroll container. One `PlayerView`, so focus and VoiceOver see one slider.
//!
//! The tab row reads `app.tab`, which Command-1 and Command-2 set at the window root. Its Copy,
//! Export, and ⋯ buttons dispatch `crate::actions`, which the shell hands to `export.rs`.
//!
//! The shell creates one `DetailPane` per selection and drops it when the selection changes.

use std::cell::Cell;
use std::rc::Rc;

use chrono::{Local, TimeZone};
use dory_core::domain::{
    Blocker, ChannelLayout, JobState, Recording, RecordingRow, Stage, SummaryFlag, Transcript,
};
use dory_core::models::UnavailableReason;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::{
    Anchor, AnchoredPositionMode, AnyElement, App, AppContext as _, Bounds, Context, Entity,
    FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Pixels, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, anchored, canvas, deferred, div, point,
    prelude::FluentBuilder as _, px, relative,
};

use crate::a11y;
use crate::actions::{CopySummary, CopyTranscript, ExportMarkdown, ExportSrt, ExportText};
use crate::app::{AppState, Detail, DetailTab, Loadable, Session};
use crate::library::{self, RowStatus};
use crate::menus;
use crate::player_view::PlayerView;
use crate::summary_view;
use crate::theme::Theme;
use crate::transcript_view::TranscriptView;

/// What a job that stopped needs from the user, in the CLI's words adapted to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub text: String,
    pub fix: Option<String>,
}

impl Problem {
    /// `None` unless the job is blocked or failed.
    pub fn of(job: &JobState) -> Option<Problem> {
        let problem = |text: String, fix: &str| Problem {
            text,
            fix: Some(fix.to_owned()),
        };
        match job {
            JobState::Blocked { blocker, .. } => Some(match blocker {
                Blocker::NeedsProvision { model, bytes } => problem(
                    match bytes {
                        Some(b) => format!("{model} is not on this Mac ({} MB).", b / 1_000_000),
                        None => format!("{model} is not on this Mac."),
                    },
                    "Download it, then try again.",
                ),
                Blocker::Unavailable { model, why } => Problem {
                    text: format!("{model} is unavailable: {}.", unavailable(&why.reason)),
                    fix: Some(
                        why.fix
                            .clone()
                            .unwrap_or_else(|| "Pick another model, then try again.".into()),
                    ),
                },
                Blocker::Incompatible { model, why } => problem(
                    format!("{model} {why}."),
                    "Pick another model, or set the recording's language.",
                ),
                Blocker::NoModel { kind } => problem(
                    format!("No {kind} model is set up."),
                    "Choose one in Settings, then try again.",
                ),
            }),
            JobState::Failed { stage, error } => Some(Problem {
                text: format!("{} failed: {}", stage_noun(*stage), error.message),
                fix: None,
            }),
            _ => None,
        }
    }
}

fn unavailable(reason: &UnavailableReason) -> String {
    match reason {
        UnavailableReason::UnsupportedPlatform => "not supported on this Mac".into(),
        UnavailableReason::OsTooOld { required } => format!("needs {required}"),
        UnavailableReason::NotEligible { detail } => detail.clone(),
        UnavailableReason::ServerUnreachable { endpoint } => format!("cannot reach {endpoint}"),
    }
}

fn stage_noun(stage: Stage) -> &'static str {
    match stage {
        Stage::Transcribe => "Transcription",
        Stage::Summarize => "The summary",
    }
}

/// What the page shows under the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// No frames landed, so there is nothing to play or transcribe.
    NoAudio,
    /// Recording, queued, or transcribing, with no transcript yet (UX-11, UX-12).
    Processing(RowStatus),
    /// The job stopped before a transcript existed.
    NeedsAttention(Problem),
    NotTranscribed,
    /// Transcribed, but the transcript is empty and there is no summary.
    NothingSaid,
    Tabs,
}

impl Body {
    /// `job` and `status` are the selected row's; `None` when the row is outside the list.
    pub fn derive(detail: &Detail, job: Option<&JobState>, status: Option<RowStatus>) -> Body {
        if status == Some(RowStatus::Recording) {
            return Body::Processing(RowStatus::Recording);
        }
        if detail.recording.take.is_none() {
            return Body::NoAudio;
        }
        let Some(transcript) = &detail.transcript else {
            if let Some(problem) = job.and_then(Problem::of) {
                return Body::NeedsAttention(problem);
            }
            return match status {
                Some(s @ (RowStatus::Queued | RowStatus::Transcribing(_))) => Body::Processing(s),
                _ => Body::NotTranscribed,
            };
        };
        if transcript.segments.is_empty() && detail.summary.is_none() {
            Body::NothingSaid
        } else {
            Body::Tabs
        }
    }
}

/// The title as the sidebar shows it, with the same date-based fallback.
pub fn shown_title<Tz: TimeZone>(recording: &Recording, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let row = RecordingRow {
        recording: recording.clone(),
        job: None,
        has_transcript: false,
        summary: SummaryFlag::None,
    };
    library::title(&row, tz)
}

/// "Thu 12 Nov 2026, 10:30 · 48 min · Mic and system audio · English".
pub fn meta_line<Tz: TimeZone>(
    recording: &Recording,
    transcript: Option<&Transcript>,
    tz: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let mut parts = vec![
        library::local(recording.created_at, tz)
            .format("%a %-d %b %Y, %H:%M")
            .to_string(),
    ];
    if let Some(take) = recording.take {
        parts.push(library::short_duration(take.duration));
    }
    parts.push(
        match recording.layout {
            ChannelLayout::MicSystem => "Mic and system audio",
            ChannelLayout::MicOnly => "Mic only",
            ChannelLayout::SystemOnly => "System audio only",
            ChannelLayout::Mixed => "Mixed audio",
        }
        .to_owned(),
    );
    let languages: Vec<String> = transcript
        .map(|t| t.detected_languages.iter().map(language_name).collect())
        .unwrap_or_default();
    if !languages.is_empty() {
        parts.push(languages.join(" and "));
    }
    parts.join(" · ")
}

fn language_name(tag: &dory_core::domain::LangTag) -> String {
    match tag.primary() {
        "en" => "English",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Portuguese",
        "nl" => "Dutch",
        "vi" => "Vietnamese",
        "ja" => "Japanese",
        "zh" => "Chinese",
        "ko" => "Korean",
        "ru" => "Russian",
        _ => return tag.to_string(),
    }
    .to_owned()
}

/// Where the column, the player card, and the tab row sat in the last prepaint.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pinning {
    /// The scroll area's clip.
    visible: Bounds<Pixels>,
    column_x: Pixels,
    column_w: Pixels,
    card_h: Pixels,
    tabs_h: Pixels,
    docked: bool,
    stuck: bool,
}

#[derive(Clone, Copy)]
enum Slot {
    Card,
    Tabs,
}

struct TitleEdit {
    field: Entity<InputState>,
    _events: Subscription,
}

pub struct DetailPane {
    app: Entity<AppState>,
    session: Entity<Session>,
    player: Entity<PlayerView>,
    transcript: Entity<TranscriptView>,
    tab_focus: [FocusHandle; 2],
    title_focus: FocusHandle,
    title_edit: Option<TitleEdit>,
    /// What this frame lays out by.
    pinning: Pinning,
    /// What prepaint measured; a change re-renders with it.
    measured: Rc<Cell<Pinning>>,
}

impl DetailPane {
    pub fn new(
        app: Entity<AppState>,
        session: Entity<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        let handle = session.read(cx).player().clone();
        let player = cx.new(|cx| PlayerView::new(handle, cx));
        let transcript = cx.new(|cx| TranscriptView::new(session.clone(), window, cx));
        Self {
            app,
            session,
            player,
            transcript,
            tab_focus: [cx.focus_handle(), cx.focus_handle()],
            title_focus: cx.focus_handle(),
            title_edit: None,
            pinning: Pinning::default(),
            measured: Rc::default(),
        }
    }

    pub fn session(&self) -> &Entity<Session> {
        &self.session
    }

    pub fn is_editing_title(&self) -> bool {
        self.title_edit.is_some()
    }

    /// Swaps the title for a field holding it, all selected.
    pub fn edit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.session.read(cx).detail() else {
            return;
        };
        let current = detail.recording.title.clone().unwrap_or_default();
        let placeholder = shown_title(&detail.recording, &Local);
        let field = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(current)
        });
        field.update(cx, |field, cx| {
            field.focus(window, cx);
            field.select_all(window, cx);
        });
        let events = cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.finish_title_edit(true, window, cx);
            }
        });
        self.title_edit = Some(TitleEdit {
            field,
            _events: events,
        });
        cx.notify();
    }

    /// Saves the field when `commit` and it changed; either way the title comes back.
    fn finish_title_edit(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.title_edit.take() else {
            return;
        };
        if commit {
            let typed = edit.field.read(cx).value().trim().to_owned();
            let session = self.session.read(cx);
            let current = session
                .detail()
                .and_then(|d| d.recording.title.clone())
                .unwrap_or_default();
            if typed != current.trim() {
                let title = (!typed.is_empty()).then_some(typed);
                self.session
                    .update(cx, |session, cx| session.set_title(title, cx));
            }
        }
        window.focus(&self.title_focus, cx);
        cx.notify();
    }

    fn relayout(&mut self, cx: &mut Context<Self>) {
        let next = self.measured.get();
        if next == self.pinning {
            return;
        }
        if next.docked != self.pinning.docked {
            self.player
                .update(cx, |player, cx| player.set_docked(next.docked, cx));
        }
        self.pinning = next;
        cx.notify();
    }

    /// Measures `slot` during prepaint, and re-renders the pane when pinning changes.
    fn watch(&self, slot: Slot, cx: &mut Context<Self>) -> impl IntoElement {
        let laid_out = self.pinning;
        let measured = Rc::clone(&self.measured);
        let pane: WeakEntity<Self> = cx.entity().downgrade();
        canvas(
            move |bounds, window, cx: &mut App| {
                let visible = window.content_mask().bounds;
                let mut next = measured.get();
                next.visible = visible;
                next.column_x = bounds.origin.x;
                next.column_w = bounds.size.width;
                match slot {
                    Slot::Card => {
                        next.card_h = bounds.size.height;
                        next.docked = bounds.bottom() < visible.top();
                    }
                    Slot::Tabs => {
                        next.tabs_h = bounds.size.height;
                        next.stuck = bounds.top() < visible.top();
                    }
                }
                measured.set(next);
                if next != laid_out {
                    cx.defer(move |cx| {
                        let _ = pane.update(cx, |pane, cx| pane.relayout(cx));
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    fn title(&self, detail: &Detail, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if let Some(edit) = &self.title_edit {
            return a11y::node("title-editor", Role::Group, "Title")
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.finish_title_edit(false, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    Input::new(&edit.field)
                        .appearance(false)
                        .text_size(theme.text(30.))
                        .font_weight(FontWeight::BOLD),
                )
                .into_any_element();
        }
        let title = shown_title(&detail.recording, &Local);
        a11y::focusable(
            a11y::node("title", Role::Heading, title.clone()),
            &self.title_focus,
            theme,
            false,
        )
        .aria_level(1)
        .aria_description("Press Return to rename")
        .on_click(cx.listener(|this, _, window, cx| this.edit_title(window, cx)))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "enter" {
                this.edit_title(window, cx);
                cx.stop_propagation();
            }
        }))
        .rounded(px(6.))
        .text_size(theme.text(30.))
        .font_weight(FontWeight::BOLD)
        .line_height(relative(1.2))
        .child(title)
        .into_any_element()
    }

    fn tab_row(
        &self,
        tab: DetailTab,
        title: Option<String>,
        detail: &Detail,
        theme: &Theme,
    ) -> AnyElement {
        let tab_button = |id: &'static str, label: &'static str, which: DetailTab, ix: usize| {
            let app = self.app.clone();
            a11y::tab(
                id,
                label,
                tab == which,
                &self.tab_focus[ix],
                theme,
                move |_, cx| app.update(cx, |app, cx| app.set_tab(which, cx)),
            )
            .h(theme.text(34.))
            .flex()
            .items_center()
            .border_b_2()
            .when(tab == which, |t| t.border_color(theme.accent))
        };
        let (copy_label, copy): (&str, Box<dyn gpui_kit::Action>) = match tab {
            DetailTab::Summary => ("Copy", Box::new(CopySummary)),
            DetailTab::Transcript => ("Copy Transcript", Box::new(CopyTranscript)),
        };
        let id = detail.recording.id.clone();
        let action = |id: &'static str, label: &str| {
            Button::new(id)
                .ghost()
                .compact()
                .label(label.to_owned())
                .accessibility_id(id)
                .text_color(theme.accent_text)
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                action("copy", copy_label)
                    .on_click(move |_, window, cx| window.dispatch_action(copy.boxed_clone(), cx)),
            )
            .child(
                action("export", "Export")
                    .accessibility_label("Export")
                    .dropdown_caret(true)
                    .dropdown_menu(|menu, _, _| {
                        menu.menu("Markdown", Box::new(ExportMarkdown))
                            .menu("Plain Text", Box::new(ExportText))
                            .menu("Subtitles (SRT)", Box::new(ExportSrt))
                    }),
            )
            .child(
                action("more", "⋯")
                    .accessibility_label("More actions")
                    .dropdown_menu(move |menu, window, cx| {
                        menus::recording_menu(menu, &id, window, cx)
                    }),
            );
        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .border_b_1()
            .border_color(theme.divider)
            .bg(theme.window_bg)
            .children(title.map(|title| {
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .truncate()
                    .text_size(theme.text(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title)
            }))
            .child(
                a11y::node("detail-tabs", Role::TabList, "Recording views")
                    .flex()
                    .gap(px(16.))
                    .child(tab_button("summary-tab", "Summary", DetailTab::Summary, 0))
                    .child(tab_button(
                        "transcript-tab",
                        "Transcript",
                        DetailTab::Transcript,
                        1,
                    )),
            )
            .child(div().flex_1())
            .child(actions)
            .into_any_element()
    }
}

/// A one- or two-line state in place of the tabs. Its label is its text, for VoiceOver.
fn state(
    id: &'static str,
    role: Role,
    text: impl Into<SharedString>,
    fix: Option<String>,
    theme: &Theme,
) -> a11y::Node {
    let text: SharedString = text.into();
    let label: SharedString = match &fix {
        Some(fix) => format!("{text} {fix}").into(),
        None => text.clone(),
    };
    a11y::node(id, role, label)
        .flex()
        .flex_col()
        .gap(px(4.))
        .py(px(24.))
        .child(
            div()
                .text_size(theme.text(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(text),
        )
        .children(fix.map(|fix| {
            div()
                .text_size(theme.text(14.))
                .text_color(theme.text_secondary)
                .child(fix)
        }))
}

fn processing(status: RowStatus, theme: &Theme) -> a11y::Node {
    let percent = match status {
        RowStatus::Transcribing(Some(p)) => Some(p.get()),
        _ => None,
    };
    state("detail-state", Role::Status, status.label(), None, theme).children(percent.map(|p| {
        div()
            .h(px(4.))
            .w_full()
            .max_w(px(320.))
            .rounded(px(2.))
            .bg(theme.wave_rest.opacity(0.35))
            .child(
                div()
                    .h_full()
                    .rounded(px(2.))
                    .bg(theme.accent)
                    .w(relative(f32::from(p) / 100.)),
            )
    }))
}

impl Render for DetailPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (detail, write_error) = {
            let session = self.session.read(cx);
            match session.loaded() {
                Loadable::Ready(detail) => (detail.clone(), session.error().cloned()),
                Loadable::Loading => {
                    return state("detail-loading", Role::Status, "Loading…", None, &theme)
                        .text_color(theme.text_secondary)
                        .into_any_element();
                }
                Loadable::Failed(why) => {
                    return state("detail-failed", Role::Status, why.clone(), None, &theme)
                        .into_any_element();
                }
            }
        };
        let (tab, job, status) = {
            let app = self.app.read(cx);
            let row = app
                .recordings
                .iter()
                .find(|r| r.recording.id == detail.recording.id);
            (
                app.tab,
                row.and_then(|r| r.job.clone()),
                row.map(|r| app.status(r)),
            )
        };
        let body = Body::derive(&detail, job.as_ref(), status);
        let pinning = self.pinning;
        let has_player = detail.recording.take.is_some();
        let docked = has_player && pinning.docked;
        let stuck = body == Body::Tabs && pinning.stuck;

        let card_slot = has_player.then(|| {
            div()
                .relative()
                .when(docked, |slot| slot.h(pinning.card_h))
                .when(!docked, |slot| slot.child(self.player.clone()))
                .child(self.watch(Slot::Card, cx))
        });
        let docked_player = docked.then(|| {
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Window)
                    .anchor(Anchor::BottomLeft)
                    .position(point(pinning.column_x, pinning.visible.bottom() - px(14.)))
                    .child(div().w(pinning.column_w).child(self.player.clone())),
            )
            .with_priority(1)
        });

        let (content, sticky_row) = match &body {
            Body::Tabs => {
                let shown = shown_title(&detail.recording, &Local);
                let row = self.tab_row(tab, stuck.then_some(shown), &detail, &theme);
                let (in_flow, pinned) = if stuck {
                    (None, Some(row))
                } else {
                    (Some(row), None)
                };
                let slot = div()
                    .relative()
                    .when(stuck, |slot| slot.h(pinning.tabs_h))
                    .children(in_flow)
                    .child(self.watch(Slot::Tabs, cx));
                let page = match tab {
                    DetailTab::Summary => {
                        summary_view::summary(&detail, job.as_ref(), status, &theme)
                    }
                    DetailTab::Transcript => self.transcript.clone().into_any_element(),
                };
                let sticky = pinned.map(|row| {
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Window)
                            .position(point(pinning.visible.left(), pinning.visible.top()))
                            .child(
                                div()
                                    .w(pinning.visible.size.width)
                                    .bg(theme.window_bg)
                                    .child(
                                        div()
                                            .ml(pinning.column_x - pinning.visible.left())
                                            .w(pinning.column_w)
                                            .child(row),
                                    ),
                            ),
                    )
                    .with_priority(1)
                });
                (
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(slot)
                        .child(page)
                        .into_any_element(),
                    sticky,
                )
            }
            Body::Processing(status) => (processing(*status, &theme).into_any_element(), None),
            Body::NeedsAttention(problem) => (
                state(
                    "detail-state",
                    Role::Group,
                    problem.text.clone(),
                    problem.fix.clone(),
                    &theme,
                )
                .into_any_element(),
                None,
            ),
            Body::NoAudio => (
                state(
                    "detail-state",
                    Role::Group,
                    "No audio was recorded.",
                    Some("Nothing reached the microphone or system audio.".into()),
                    &theme,
                )
                .into_any_element(),
                None,
            ),
            Body::NotTranscribed => (
                state(
                    "detail-state",
                    Role::Group,
                    "Not transcribed yet.",
                    None,
                    &theme,
                )
                .into_any_element(),
                None,
            ),
            Body::NothingSaid => (
                state(
                    "detail-state",
                    Role::Group,
                    "Nothing was said.",
                    Some("The transcript is empty, so there is nothing to summarize.".into()),
                    &theme,
                )
                .into_any_element(),
                None,
            ),
        };

        let meta = meta_line(&detail.recording, detail.transcript.as_ref(), &Local);
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .pt(px(24.))
            .when(docked, |page| page.pb(px(80.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(self.title(&detail, &theme, cx))
                    .child(
                        a11y::node("detail-meta", Role::Label, meta.clone())
                            .text_size(theme.text(14.))
                            .line_height(relative(1.4))
                            .text_color(theme.text_secondary)
                            .child(meta),
                    )
                    .children(write_error.map(|error| {
                        a11y::node("detail-error", Role::Status, error.clone())
                            .text_size(theme.text(14.))
                            .text_color(theme.attention)
                            .child(error)
                    })),
            )
            .children(card_slot)
            .child(content)
            .children(docked_player)
            .children(sticky_row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{
        Blocker, FailureClass, JobError, JobState, LangTag, Revision, Stage, Transcript,
    };
    use dory_core::models::{KindTag, ModelRef};

    use super::*;
    use crate::app::Detail;
    use crate::fixtures::{self, SAMPLES};
    use crate::library::Percent;

    fn detail(title: &str) -> Detail {
        let sample = SAMPLES.iter().find(|s| s.title == title).unwrap();
        Detail {
            recording: sample.recording(),
            transcript: sample.transcript(),
            summary: sample.summary(),
            names: Default::default(),
            turns: Vec::new(),
        }
    }

    #[test]
    fn the_body_follows_the_take_the_transcript_and_the_job() {
        let done = detail("Design review");
        assert_eq!(
            Body::derive(&done, Some(&JobState::Succeeded), Some(RowStatus::Done)),
            Body::Tabs
        );

        let weekly = detail("Weekly sync");
        let running = JobState::Running {
            stage: Stage::Transcribe,
        };
        let forty =
            RowStatus::Transcribing(Some(Percent::of(dory_core::domain::Fraction::new(0.4))));
        assert_eq!(
            Body::derive(&weekly, Some(&running), Some(forty)),
            Body::Processing(forty)
        );
        assert_eq!(
            Body::derive(&weekly, None, Some(RowStatus::ReadyToTranscribe)),
            Body::NotTranscribed
        );

        let blocked = JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NoModel { kind: KindTag::Stt },
        };
        assert_eq!(
            Body::derive(&weekly, Some(&blocked), Some(RowStatus::NeedsAttention)),
            Body::NeedsAttention(Problem {
                text: "No speech model is set up.".into(),
                fix: Some("Choose one in Settings, then try again.".into()),
            })
        );

        let mut silent = detail("Weekly sync");
        silent.transcript = Some(Transcript {
            segments: Vec::new(),
            ..detail("Design review").transcript.unwrap()
        });
        assert_eq!(Body::derive(&silent, None, None), Body::NothingSaid);

        let mut no_take = detail("Design review");
        no_take.recording.take = None;
        assert_eq!(Body::derive(&no_take, None, None), Body::NoAudio);
    }

    #[test]
    fn problems_read_as_the_cli_words_them() {
        let failed = JobState::Failed {
            stage: Stage::Summarize,
            error: JobError {
                class: FailureClass::Permanent,
                message: "The summary model isn't installed.".into(),
            },
        };
        assert_eq!(
            Problem::of(&failed),
            Some(Problem {
                text: "The summary failed: The summary model isn't installed.".into(),
                fix: None,
            })
        );
        let model = ModelRef::parse("fake/whisper").unwrap();
        let provision = JobState::Blocked {
            stage: Stage::Transcribe,
            blocker: Blocker::NeedsProvision {
                model,
                bytes: Some(1_500_000_000),
            },
        };
        assert_eq!(
            Problem::of(&provision).unwrap().text,
            "fake/whisper is not on this Mac (1500 MB)."
        );
        assert_eq!(Problem::of(&JobState::Queued), None);
    }

    #[test]
    fn the_meta_line_has_date_duration_layout_and_languages() {
        let mut done = detail("Design review");
        assert_eq!(
            meta_line(&done.recording, done.transcript.as_ref(), &fixtures::zone()),
            "Thu 12 Nov 2026, 10:30 · 48 min · Mic and system audio"
        );
        let transcript = done.transcript.as_mut().unwrap();
        transcript.revision = Revision(2);
        transcript.detected_languages = vec![
            LangTag::parse("en").unwrap(),
            LangTag::parse("fr-FR").unwrap(),
        ];
        assert_eq!(
            meta_line(&done.recording, done.transcript.as_ref(), &fixtures::zone()),
            "Thu 12 Nov 2026, 10:30 · 48 min · Mic and system audio · English and French"
        );
    }
}
