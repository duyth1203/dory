//! Headless driver and test bench for Dory.

mod args;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod tone;

use std::process::ExitCode;

const USAGE: &str = "usage:
  dory-cli capture --seconds N --out FILE.wav [--layout mic-system|mic|system] [--rate HZ] [--mic DEVICE_UID]
  dory-cli devices";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("capture") => args::CaptureArgs::parse(&args[1..]).and_then(|a| platform::capture(&a)),
        Some("devices") => platform::devices(),
        _ => Err(USAGE.to_string()),
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
        Err("capture is only implemented on macOS".into())
    }
    pub fn devices() -> Result<(), String> {
        Err("devices is only implemented on macOS".into())
    }
}
