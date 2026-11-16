//! The Summary tab (UX-4).
//!
//! Rendered Markdown at 15/1.6 with 17 pt semibold section headings, from `Detail.summary`.
//! Kit's `TextView::markdown` draws it; its AX nodes come from `a11y::with_markdown_children`,
//! since Kit emits none (spike finding 3). A stale summary says so above the text. No summary
//! says why, from the recording's job, with the one button that fixes it (UX-13), which
//! `detail.rs` builds. Copy and Export sit in the tab row, which `detail.rs` draws.

use dory_core::domain::JobState;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::{
    AnyElement, FontWeight, IntoElement as _, ParentElement as _, Role, Styled as _, div, px,
    relative, rems,
};

use crate::a11y;
use crate::app::Detail;
use crate::detail::Problem;
use crate::library::RowStatus;
use crate::processing::Fix;
use crate::theme::Theme;

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
    pub fn of(
        detail: &'a Detail,
        job: Option<&JobState>,
        status: Option<RowStatus>,
    ) -> SummaryState<'a> {
        if let Some(summary) = &detail.summary {
            return SummaryState::Markdown {
                markdown: &summary.markdown,
                stale: detail
                    .transcript
                    .as_ref()
                    .is_some_and(|t| summary.is_stale_for(t.revision)),
            };
        }
        if let Some(problem) = job.and_then(Problem::of) {
            return SummaryState::Missing {
                text: problem.text,
                fix: Some(problem.fix),
            };
        }
        let text = match status {
            Some(RowStatus::Summarizing) => "Summarizing…",
            Some(RowStatus::Queued | RowStatus::Transcribing(_)) => {
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

/// `fix_button` is the job's fix, shown under a summary that is missing because of it.
pub fn summary(
    detail: &Detail,
    job: Option<&JobState>,
    status: Option<RowStatus>,
    fix_button: Option<AnyElement>,
    theme: &Theme,
) -> AnyElement {
    let doc = a11y::node("summary-document", Role::Document, "Summary");
    match SummaryState::of(detail, job, status) {
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
            let heading = theme.text(17.);
            let style = TextViewStyle::default()
                .paragraph_gap(rems(0.75))
                .heading_font_size(move |_, _| heading);
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
                        .child(format!("⚠︎ {STALE}")),
                )
            } else {
                body
            };
            body.child(
                TextView::markdown("summary-markdown", markdown.to_owned())
                    .style(style)
                    .selectable(true)
                    .text_size(theme.text(15.))
                    .line_height(relative(1.6)),
            )
            .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{Blocker, JobState, Revision, Stage};
    use dory_core::models::KindTag;
    use gpui_kit::Role;

    use super::*;
    use crate::fixtures::SAMPLES;

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
            SummaryState::of(&done, None, None),
            SummaryState::Markdown { stale: false, .. }
        ));
        done.transcript.as_mut().unwrap().revision = Revision(2);
        assert!(matches!(
            SummaryState::of(&done, None, None),
            SummaryState::Markdown { stale: true, .. }
        ));
    }

    #[test]
    fn a_missing_summary_says_why() {
        let hiring = detail("Hiring debrief");
        let blocked = JobState::Blocked {
            stage: Stage::Summarize,
            blocker: Blocker::NoModel { kind: KindTag::Llm },
        };
        assert_eq!(
            SummaryState::of(&hiring, Some(&blocked), Some(RowStatus::NeedsAttention)),
            SummaryState::Missing {
                text: "No summary model is set up.".into(),
                fix: Some(Fix::SetUpSummaries),
            }
        );
        let missing = |status| match SummaryState::of(&hiring, None, status) {
            SummaryState::Missing { text, .. } => text,
            other => panic!("{other:?}"),
        };
        assert_eq!(missing(Some(RowStatus::Summarizing)), "Summarizing…");
        assert_eq!(missing(Some(RowStatus::Done)), "No summary yet.");
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
