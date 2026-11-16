//! The Set up summaries sheet (UX-13). Summaries need Ollama and the Gemma model, so the sheet
//! says so and offers the three commands to copy. P6 replaces it with a one-click pull.

use gpui_kit::component::{WindowExt as _, dialog::DialogFooter};
use gpui_kit::{
    App, ClipboardItem, FocusHandle, ParentElement as _, Role, Styled as _, Window, div, px,
};

use crate::a11y;
use crate::queue_popover::pill_button;
use crate::theme::Theme;

/// What the user runs, in order.
pub const COMMANDS: [&str; 3] = [
    "brew install ollama",
    "ollama serve",
    "ollama pull gemma4:e4b",
];

pub const EXPLANATION: &str = "Dory writes summaries with Gemma, which runs on this Mac through Ollama. Run these three commands in Terminal, then try the summary again.";

pub fn sheet(window: &mut Window, cx: &mut App) {
    let copy_focus: Vec<FocusHandle> = COMMANDS.iter().map(|_| cx.focus_handle()).collect();
    let done_focus = cx.focus_handle();
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = Theme::get(cx);
        let mut body = a11y::node("set-up-summaries", Role::Group, "Set up summaries")
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(theme.text(13.))
                    .text_color(theme.text_secondary)
                    .child(EXPLANATION),
            );
        for (command, focus) in COMMANDS.into_iter().zip(&copy_focus) {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        a11y::node(
                            format!("command-{command}"),
                            Role::Label,
                            format!("Command: {command}"),
                        )
                        .flex_1()
                        .min_w_0()
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .bg(theme.field_bg)
                        .font_family("Menlo")
                        .text_size(theme.text(13.))
                        .child(command),
                    )
                    .child(pill_button(
                        format!("copy-{command}"),
                        format!("Copy {command}"),
                        "Copy",
                        focus,
                        &theme,
                        move |_, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(command.into()))
                        },
                    )),
            );
        }
        let done = pill_button(
            "set-up-done",
            "Done",
            "Done",
            &done_focus,
            &theme,
            |window, cx| window.close_dialog(cx),
        );
        dialog
            .title("Set up summaries")
            .w(px(520.))
            .child(body)
            .footer(DialogFooter::new().child(done))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_are_the_three_the_plan_names_in_order() {
        assert_eq!(
            COMMANDS,
            [
                "brew install ollama",
                "ollama serve",
                "ollama pull gemma4:e4b"
            ]
        );
    }
}
