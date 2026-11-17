//! Asks the user before an assistant changes anything. Each request arrives from the control
//! socket with its question already written by the core, and shows as a native alert on Dory's
//! window: Allow or Don't Allow. With no window open there is nobody to ask, so the request is
//! answered as unseen and the assistant is told to have the user open Dory.

use dory_core::app::ConsentRequest;
use dory_core::domain::ConsentAnswer;
use gpui_kit::{App, Global, PromptLevel};

/// Don't Allow comes first, so it is the default button and a stray Return never approves a
/// change an assistant asked for.
const ALLOW: usize = 1;

/// Keeps the control socket served for as long as the app runs.
struct Serving {
    _server: Option<dory::AssistantServer>,
}

impl Global for Serving {}

pub fn install(
    server: Option<dory::AssistantServer>,
    requests: async_channel::Receiver<ConsentRequest>,
    cx: &mut App,
) {
    cx.set_global(Serving { _server: server });
    cx.spawn(async move |cx| {
        while let Ok(request) = requests.recv().await {
            let asked = cx.update(|cx| {
                let window = cx.windows().first().copied()?;
                cx.activate(true);
                window
                    .update(cx, |_, window, cx| {
                        window.activate_window();
                        window.prompt(
                            PromptLevel::Warning,
                            &request.question.message,
                            Some(&request.question.detail),
                            &["Don't Allow", "Allow"],
                            cx,
                        )
                    })
                    .ok()
            });
            let answer = match asked {
                Some(answer) => match answer.await {
                    Ok(ALLOW) => ConsentAnswer::Allow,
                    _ => ConsentAnswer::Deny,
                },
                None => ConsentAnswer::NoWindow,
            };
            request.answer(answer);
        }
    })
    .detach();
}
