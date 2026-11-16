//! Model lists and the meeting languages picker (UX-18, UX-28, UX-29). A stub until the P6
//! settings agent's file lands.

use gpui_kit::{AnyElement, App, Entity, IntoElement as _, Window, div};

use crate::app::AppState;

pub fn speech_step(_app: &Entity<AppState>, _window: &mut Window, _cx: &mut App) -> AnyElement {
    div().into_any_element()
}

pub fn meeting_languages_picker(
    _app: &Entity<AppState>,
    _window: &mut Window,
    _cx: &mut App,
) -> AnyElement {
    div().into_any_element()
}
