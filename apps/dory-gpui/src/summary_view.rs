//! The Summary tab (UX-4).
//!
//! Rendered Markdown at 15/1.6 with 17 pt semibold section headings, from `Detail.summary`.
//! Kit's `TextView::markdown` draws it; its AX nodes come from `a11y::with_markdown_children`,
//! since Kit emits none (spike finding 3). A stale summary says so above the text. No summary
//! says why, from the recording's phase. A summary that failed shows here only, with the one
//! button that fixes it (UX-13), which `detail.rs` builds, while the transcript stays readable.
//! Copy and Export sit in the tab row, which `detail.rs` draws.
//!
//! The Markdown is drawn by [`SummaryText`], one per detail pane, from the text view state the
//! session owns, which keeps its state across tab switches. The session counts the matches over
//! the text that state renders, so while a search is active this view marks exactly those ranges
//! with the `search_match` fill, and scrolls the page to the current one when the session asks.
//! The text view takes a background per range and nothing else, so unlike the transcript's
//! marks these are the fill alone.

use dory_core::domain::Stage;
use gpui_kit::component::text::{
    RangeHighlight, RenderedText, TextView, TextViewState, TextViewStyle,
};
use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, Hsla, IntoElement as _,
    ParentElement as _, Render, Role, ScrollHandle, Styled as _, Subscription, Window, div, point,
    px, relative, rems,
};
use std::ops::Range;

use crate::a11y;
use crate::app::{Detail, Session};
use crate::find::shift_into_view;
use crate::icons::{self, Icon};
use crate::processing::{Fix, Phase};
use crate::theme::Theme;
use crate::transcript_view::{DOCK_CLEARANCE, TAB_ROW};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SummaryState<'a> {
    Markdown {
        markdown: &'a str,
        /// Written from an older transcript revision than the one stored.
        stale: bool,
    },
    Missing {
        text: String,
        fix: Option<Fix>,
    },
}

pub const STALE: &str = "This summary is from an earlier version of the transcript.";

impl<'a> SummaryState<'a> {
    pub fn of(detail: &'a Detail, phase: Option<&Phase>) -> SummaryState<'a> {
        if let Some(summary) = &detail.summary {
            return SummaryState::Markdown {
                markdown: &summary.markdown,
                stale: detail
                    .transcript
                    .as_ref()
                    .is_some_and(|t| summary.is_stale_for(t.revision)),
            };
        }
        if let Some(Phase::Failed(problem)) = phase
            && problem.stage() == Stage::Summarize
        {
            return SummaryState::Missing {
                text: problem.text(),
                fix: problem.fix(),
            };
        }
        let text = match phase {
            Some(Phase::Summarizing { .. }) => "Writing the summary…",
            Some(phase) if phase.is_processing() => {
                "The summary comes once the transcript is done."
            }
            _ => "No summary yet.",
        };
        SummaryState::Missing {
            text: text.into(),
            fix: None,
        }
    }
}

/// What the highlights in the text view were last set for.
#[derive(Clone, PartialEq)]
struct Marked {
    text: RenderedText,
    ranges: Vec<Range<usize>>,
    fill: Hsla,
}

/// Emitted once the match the session asked to show is in view, or there was none to show.
pub struct Revealed;

pub struct SummaryText {
    session: Entity<Session>,
    page: ScrollHandle,
    state: Entity<TextViewState>,
    /// The session's `reveal` count this view last acted on.
    seen: u64,
    /// The current match's place among the summary's matches, until the view has scrolled to it.
    wanted: Option<usize>,
    marked: Option<Marked>,
    ranges: Vec<Range<usize>>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<Revealed> for SummaryText {}

impl SummaryText {
    /// `page` is the scroll area the detail pane sits in, which a match scrolls into view.
    pub fn new(session: Entity<Session>, page: ScrollHandle, cx: &mut Context<Self>) -> Self {
        let state = session.read(cx).summary_state().clone();
        let on_session = cx.observe(&session, |this, _, cx| this.sync(cx));
        let on_state = cx.observe(&state, |this, _, cx| this.sync(cx));
        let mut view = Self {
            session,
            page,
            state,
            seen: 0,
            wanted: None,
            marked: None,
            ranges: Vec::new(),
            _subscriptions: [on_session, on_state],
        };
        view.sync(cx);
        view
    }

