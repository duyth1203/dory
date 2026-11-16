//! The `Machine` port on macOS: the power source through IOKit, Low Power Mode and the thermal
//! state through `NSProcessInfo`.

use std::ffi::c_void;
use std::ptr::NonNull;

use dory_core::ports::{Conditions, Machine, Power, Thermal};
use objc2_core_foundation::{CFRetained, CFString, CFType};
use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// Returns a +1 snapshot, or null.
    fn IOPSCopyPowerSourcesInfo() -> *mut c_void;
    /// Returns a string the snapshot owns ("AC Power", "Battery Power", or "UPS Power"), or null.
    fn IOPSGetProvidingPowerSourceType(snapshot: *const c_void) -> *const c_void;
}

/// Reads conditions on every call, so a change shows at the next window.
#[derive(Debug, Default)]
pub struct MacMachine;

impl MacMachine {
    pub fn new() -> Self {
        Self
    }
}

impl Machine for MacMachine {
    fn conditions(&self) -> Conditions {
        let info = NSProcessInfo::processInfo();
        Conditions {
            power: power_source(),
            low_power: info.isLowPowerModeEnabled(),
            thermal: match info.thermalState() {
                NSProcessInfoThermalState::Nominal => Thermal::Nominal,
                NSProcessInfoThermalState::Fair => Thermal::Fair,
                NSProcessInfoThermalState::Serious => Thermal::Serious,
                _ => Thermal::Critical,
            },
        }
    }
}

fn power_source() -> Power {
    // SAFETY: `IOPSCopyPowerSourcesInfo` follows the Create rule, so the non-null snapshot is +1
    // and `CFRetained::from_raw` takes that reference and releases it on drop.
    let Some(snapshot) = NonNull::new(unsafe { IOPSCopyPowerSourcesInfo() })
        .map(|p| unsafe { CFRetained::from_raw(p.cast::<CFType>()) })
    else {
        return Power::Unknown;
    };
    let snapshot_ptr: *const CFType = &*snapshot;
    // SAFETY: the snapshot is alive for the whole call. The result follows the Get rule, so it is
    // borrowed from the snapshot and only read while `snapshot` lives.
    let kind = unsafe { IOPSGetProvidingPowerSourceType(snapshot_ptr.cast()) };
    let Some(kind) = NonNull::new(kind.cast_mut().cast::<CFString>()) else {
        return Power::Unknown;
    };
    // SAFETY: a valid CFString borrowed from `snapshot`, which outlives this reference.
    let kind = unsafe { kind.as_ref() }.to_string();
    drop(snapshot);
    match kind.as_str() {
        "AC Power" => Power::Ac,
        "Battery Power" | "UPS Power" => Power::Battery,
        _ => Power::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_read_without_prompting() {
        let now = MacMachine::new().conditions();
        assert_ne!(now.power, Power::Unknown, "a Mac reports its power source");
    }
}
