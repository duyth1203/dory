//! macOS 26 adapters.

#[cfg(target_os = "macos")]
mod capture;

pub mod legacy_cleanup;

#[cfg(target_os = "macos")]
mod machine;

#[cfg(target_os = "macos")]
mod playback;

#[cfg(target_os = "macos")]
mod trash;

#[cfg(target_os = "macos")]
pub use capture::{CaptureGraph, CoreAudioCapture, CoreAudioHandle, PcmFormat, TimelineClock};
#[cfg(target_os = "macos")]
pub use machine::MacMachine;
#[cfg(target_os = "macos")]
pub use playback::AvPlayback;
#[cfg(target_os = "macos")]
pub use trash::MacTrash;