    /// The ranges of the rendered text the search marks.
    pub fn marks(&self) -> &[Range<usize>] {
        &self.ranges
    }

    /// The text as drawn, which `marks` index.
    pub fn rendered(&self, cx: &gpui_kit::App) -> String {
        self.state.read(cx).rendered_text().as_str().to_owned()
    }

    /// Brings the highlights and the wish to scroll in line with the session.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let reveal = session.reveal();
        let found = session.found();
        let wanted = found.and_then(|found| found.in_summary().1);
        if reveal != self.seen {
            self.seen = reveal;
            self.wanted = wanted;
        }
        let rendered = self.state.read(cx).rendered_text();
        let ranges: Vec<Range<usize>> = match (found, session.counted_summary()) {
            (Some(found), Some(counted)) if *counted == rendered => found
                .in_summary()
                .0
                .iter()
                .map(|m| m.span.clone())
                .collect(),
            // The session counts the text this view draws, and has not counted this version of it.
            (Some(_), _) => return,
            (None, _) => Vec::new(),
        };
        let key = Marked {
            text: rendered,
            ranges,
            fill: Theme::get(cx).search_match,
        };
        if self.marked.as_ref() == Some(&key) {
            return;
        }
        self.ranges = key.ranges.clone();
        let fill = key.fill;
        let ranges = key.ranges.clone();
        self.state.update(cx, |state, cx| {
            if ranges.is_empty() {
                state.clear_range_highlights(cx);
            } else if let Err(error) = state.set_range_highlights(
                ranges
                    .into_iter()
                    .map(|range| RangeHighlight::new(range, fill)),
                cx,
            ) {
                eprintln!("summary marks failed: {error}");
            }
        });
        self.marked = Some(key);
    }

    /// Asks the text view to scroll to the `ordinal`th match. Called while the tab is drawn, so
    /// the line is laid out within the second the request lasts.
    fn show(&mut self, ordinal: usize, window: &mut Window, cx: &mut Context<Self>) {
        let range = self
            .ranges
            .get(ordinal.min(self.ranges.len().saturating_sub(1)))
            .cloned();
        if let Some(range) = range {
            let revealed = self
                .state
                .update(cx, |state, cx| state.reveal_range(range, cx));
            if let Err(error) = revealed {
                eprintln!("summary reveal failed: {error}");
            }
        }
        // The text view calls back with the line once it is laid out and not in view, and the
        // page moves in that frame, so the frame after it paints the match in view.
        let this = cx.entity().downgrade();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(Revealed));
            })
        });
    }
}

impl Render for SummaryText {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = Theme::get(cx);
        if self.marked.is_some()
            && let Some(ordinal) = self.wanted.take()
        {
            self.show(ordinal, window, cx);
        }
        let heading = theme.text(17.);
        let style = TextViewStyle::default()
            .paragraph_gap(rems(0.75))
            .heading_font_size(move |_, _| heading);
        let page = self.page.clone();
        let (top, bottom) = (theme.text(TAB_ROW) + px(8.), px(DOCK_CLEARANCE));
        TextView::new(&self.state)
            .style(style)
            .selectable(true)
            .text_size(theme.text(15.))
            .line_height(relative(1.6))
            .on_reveal(move |line, _, _| {
                if let Some(dy) = shift_into_view(line, page.bounds(), top, bottom) {
                    let offset = page.offset();
                    let y = (offset.y + dy).max(-page.max_offset().y).min(px(0.));
                    page.set_offset(point(offset.x, y));
                }
            })
    }
}

