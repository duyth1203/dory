//! The Summary tab (UX-4).
//!
//! Rendered Markdown at 15/1.6 with 17 pt semibold section headings, from `Detail.summary`.
//! Kit's `TextView::markdown` draws it; its AX nodes come from `a11y::with_markdown_children`,
//! since Kit emits none (spike finding 3). A stale summary says so above the text. No summary
//! says why, from the recording's phase. A summary that failed shows here only, with the one
//! button that fixes it (UX-13), which `detail.rs` builds, while the transcript stays readable.
//! Copy and Export sit in the tab row, which `detail.rs` draws.

use dory_core::domain::Stage;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::{
    AnyElement, FontWeight, IntoElement as _, ParentElement as _, Role, Styled as _, div, px,
    relative, rems,
};

use crate::a11y;
use crate::app::Detail;
use crate::processing::{Fix, Phase};
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

/// `fix_button` is the job's fix, shown under a summary that is missing because of it.
pub fn summary(
    detail: &Detail,
    phase: Option<&Phase>,
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
