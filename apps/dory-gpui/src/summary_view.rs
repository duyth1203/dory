//! The Summary tab (UX-4). Stub for agent 1, who owns this file together with `detail.rs`.
//!
//! Agent 1 builds, without editing any other file:
//! - Rendered Markdown at 15/1.6 with 17 pt semibold section headings, from `Detail.summary`.
//!   Kit's `TextView::markdown` draws it; its AX nodes come from `a11y::with_markdown_children`
//!   (Kit emits none, spike finding 3). If a block walk of `TextView`'s own AST replaces the
//!   line parser in `a11y::markdown_blocks`, keep that function's signature.
//! - A stale summary (`Summary::is_stale_for` the current transcript revision) says so above
//!   the text. No summary says why, using the recording's job state from `AppState::status`.
//! - Copy and Export sit in the tab row, which `detail.rs` draws; they dispatch
//!   `crate::actions::CopySummary` and the `Export*` actions.

use gpui_kit::component::text::TextView;
use gpui_kit::{AnyElement, IntoElement as _, ParentElement as _, Role, Styled as _};

use crate::a11y;
use crate::app::Detail;
use crate::theme::Theme;

pub fn summary(detail: &Detail, theme: &Theme) -> AnyElement {
    let Some(summary) = &detail.summary else {
        return a11y::node("summary-document", Role::Document, "Summary")
            .text_color(theme.text_secondary)
            .child("No summary yet.")
            .into_any_element();
    };
    a11y::with_markdown_children(
        a11y::node("summary-document", Role::Document, "Summary"),
        &summary.markdown,
    )
    .text_size(theme.text(15.))
    .child(TextView::markdown(
        "summary-markdown",
        summary.markdown.clone(),
    ))
    .into_any_element()
}
