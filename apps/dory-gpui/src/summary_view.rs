//! The Summary tab (UX-4).
//!
//! Rendered Markdown at 15/1.6 with 17 pt semibold section headings, from `Detail.summary`.
//! Kit's `TextView::markdown` draws it; its AX nodes come from `a11y::with_markdown_children`,
//! since Kit emits none (spike finding 3). A stale summary says so above the text. No summary
//! says why, from the recording's phase. A summary that failed shows here only, with the one
//! button that fixes it (UX-13), which `detail.rs` builds, while the transcript stays readable.
//! Copy and Export sit in the tab row, which `detail.rs` draws.
//!
//! Book (`docs/design/book-theme.md` section 4) draws the same Markdown block by block: serif 17/1.7
//! paragraphs and list items in Kit's `TextView` (so they stay selectable and keep bold), small
//! caps headings, hanging en dashes for bullets, a drop cap on a leading paragraph, and an
//! asterism between the first two sections. The accessibility nodes are the same in both themes.

use dory_core::domain::Stage;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::{
    AnyElement, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Role,
    Styled as _, div, prelude::FluentBuilder as _, px, relative, rems,
};

use crate::a11y;
use crate::app::Detail;
use crate::fonts;
use crate::icons::{self, Icon};
use crate::processing::{Fix, Phase};
use crate::theme::{Presentation, Theme};

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
    presentation: Presentation,
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
            if theme.serif() {
                return body
                    .gap(px(0.))
                    .child(book(markdown, theme, presentation))
                    .into_any_element();
            }
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

/// One line of summary Markdown as Book lays it out, its inline Markdown kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    Heading(&'a str),
    /// `marker` is "–" for a bullet, or the number and its period.
    Item {
        marker: &'a str,
        text: &'a str,
    },
    Paragraph(&'a str),
}

/// The lines of `markdown`, the same split as `a11y::markdown_blocks`.
pub fn lines(markdown: &str) -> Vec<Line<'_>> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            let numbered = line
                .split_once(". ")
                .filter(|(n, _)| n.parse::<u32>().is_ok());
            if hashes > 0 {
                Line::Heading(line[hashes..].trim())
            } else if let Some(text) = line.strip_prefix("- ").or(line.strip_prefix("* ")) {
                Line::Item {
                    marker: "–", text
                }
            } else if let Some((n, text)) = numbered {
                Line::Item {
                    marker: &line[..n.len() + 1],
                    text,
                }
            } else {
                Line::Paragraph(line)
            }
        })
        .collect()
}

/// Where Book's decorations go in `lines`: the paragraph that takes the drop cap, and the line
/// the asterism sits before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub drop_cap: Option<usize>,
    pub ornament_before: Option<usize>,
}

/// A paragraph shorter than this would sit beside its own 3-line initial.
const DROP_CAP_MIN_CHARS: usize = 120;

impl Marks {
    /// The drop cap goes on the first line under the opening headings when it is a paragraph long
    /// enough to wrap past the initial and starts with a letter or digit. A section starts at a
    /// heading that follows body text, so `# Summary` over `## Key points` stays one section, and
    /// the asterism goes before the second.
    pub fn of(lines: &[Line]) -> Marks {
        let first_body = lines.iter().position(|l| !matches!(l, Line::Heading(_)));
        let drop_cap = first_body.filter(|&ix| match lines[ix] {
            Line::Paragraph(text) => {
                text.chars().count() >= DROP_CAP_MIN_CHARS
                    && text.chars().next().is_some_and(char::is_alphanumeric)
            }
            _ => false,
        });
        let ornament_before = first_body.and_then(|start| {
            (start + 1..lines.len()).find(|&ix| {
                matches!(lines[ix], Line::Heading(_)) && !matches!(lines[ix - 1], Line::Heading(_))
            })
        });
        Marks {
            drop_cap,
            ornament_before,
        }
    }
}

fn plain(text: &str) -> String {
    text.replace("**", "")
}

