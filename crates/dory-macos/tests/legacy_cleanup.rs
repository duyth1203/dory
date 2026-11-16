//! `legacy_cleanup` against a fake HAL. Nothing here touches the machine's real audio devices.

use std::cell::RefCell;
use std::path::Path;

use dory_macos::legacy_cleanup::{
    AGGREGATE_UID, AudioHal, Device, MULTI_OUTPUT_UID, OUTPUT_RESTORE_FILE, Outcome, Paths,
    SESSION_FILE, run_once,
};

const BUILTIN_MIC: u32 = 1;
const BUILTIN_SPEAKERS: u32 = 2;
const HEADPHONES: u32 = 3;
const USB_MIC: u32 = 4;
const AGGREGATE: u32 = 10;
const MULTI_OUTPUT: u32 = 11;
const USER_DORY_INPUT: u32 = 12;

fn device(id: u32, uid: &str, name: &str, input: bool, output: bool, builtin: bool) -> Device {
    Device {
        id,
        uid: uid.into(),
        name: name.into(),
        input,
        output,
        builtin,
    }
}

struct FakeHal {
    devices: RefCell<Vec<Device>>,
    input: RefCell<Option<u32>>,
    output: RefCell<Option<u32>>,
    calls: RefCell<Vec<String>>,
    volumes: RefCell<Vec<(u32, f32)>>,
    fail_destroy: bool,
}

impl FakeHal {
    /// A Mac whose defaults still point at the two legacy devices, plus a user device named like
    /// ours but with another UID.
    fn leftover() -> Self {
        let devices = vec![
            device(
                BUILTIN_MIC,
                "BuiltInMic",
                "MacBook Microphone",
                true,
                false,
                true,
            ),
            device(
                BUILTIN_SPEAKERS,
                "BuiltInSpeakers",
                "MacBook Speakers",
                false,
                true,
                true,
            ),
            device(HEADPHONES, "Headphones", "Headphones", false, true, false),
            device(USB_MIC, "UsbMic", "USB Mic", true, false, false),
            device(AGGREGATE, AGGREGATE_UID, "Dory Input", true, false, false),
            device(
                MULTI_OUTPUT,
                MULTI_OUTPUT_UID,
                "Dory Output",
                false,
                true,
                false,
            ),
            device(
                USER_DORY_INPUT,
                "user-made",
                "Dory Input",
                true,
                false,
                false,
            ),
        ];
        Self {
            devices: RefCell::new(devices),
            input: RefCell::new(Some(AGGREGATE)),
            output: RefCell::new(Some(MULTI_OUTPUT)),
            calls: RefCell::default(),
            volumes: RefCell::default(),
            fail_destroy: false,
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn uids(&self) -> Vec<String> {
        self.devices
            .borrow()
            .iter()
            .map(|d| d.uid.clone())
            .collect()
    }
}

impl AudioHal for FakeHal {
    fn devices(&self) -> Result<Vec<Device>, String> {
        Ok(self.devices.borrow().clone())
    }

    fn default_device(&self, input: bool) -> Option<u32> {
        *(if input { &self.input } else { &self.output }).borrow()
    }

    fn set_default(&self, input: bool, device: u32) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push(format!("set_default input={input} {device}"));
        *(if input { &self.input } else { &self.output }).borrow_mut() = Some(device);
        Ok(())
    }

    fn set_output_volume(&self, device: u32, scalar: f32) -> Result<(), String> {
        self.volumes.borrow_mut().push((device, scalar));
        Ok(())
    }

    fn destroy(&self, device: u32) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("destroy {device}"));
        if self.fail_destroy {
            return Err("busy".into());
        }
        self.devices.borrow_mut().retain(|d| d.id != device);
        Ok(())
    }
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn paths(&self) -> Paths {
        Paths {
            legacy_dir: self.dir.path().join("legacy"),
            marker: self.dir.path().join("v2").join("legacy-audio-cleanup.done"),
        }
    }

    fn write_legacy(&self, name: &str, body: &str) {
        let legacy = self.dir.path().join("legacy");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        std::fs::write(legacy.join(name), body).expect("write restore file");
    }
}

fn run(hal: &FakeHal, env: &Env) -> (Outcome, Vec<String>) {
    let mut log = Vec::new();
    let outcome = run_once(hal, &env.paths(), &mut |step| log.push(step.to_owned()));
    (outcome, log)
}

fn assert_legacy_gone_and_user_device_kept(hal: &FakeHal) {
    let uids = hal.uids();
    assert!(!uids.contains(&AGGREGATE_UID.to_owned()));
    assert!(!uids.contains(&MULTI_OUTPUT_UID.to_owned()));
    assert!(
        uids.contains(&"user-made".to_owned()),
        "the user's own Dory Input survives"
    );
}

#[test]
fn session_file_wins_for_both_defaults_and_restores_volume() {
    let env = Env::new();
    env.write_legacy(
        SESSION_FILE,
        r#"{"output_device_uid":"Headphones","input_device_uid":"UsbMic","output_volume_scalar":0.4}"#,
    );
    env.write_legacy(
        OUTPUT_RESTORE_FILE,
        r#"{"output_device_uid":"BuiltInSpeakers"}"#,
    );
    let hal = FakeHal::leftover();
    let (outcome, log) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(hal.default_device(false), Some(HEADPHONES));
    assert_eq!(hal.default_device(true), Some(USB_MIC));
    assert_eq!(*hal.volumes.borrow(), vec![(HEADPHONES, 0.4)]);
    assert_legacy_gone_and_user_device_kept(&hal);
    assert!(env.paths().marker.exists());
    assert!(
        log.iter().any(|l| l.contains("destroyed")),
        "every step is logged: {log:?}"
    );
}

