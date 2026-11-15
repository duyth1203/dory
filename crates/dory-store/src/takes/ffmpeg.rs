//! Legacy WebM and unknown formats go through `ffmpeg`, found from `DORY_FFMPEG`, then `PATH`,
//! then the Homebrew location. A candidate counts only if it runs: a Homebrew ffmpeg whose
//! dependency was upgraded away still sits on `PATH` but dies on launch.

use std::path::{Path, PathBuf};
use std::process::Command;

use dory_core::ports::TakeError;

fn candidates() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(explicit) = std::env::var_os("DORY_FFMPEG").map(PathBuf::from) {
        found.push(explicit);
    }
    if let Some(path) = std::env::var_os("PATH") {
        found.extend(std::env::split_paths(&path).map(|dir| dir.join("ffmpeg")));
    }
    found.push(PathBuf::from("/opt/homebrew/bin/ffmpeg"));
    found.retain(|p| p.is_file());
    found.dedup();
    found
}

/// The first `ffmpeg` that starts. `Err` says why none did.
pub(crate) fn find_ffmpeg() -> Result<PathBuf, String> {
    let mut broken = None;
    for candidate in candidates() {
        match Command::new(&candidate).arg("-version").output() {
            Ok(out) if out.status.success() => return Ok(candidate),
            Ok(out) => {
                let why = String::from_utf8_lossy(&out.stderr);
                let first = why.lines().next().unwrap_or("it exited with an error");
                broken.get_or_insert(format!("{} does not run: {first}", candidate.display()));
            }
            Err(e) => {
                broken.get_or_insert(format!("{} does not start: {e}", candidate.display()));
            }
        }
    }
    Err(broken.unwrap_or_else(|| {
        "ffmpeg not found (set DORY_FFMPEG, or install it with `brew install ffmpeg`)".to_owned()
    }))
}

/// Decodes `input` to a 16-bit mono WAV at the file's own sample rate.
pub(crate) fn decode_to_wav(input: &Path, output: &Path) -> Result<(), TakeError> {
    let ffmpeg = find_ffmpeg().map_err(TakeError::ConverterMissing)?;
    let result = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-c:a", "pcm_s16le"])
        .arg(output)
        .output()
        .map_err(super::io_err)?;
    if result.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        Err(TakeError::Decode(format!(
            "ffmpeg could not decode the file: {}",
            stderr.trim()
        )))
    }
}