/// `text` draws the Markdown. `fix_button` is the job's fix, shown under a summary that is missing because of it.
pub fn summary(
    detail: &Detail,
    phase: Option<&Phase>,
    text: &Entity<SummaryText>,
    fix_button: Option<AnyElement>,
    theme: &Theme,
) -> AnyElement {
    let doc = a11y::node("summary-document", Role::Document, "Summary");
    match SummaryState::of(detail, phase) {
        SummaryState::Missing { text, fix } => doc
            .child(
                a11y::node("summary-missing", Role::Status, text.clone())
                    .text_size(theme.text(15.))
                    .text_color(theme.text_secondary)
                    .child(text),
            )
            .children(
                fix.and(fix_button)
                    .map(|button| div().pt(px(8.)).child(button)),
            )
            .into_any_element(),
        SummaryState::Markdown { markdown, stale } => {
            let body = a11y::with_markdown_children(doc, markdown)
                .flex()
                .flex_col()
                .gap(px(12.));
            let body = if stale {
                body.child(
                    a11y::node("summary-stale", Role::Status, STALE)
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(7.))
                        .bg(theme.control_bg)
                        .text_size(theme.text(13.))
                        .text_color(theme.text_secondary)
                        .font_weight(FontWeight::MEDIUM)
                        .child(icons::beside(
                            Icon::TriangleAlert,
                            12.,
                            theme.text_secondary,
                            theme.text(13.) * 1.4,
                            STALE,
                        )),
                )
            } else {
                body
            };
            body.child(text.clone()).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::Revision;
    use gpui_kit::Role;

    use super::*;
    use crate::estimate::Left;
    use crate::fixtures::SAMPLES;
    use crate::processing::Problem;

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
    fn a_summary_from_an_older_transcript_is_stale() {
        let mut done = detail("Design review");
        assert!(matches!(
            SummaryState::of(&done, None),
            SummaryState::Markdown { stale: false, .. }
        ));
        done.transcript.as_mut().unwrap().revision = Revision(2);
        assert!(matches!(
            SummaryState::of(&done, None),
            SummaryState::Markdown { stale: true, .. }
        ));
    }

    #[test]
    fn a_missing_summary_says_why() {
        let hiring = detail("Hiring debrief");
        assert_eq!(
            SummaryState::of(&hiring, Some(&Phase::Failed(Problem::SummaryModelMissing))),
            SummaryState::Missing {
                text: "The transcript is ready, but summaries aren't set up.".into(),
                fix: Some(Fix::SetUpSummaries),
            }
        );
        let missing = |phase: Phase| match SummaryState::of(&hiring, Some(&phase)) {
            SummaryState::Missing { text, .. } => text,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            missing(Phase::Summarizing {
                left: Left::Estimating
            }),
            "Writing the summary…"
        );
        assert_eq!(
            missing(Phase::Preparing),
            "The summary comes once the transcript is done."
        );
        assert_eq!(missing(Phase::Done { summary: false }), "No summary yet.");
    }

    #[test]
    fn summary_markdown_maps_to_heading_list_and_paragraph_nodes() {
        let blocks = a11y::markdown_blocks(
            "## Decisions\n\n- Ship the docked player.\n- Keep **one** slider.\n\n## Next steps\n\n1. Book the review.\n\nThanks, all.",
        );
        let shape: Vec<(Role, Option<usize>, &str)> = blocks
            .iter()
            .map(|b| (b.role, b.level, b.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                (Role::Heading, Some(2), "Decisions"),
                (Role::ListItem, None, "Ship the docked player."),
                (Role::ListItem, None, "Keep one slider."),
                (Role::Heading, Some(2), "Next steps"),
                (Role::ListItem, None, "Book the review."),
                (Role::Paragraph, None, "Thanks, all."),
            ]
        );
    }
}