#[test]
fn output_restore_file_is_used_when_the_session_file_is_missing() {
    let env = Env::new();
    env.write_legacy(OUTPUT_RESTORE_FILE, r#"{"output_device_uid":"Headphones"}"#);
    let hal = FakeHal::leftover();
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(hal.default_device(false), Some(HEADPHONES));
    assert_eq!(
        hal.default_device(true),
        Some(BUILTIN_MIC),
        "input falls to the built-in mic"
    );
    assert!(hal.volumes.borrow().is_empty());
    assert_legacy_gone_and_user_device_kept(&hal);
}

#[test]
fn built_in_devices_are_the_last_resort_with_no_restore_files() {
    let env = Env::new();
    let hal = FakeHal::leftover();
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(hal.default_device(false), Some(BUILTIN_SPEAKERS));
    assert_eq!(hal.default_device(true), Some(BUILTIN_MIC));
    assert_legacy_gone_and_user_device_kept(&hal);
}

#[test]
fn a_restore_file_naming_an_unplugged_device_falls_through() {
    let env = Env::new();
    env.write_legacy(
        SESSION_FILE,
        r#"{"output_device_uid":"Gone","input_device_uid":"AlsoGone"}"#,
    );
    env.write_legacy(OUTPUT_RESTORE_FILE, r#"{"output_device_uid":"Headphones"}"#);
    let hal = FakeHal::leftover();
    run(&hal, &env);
    assert_eq!(hal.default_device(false), Some(HEADPHONES));
    assert_eq!(hal.default_device(true), Some(BUILTIN_MIC));
}

#[test]
fn a_corrupt_restore_file_is_ignored() {
    let env = Env::new();
    env.write_legacy(SESSION_FILE, "not json");
    let hal = FakeHal::leftover();
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(hal.default_device(false), Some(BUILTIN_SPEAKERS));
}

#[test]
fn a_restore_file_never_points_a_default_at_a_legacy_device() {
    let env = Env::new();
    env.write_legacy(
        SESSION_FILE,
        &format!(
            r#"{{"output_device_uid":"{MULTI_OUTPUT_UID}","input_device_uid":"{AGGREGATE_UID}"}}"#
        ),
    );
    let hal = FakeHal::leftover();
    run(&hal, &env);
    assert_eq!(hal.default_device(false), Some(BUILTIN_SPEAKERS));
    assert_eq!(hal.default_device(true), Some(BUILTIN_MIC));
}

#[test]
fn defaults_the_user_chose_are_left_alone() {
    let env = Env::new();
    let hal = FakeHal::leftover();
    *hal.input.borrow_mut() = Some(USB_MIC);
    *hal.output.borrow_mut() = Some(HEADPHONES);
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Done);
    assert!(!hal.calls().iter().any(|c| c.starts_with("set_default")));
    assert_eq!(hal.default_device(false), Some(HEADPHONES));
    assert_legacy_gone_and_user_device_kept(&hal);
}

#[test]
fn a_device_with_the_same_name_and_another_uid_is_never_touched() {
    let env = Env::new();
    let hal = FakeHal::leftover();
    hal.devices
        .borrow_mut()
        .retain(|d| d.uid != AGGREGATE_UID && d.uid != MULTI_OUTPUT_UID);
    *hal.input.borrow_mut() = Some(USER_DORY_INPUT);
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Done);
    assert!(
        hal.calls().is_empty(),
        "no legacy device, so no call: {:?}",
        hal.calls()
    );
    assert_eq!(hal.default_device(true), Some(USER_DORY_INPUT));
}

#[test]
fn a_second_run_does_nothing() {
    let env = Env::new();
    let hal = FakeHal::leftover();
    assert_eq!(run(&hal, &env).0, Outcome::Done);
    let calls = hal.calls();
    // Even with the devices back, the marker stops a second run.
    *hal.devices.borrow_mut() = FakeHal::leftover().devices.take();
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::AlreadyDone);
    assert_eq!(hal.calls(), calls);
}

#[test]
fn a_failed_destroy_leaves_no_marker_so_the_next_launch_retries() {
    let env = Env::new();
    let mut hal = FakeHal::leftover();
    hal.fail_destroy = true;
    let (outcome, _) = run(&hal, &env);
    assert_eq!(outcome, Outcome::Incomplete);
    assert!(!env.paths().marker.exists());
    hal.fail_destroy = false;
    assert_eq!(run(&hal, &env).0, Outcome::Done);
    assert_legacy_gone_and_user_device_kept(&hal);
}

#[test]
fn nothing_to_clean_still_writes_the_marker() {
    let env = Env::new();
    let hal = FakeHal::leftover();
    hal.devices
        .borrow_mut()
        .retain(|d| d.uid != AGGREGATE_UID && d.uid != MULTI_OUTPUT_UID);
    assert_eq!(run(&hal, &env).0, Outcome::Done);
    assert!(Path::new(&env.paths().marker).exists());
}
