//! Headless driver and test bench for Dory. Commands go through the `dory` composition root, so
//! this crate names no adapter, except in the macOS capture harness (see `capture.rs`).

mod args;
#[cfg(target_os = "macos")]
mod capture;
mod library;
mod process;
mod record;
#[cfg(target_os = "macos")]
mod tone;
mod view;
mod wait;

use std::process::ExitCode;

use dory::{Config, Dory};

const USAGE: &str = "usage:
  dory-cli import OLD_DATA_DIR
  dory-cli list [--search TEXT]
  dory-cli show ID
  dory-cli record [--layout mic-system|mic|system] [--mic DEVICE_UID] [--fake-capture SECONDS]
  dory-cli process ID [--fake-engines]
  dory-cli models [--fake-engines]
  dory-cli capture --seconds N --out FILE.wav [--layout mic-system|mic|system] [--rate HZ] [--mic DEVICE_UID]
  dory-cli devices

The data folder is $DORY_DATA_DIR, else ~/Library/Application Support/com.dory.app/v2.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result =
        match args.first().map(String::as_str) {
            Some("import") => args::ImportArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::import(&c, &a))),
            Some("list") => args::ListArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::list(&c, &a))),
            Some("show") => args::ShowArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::show(&c, &a))),
            Some("models") => args::ModelsArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::models(&c, &a))),
            Some("record") => args::RecordArgs::parse(rest)
                .and_then(|a| config().and_then(|c| record::run(&c, &a))),
            Some("process") => args::ProcessArgs::parse(rest)
                .and_then(|a| config().and_then(|c| process::run(&c, &a))),
            Some("capture") => args::CaptureArgs::parse(rest).and_then(|a| platform::capture(&a)),
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

fn config() -> Result<Config, String> {
    Config::from_env().map_err(|e| e.to_string())
}

fn open(config: &Config, fake_engines: bool) -> Result<Dory, String> {
    match fake_engines {
        true => dory::open_with_fake_engines(config),
        false => dory::open(config),
    }
    .map_err(chain)
}

/// An error and its causes on one line.
fn chain(error: impl std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
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
