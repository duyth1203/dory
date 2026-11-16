//! The `Machine` port on macOS: the power source through IOKit, Low Power Mode and the thermal
//! state through `NSProcessInfo`, and the pace as the calling thread's QoS class.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use dory_core::ports::{Conditions, Machine, Pace, Power, Thermal};
use objc2_core_foundation::{CFRetained, CFString, CFType};
use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};

/// `qos_class_t` from `<sys/qos.h>`.
type QosClass = u32;
const QOS_CLASS_DEFAULT: QosClass = 0x15;
const QOS_CLASS_BACKGROUND: QosClass = 0x09;
const QOS_CLASS_UNSPECIFIED: QosClass = 0x00;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// Returns a +1 snapshot, or null.
    fn IOPSCopyPowerSourcesInfo() -> *mut c_void;
    /// Returns a string the snapshot owns ("AC Power", "Battery Power", or "UPS Power"), or null.
    fn IOPSGetProvidingPowerSourceType(snapshot: *const c_void) -> *const c_void;
}

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: QosClass, relative_priority: i32) -> i32;
    fn qos_class_self() -> QosClass;
}

thread_local! {
    /// The QoS this thread had before its first `Light`, so `Fast` can put it back.
    static BEFORE_LIGHT: Cell<Option<QosClass>> = const { Cell::new(None) };
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

    /// Background QoS keeps the calling thread on the efficiency cores at a low I/O priority.
    /// Threads it creates afterwards do not inherit it: on macOS 26 a `pthread_create` child of a
    /// background thread starts at DEFAULT, so an engine's own thread pool is not slowed by this.
    fn set_pace(&self, pace: Pace) {
        match pace {
            Pace::Light => {
                if BEFORE_LIGHT.get().is_none() {
                    // SAFETY: reads the calling thread's QoS, with no arguments and no failure.
                    BEFORE_LIGHT.set(Some(unsafe { qos_class_self() }));
                }
                // SAFETY: a valid class for the calling thread. A thread opted out of QoS returns
                // EPERM and keeps running as it was, which is the most this can do.
                unsafe { pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0) };
            }
            Pace::Fast => {
                if let Some(before) = BEFORE_LIGHT.take() {
                    // UNSPECIFIED cannot be requested; DEFAULT is how macOS schedules it.
                    let class = match before {
                        QOS_CLASS_UNSPECIFIED => QOS_CLASS_DEFAULT,
                        class => class,
                    };
                    // SAFETY: as above, with a class this thread already had or DEFAULT.
                    unsafe { pthread_set_qos_class_self_np(class, 0) };
                }
            }
        }
    }
}

/// The calling thread's QoS class, for checking that a pace reached it.
pub fn thread_qos() -> u32 {
    // SAFETY: as in `set_pace`.
    unsafe { qos_class_self() }
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
    fn light_moves_the_thread_to_background_and_fast_puts_it_back() {
        std::thread::spawn(|| {
            let machine = MacMachine::new();
            let before = thread_qos();
            machine.set_pace(Pace::Light);
            assert_eq!(thread_qos(), QOS_CLASS_BACKGROUND);
            machine.set_pace(Pace::Light);
            machine.set_pace(Pace::Fast);
            let restored = match before {
                QOS_CLASS_UNSPECIFIED => QOS_CLASS_DEFAULT,
                class => class,
            };
            assert_eq!(thread_qos(), restored);
            machine.set_pace(Pace::Fast);
            assert_eq!(thread_qos(), restored, "fast twice changes nothing");
        })
        .join()
        .unwrap();
    }

    #[test]
    fn conditions_read_without_prompting() {
        let now = MacMachine::new().conditions();
        assert_ne!(now.power, Power::Unknown, "a Mac reports its power source");
    }
}
