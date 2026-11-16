//! System notifications (UX-10, UX-14). Stub until the notify agent's body lands; the record flow
//! already calls each function at the moment the spec names.

use dory_core::domain::RecordingId;
use gpui_kit::{App, Entity};

use crate::app::AppState;

/// A job finished while no Dory window was key. The caller checks the window; this always posts.
pub fn job_finished(_id: &RecordingId, _title: &str, _cx: &mut App) {}

/// Five minutes of recording are left before the cap.
pub fn time_warning(_cx: &mut App) {}

/// The recording reached the cap and was saved.
pub fn cap_reached(_cx: &mut App) {}

/// Clicking a posted notification selects its recording. Called once at launch.
pub fn install(_app: &Entity<AppState>, _cx: &mut App) {}
