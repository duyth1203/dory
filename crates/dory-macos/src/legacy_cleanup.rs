//! One-time cleanup of the audio devices the old app left on the machine.
//!
//! The old app built an aggregate device and a multi-output device with fixed UIDs and switched
//! the system defaults to them while it recorded. If it quit badly, the defaults can still point
//! at them. This module puts the defaults back and destroys both devices.
//!
//! Devices are found by UID only. A device a user named "Dory Input" with another UID is theirs
//! and is never touched. The HAL sits behind [`AudioHal`], so tests drive a fake device list.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const AGGREGATE_UID: &str = "com.dory.aggregate";
pub const MULTI_OUTPUT_UID: &str = "com.dory.multioutput";
pub const SESSION_FILE: &str = "audio_recording_session.json";
pub const OUTPUT_RESTORE_FILE: &str = "audio_output_restore.json";

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: u32,
    pub uid: String,
    pub name: String,
    pub input: bool,
    pub output: bool,
    pub builtin: bool,
}

impl Device {
    fn is_legacy(&self) -> bool {
        self.uid == AGGREGATE_UID || self.uid == MULTI_OUTPUT_UID
    }
}

pub trait AudioHal {
    fn devices(&self) -> Result<Vec<Device>, String>;
    fn default_device(&self, input: bool) -> Option<u32>;
    fn set_default(&self, input: bool, device: u32) -> Result<(), String>;
    fn set_output_volume(&self, device: u32, scalar: f32) -> Result<(), String>;
    fn destroy(&self, device: u32) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct Paths {
    /// The old app's data folder, where it kept the two restore files. Read only.
    pub legacy_dir: PathBuf,
    /// Written after a complete run, so the cleanup never runs twice.
    pub marker: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    AlreadyDone,
    Done,
    /// A step failed. No marker is written, so the next launch tries again.
    Incomplete,
}

#[derive(Deserialize)]
struct Session {
    output_device_uid: Option<String>,
    input_device_uid: Option<String>,
    output_volume_scalar: Option<f32>,
}

#[derive(Deserialize)]
struct OutputRestore {
    output_device_uid: Option<String>,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, log: &mut dyn FnMut(&str)) -> Option<T> {
    let Ok(text) = std::fs::read_to_string(path) else {
        log(&format!("{} is not there", path.display()));
        return None;
    };
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(error) => {
            log(&format!("{} is unreadable: {error}", path.display()));
            None
        }
    }
}

pub fn run_once(hal: &dyn AudioHal, paths: &Paths, log: &mut dyn FnMut(&str)) -> Outcome {
    if paths.marker.exists() {
        log("already done on an earlier launch");
        return Outcome::AlreadyDone;
    }
    let devices = match hal.devices() {
        Ok(devices) => devices,
        Err(error) => {
            log(&format!("could not list audio devices: {error}"));
            return Outcome::Incomplete;
        }
    };
    let legacy: Vec<&Device> = devices.iter().filter(|d| d.is_legacy()).collect();
    if legacy.is_empty() {
        log("no legacy Dory audio devices found");
        return finish(paths, true, log);
    }
    for device in &legacy {
        log(&format!(
            "found legacy device {} (uid {}, id {})",
            device.name, device.uid, device.id
        ));
    }

    let session: Option<Session> = read_json(&paths.legacy_dir.join(SESSION_FILE), log);
    let output_restore: Option<OutputRestore> =
        read_json(&paths.legacy_dir.join(OUTPUT_RESTORE_FILE), log);
    let mut ok = true;

    for input in [false, true] {
        let side = if input { "input" } else { "output" };
        let Some(current) = hal.default_device(input) else {
            log(&format!("no default {side} device"));
            continue;
        };
        if !legacy.iter().any(|d| d.id == current) {
            log(&format!(
                "default {side} (id {current}) is not a legacy device, leaving it"
            ));
            continue;
        }
        let from_session = session.as_ref().and_then(|s| {
            if input {
                s.input_device_uid.as_deref()
            } else {
                s.output_device_uid.as_deref()
            }
        });
        let mut wanted = vec![(SESSION_FILE, from_session)];
        if !input {
            wanted.push((
                OUTPUT_RESTORE_FILE,
                output_restore
                    .as_ref()
                    .and_then(|r| r.output_device_uid.as_deref()),
            ));
        }
        let usable = |d: &&Device| !d.is_legacy() && if input { d.input } else { d.output };
        let mut target: Option<(&Device, &str)> = None;
        for (source, uid) in wanted {
            let Some(uid) = uid else { continue };
            match devices.iter().filter(usable).find(|d| d.uid == uid) {
                Some(device) => {
                    target = Some((device, source));
                    break;
                }
                None => log(&format!("{source} names {uid}, which is not present")),
            }
        }
        if target.is_none() {
            target = devices
                .iter()
                .filter(usable)
                .find(|d| d.builtin)
                .map(|d| (d, "the built-in device"));
        }
        let Some((device, source)) = target else {
            log(&format!("no device to restore the default {side} to"));
            ok = false;
            continue;
        };
        match hal.set_default(input, device.id) {
            Ok(()) => log(&format!(
                "restored default {side} to {} (uid {}) from {source}",
                device.name, device.uid
            )),
            Err(error) => {
                log(&format!("could not restore default {side}: {error}"));
                ok = false;
                continue;
            }
        }
        if !input
            && source == SESSION_FILE
            && let Some(scalar) = session.as_ref().and_then(|s| s.output_volume_scalar)
        {
            match hal.set_output_volume(device.id, scalar) {
                Ok(()) => log(&format!("restored output volume {scalar}")),
                Err(error) => log(&format!("could not restore output volume: {error}")),
            }
        }
    }

    for device in &legacy {
        match hal.destroy(device.id) {
            Ok(()) => log(&format!("destroyed {} (uid {})", device.name, device.uid)),
            Err(error) => {
                log(&format!("could not destroy {}: {error}", device.uid));
                ok = false;
            }
        }
    }
    finish(paths, ok, log)
}