/// Book's summary. Every node here is drawing only: VoiceOver reads `summary-document`'s blocks.
fn book(markdown: &str, theme: &Theme, presentation: Presentation) -> AnyElement {
    let lines = lines(markdown);
    let marks = Marks::of(&lines);
    let body = |id: String, text: &str| {
        theme
            .reading(TextView::markdown(id, text.to_owned()))
            .selectable(true)
            .text_size(theme.text(17.))
            .line_height(relative(1.7))
    };
    let line_height = theme.text(17.) * 1.7;
    let initial = theme.text(17.) * 5.7;
    let mut column = div().flex().flex_col();
    for (ix, line) in lines.iter().enumerate() {
        if presentation.section_ornament && marks.ornament_before == Some(ix) {
            column = column.child(
                theme
                    .reading(div())
                    .py(px(14.))
                    .flex()
                    .justify_center()
                    .text_size(theme.text(17.))
                    .text_color(theme.text_secondary)
                    .child("⁂"),
            );
        }
        let el = match *line {
            Line::Heading(text) => theme
                .reading(div())
                .when(ix > 0, |h| h.pt(px(14.)))
                .pb(px(4.))
                .text_size(theme.text(14.))
                .font_weight(FontWeight::MEDIUM)
                .font_features(fonts::small_caps())
                .text_color(theme.text_secondary)
                .child(plain(text))
                .into_any_element(),
            Line::Item { marker, text } => div()
                .flex()
                .gap(px(10.))
                .pb(px(4.))
                .child(
                    theme
                        .reading(div())
                        .flex_none()
                        .min_w(theme.text(14.))
                        .text_size(theme.text(17.))
                        .line_height(relative(1.7))
                        .text_color(theme.text_secondary)
                        .child(marker.to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(body(format!("summary-line-{ix}"), text)),
                )
                .into_any_element(),
            Line::Paragraph(text) if presentation.drop_cap && marks.drop_cap == Some(ix) => {
                let split = text.char_indices().nth(1).map_or(text.len(), |(at, _)| at);
                div()
                    .flex()
                    .gap(px(6.))
                    .pb(px(8.))
                    .child(
                        div().flex_none().h(line_height * 3.).child(
                            theme
                                .reading(div())
                                .id("drop-cap")
                                // Sized so the capital runs from the first line's cap height
                                // to the third line's baseline, then lifted onto the first.
                                .mt(-(initial * 0.067))
                                .text_size(initial)
                                .line_height(relative(1.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.accent_text)
                                .child(text[..split].to_owned()),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(body(format!("summary-line-{ix}"), &text[split..])),
                    )
                    .into_any_element()
            }
            Line::Paragraph(text) => div()
                .pb(px(8.))
                .child(body(format!("summary-line-{ix}"), text))
                .into_any_element(),
        };
        column = column.child(el);
    }
    column.into_any_element()
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

    const REAL: &str = "# Summary\n\n## Key Points\n* Beta is ready Friday.\n* Login is fixed.\n\n## Decisions\nNone\n\n## Action Items\n1. Maria reviews invoices.";

    #[test]
    fn book_lines_keep_every_block_and_its_inline_markdown() {
        let lines = lines("## Key\n\n- Ship **one** slider.\n12. Book it.\nThanks.");
        assert_eq!(
            lines,
            [
                Line::Heading("Key"),
                Line::Item {
                    marker: "–",
                    text: "Ship **one** slider."
                },
                Line::Item {
                    marker: "12.",
                    text: "Book it."
                },
                Line::Paragraph("Thanks."),
            ]
        );
        let spoken: Vec<String> = a11y::markdown_blocks("## Key\n\n- Ship **one** slider.")
            .into_iter()
            .map(|b| b.text)
            .collect();
        assert_eq!(
            spoken,
            ["Key", "Ship one slider."],
            "VoiceOver reads the same text"
        );
    }

    #[test]
    fn the_asterism_sits_before_the_second_section_not_under_the_title_heading() {
        let marks = Marks::of(&lines(REAL));
        assert_eq!(
            lines(REAL)[marks.ornament_before.unwrap()],
            Line::Heading("Decisions")
        );
        assert_eq!(
            marks.drop_cap, None,
            "a summary that opens with a list has no drop cap"
        );
        assert_eq!(
            Marks::of(&lines("## Key points\n\n- One")),
            Marks::default()
        );
    }

    #[test]
    fn the_drop_cap_takes_a_long_opening_paragraph_only() {
        let lead = "Onboarding drops from five screens to three. Permissions move to step two, and each one is explained before the macOS prompt appears.";
        let md = format!("{lead}\n\n## Decisions\n- Ship it.");
        let marks = Marks::of(&lines(&md));
        assert_eq!(marks.drop_cap, Some(0));
        assert_eq!(marks.ornament_before, Some(1));
        assert_eq!(
            Marks::of(&lines("Short lead.\n## Next\n- x")).drop_cap,
            None
        );
        assert_eq!(Marks::of(&lines(&format!("**{lead}**"))).drop_cap, None);
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
