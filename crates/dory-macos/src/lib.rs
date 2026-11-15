//! macOS 26 adapters.

#[cfg(target_os = "macos")]
mod capture;

#[cfg(target_os = "macos")]
pub use capture::{CaptureGraph, CoreAudioCapture, CoreAudioHandle, PcmFormat, TimelineClock};