fn finish(paths: &Paths, ok: bool, log: &mut dyn FnMut(&str)) -> Outcome {
    if !ok {
        log("not finished, will try again at the next launch");
        return Outcome::Incomplete;
    }
    let written = paths
        .marker
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&paths.marker, "done\n"));
    match written {
        Ok(()) => {
            log("done");
            Outcome::Done
        }
        Err(error) => {
            log(&format!("could not write the marker: {error}"));
            Outcome::Incomplete
        }
    }
}

#[cfg(target_os = "macos")]
mod core_audio {
    use super::{AudioHal, Device};
    use crate::capture::hal;

    pub struct CoreAudioHal;

    impl AudioHal for CoreAudioHal {
        fn devices(&self) -> Result<Vec<Device>, String> {
            let ids = hal::all_devices().map_err(|e| e.to_string())?;
            Ok(ids
                .into_iter()
                .filter_map(|id| {
                    let uid = hal::device_uid(id).ok()?;
                    Some(Device {
                        id,
                        name: hal::object_name(id).unwrap_or_default(),
                        input: hal::has_channels(id, hal::INPUT),
                        output: hal::has_channels(id, hal::OUTPUT),
                        builtin: hal::is_builtin(id),
                        uid,
                    })
                })
                .collect())
        }

        fn default_device(&self, input: bool) -> Option<u32> {
            hal::default_device(input).ok().flatten()
        }

        fn set_default(&self, input: bool, device: u32) -> Result<(), String> {
            hal::set_default_device(input, device).map_err(|e| e.to_string())
        }

        fn set_output_volume(&self, device: u32, scalar: f32) -> Result<(), String> {
            hal::set_output_volume(device, scalar).map_err(|e| e.to_string())
        }

        fn destroy(&self, device: u32) -> Result<(), String> {
            hal::destroy_aggregate(device).map_err(|e| e.to_string())
        }
    }

    /// Read-only: the legacy UIDs that exist on this machine.
    pub fn legacy_uids_present() -> Result<Vec<String>, String> {
        Ok(CoreAudioHal
            .devices()?
            .into_iter()
            .filter(|d| d.is_legacy())
            .map(|d| d.uid)
            .collect())
    }
}

/// Runs [`run_once`] against the real HAL and prints every step to stderr.
#[cfg(target_os = "macos")]
pub fn run_at_launch(paths: &Paths) -> Outcome {
    run_once(&core_audio::CoreAudioHal, paths, &mut |step| {
        eprintln!("legacy-cleanup: {step}");
    })
}

/// Read-only. Lists the legacy UIDs the real HAL reports, so a check can show there are none.
#[cfg(target_os = "macos")]
pub fn legacy_uids_present() -> Result<Vec<String>, String> {
    core_audio::legacy_uids_present()
}
