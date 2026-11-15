//! Capture harness: records through `CoreAudioCapture` directly into a WAV file and reports what
//! it saw. It lives with the adapter because it needs the device graph, the timeline clock, and a
//! forced rebuild, none of which the `CaptureBackend` port exposes.
//!
//! ```text
//! cargo run -p dory-macos --example dory-capture -- --seconds N --out FILE.wav \
//!     [--layout mic-system|mic|system] [--rate HZ] [--mic DEVICE_UID] [--self-tone] [--rebuild-at S]
//! cargo run -p dory-macos --example dory-capture -- devices
//! ```

mod args;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod tone;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("devices") => platform::devices(),
        _ => args::CaptureArgs::parse(&args).and_then(|a| platform::capture(&a)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub use crate::capture::{devices, run as capture};
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn capture(_: &crate::args::CaptureArgs) -> Result<(), String> {
        Err("the capture harness runs only on macOS".into())
    }
    pub fn devices() -> Result<(), String> {
        Err("the capture harness runs only on macOS".into())
    }
}
