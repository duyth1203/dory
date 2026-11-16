//! The detail pane for a selected recording (UX-2 to UX-4). Stub for agent 1, who owns this file
//! together with `summary_view.rs`.
//!
//! Agent 1 builds, without editing any other file:
//! - Inside the shell's centered 720 pt column, top to bottom: the editable title (30 bold,
//!   saved with `Session::set_title`), the metadata line (date, duration, speakers found; 14
//!   secondary), the player, the tab row, and the tab content. The page scrolls as one surface.
//! - The player: `player` below is the card. When the card scrolls out of view, call
//!   `PlayerView::set_docked(true)` and draw it docked 14 pt above the column's bottom edge;
//!   back to the card when it returns. One `PlayerView`, so focus and VoiceOver see one slider.
//! - The tab row (handoff "Tabs"): 34 pt, underline style, built with `a11y::tab` (Kit's tabs
//!   take no focus, spike finding 1). It sticks to the top once the title scrolls off and then
//!   shows the title at 14 semibold on its left. Command-1 and Command-2 are already handled at
//!   the window root through `AppState::set_tab`, so read `app.tab` and never keep a tab of your
//!   own. The row's right side holds Copy, Export ▾, and ⋯ (`menus::recording_menu`), each
//!   dispatching an action from `crate::actions`.
//! - Loading, failed (`Loadable::Failed`), and processing states (UX-11, UX-12: the live stage
//!   from `AppState::status` in place of the tabs).
//!
//! The shell creates one `DetailPane` per selection and drops it when the selection changes.
//! The start screen for "nothing selected" is the shell's, not this file's.

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::app::{AppState, DetailTab, Loadable, Session};
use crate::player_view::PlayerView;
use crate::summary_view;
use crate::theme::Theme;
use crate::transcript_view::TranscriptView;

pub struct DetailPane {
    app: Entity<AppState>,
    session: Entity<Session>,
    player: Entity<PlayerView>,
    transcript: Entity<TranscriptView>,
    tab_focus: [FocusHandle; 2],
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
        }
    }

    pub fn session(&self) -> &Entity<Session> {
        &self.session
    }
}

impl Render for DetailPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let tab = self.app.read(cx).tab;
        let session = self.session.read(cx);
        let detail = match session.loaded() {
            Loadable::Ready(detail) => detail.clone(),
            Loadable::Loading => return div().into_any_element(),
            Loadable::Failed(why) => {
                return div()
                    .text_color(theme.text_secondary)
                    .child(why.clone())
                    .into_any_element();
            }
        };
        let tab_button = |id: &'static str, label: &'static str, which: DetailTab, ix: usize| {
            let app = self.app.clone();
            a11y::tab(
                id,
                label,
                tab == which,
                &self.tab_focus[ix],
                &theme,
                move |_, cx| app.update(cx, |app, cx| app.set_tab(which, cx)),
            )
            .border_b_2()
            .when(tab == which, |t| t.border_color(theme.accent))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .text_size(theme.text(30.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child(detail.recording.title.clone().unwrap_or_default()),
            )
            .child(self.player.clone())
            .child(
                a11y::node("detail-tabs", gpui_kit::Role::TabList, "Recording views")
                    .flex()
                    .gap(px(16.))
                    .border_b_1()
                    .border_color(theme.divider)
                    .child(tab_button("summary-tab", "Summary", DetailTab::Summary, 0))
                    .child(tab_button(
                        "transcript-tab",
                        "Transcript",
                        DetailTab::Transcript,
                        1,
                    )),
            )
            .child(match tab {
                DetailTab::Summary => summary_view::summary(&detail, &theme),
                DetailTab::Transcript => self.transcript.clone().into_any_element(),
            })
            .into_any_element()
    }
}
