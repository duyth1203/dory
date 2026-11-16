//! Thin, safe wrappers over the Core Audio HAL calls the adapter needs. Every created object is an
//! RAII guard, so a failed `start` and a dropped handle both tear down in the right order.

use std::ffi::{CStr, c_void};
use std::mem::{MaybeUninit, size_of};
use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_core_audio::{
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectAddPropertyListenerBlock, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertyScope, AudioObjectPropertySelector, AudioObjectRemovePropertyListenerBlock,
    AudioObjectSetPropertyData, CATapDescription, CATapMuteBehavior, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyIOProcStreamUsage, kAudioDevicePropertyStreamConfiguration,
    kAudioDevicePropertyStreams, kAudioDevicePropertyTransportType,
    kAudioDevicePropertyVolumeScalar, kAudioDeviceTransportTypeBuiltIn,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioHardwarePropertyProcessObjectList,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioObjectUnknown,
    kAudioProcessPropertyIsRunningOutput, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType, Type};
use objc2_foundation::{NSArray, NSNumber, NSString, NSUUID};

use super::dsp::BufferShape;

pub type OsStatus = i32;

pub const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
pub const GLOBAL: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal;
pub const INPUT: AudioObjectPropertyScope = kAudioObjectPropertyScopeInput;
pub const OUTPUT: AudioObjectPropertyScope = kAudioObjectPropertyScopeOutput;

/// A failed HAL call: what was attempted and the four-char status.
#[derive(Debug)]
pub struct HalError {
    pub what: String,
    pub status: OsStatus,
}

impl std::fmt::Display for HalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = self.status.to_be_bytes();
        if bytes.iter().all(|b| b.is_ascii_graphic()) {
            write!(
                f,
                "{} failed: '{}' ({})",
                self.what,
                String::from_utf8_lossy(&bytes),
                self.status
            )
        } else {
            write!(f, "{} failed: {}", self.what, self.status)
        }
    }
}

impl std::error::Error for HalError {}

pub fn check(status: OsStatus, what: impl FnOnce() -> String) -> Result<(), HalError> {
    if status == 0 {
        Ok(())
    } else {
        Err(HalError {
            what: what(),
            status,
        })
    }
}

fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn fourcc(selector: AudioObjectPropertySelector) -> String {
    String::from_utf8_lossy(&selector.to_be_bytes()).into_owned()
}

/// Reads a fixed-size property. `T` must be the plain C type the HAL documents for `selector`.
fn get_with<T: Copy, Q>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    qualifier: Option<&Q>,
) -> Result<T, HalError> {
    let addr = address(selector, scope);
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    let (q_size, q_ptr) = match qualifier {
        Some(q) => (size_of::<Q>() as u32, (q as *const Q).cast::<c_void>()),
        None => (0, std::ptr::null()),
    };
    // SAFETY: `value` is a writable buffer of `size` bytes, and the qualifier lives for the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            q_size,
            q_ptr,
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast()),
        )
    };
    check(status, || {
        format!("get '{}' on object {object}", fourcc(selector))
    })?;
    // SAFETY: the HAL filled the value, and a zeroed `T` is valid for the plain C types used here.
    Ok(unsafe { value.assume_init() })
}

pub fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<T, HalError> {
    get_with::<T, ()>(object, selector, scope, None)
}

/// Reads a variable-size property into 8-byte aligned storage.
fn get_bytes(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<Vec<u64>, HalError> {
    let addr = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: valid address and size pointers, no qualifier.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check(status, || {
        format!("size of '{}' on object {object}", fourcc(selector))
    })?;
    let mut storage = vec![0u64; (size as usize).div_ceil(8).max(1)];
    // SAFETY: `storage` holds at least `size` writable bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(storage.as_mut_ptr().cast()),
        )
    };
    check(status, || {
        format!("get '{}' on object {object}", fourcc(selector))
    })?;
    storage.truncate((size as usize).div_ceil(8));
    Ok(storage)
}

fn get_ids(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<Vec<AudioObjectID>, HalError> {
    let raw = get_bytes(object, selector, GLOBAL)?;
    let bytes: &[u8] = as_bytes(&raw);
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_ne_bytes(*c))
        .collect())
}

fn as_bytes(words: &[u64]) -> &[u8] {
    // SAFETY: u64 storage reinterpreted as bytes of the same total length.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), words.len() * 8) }
}

pub fn get_string(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<String, HalError> {
    let raw: *const CFString = get(object, selector, GLOBAL)?;
    let raw = NonNull::new(raw.cast_mut()).ok_or_else(|| HalError {
        what: format!("'{}' on object {object} is null", fourcc(selector)),
        status: 0,
    })?;
    // SAFETY: CFString-valued HAL properties are returned retained (+1), so we take ownership.
    let string = unsafe { CFRetained::from_raw(raw) };
    Ok(string.to_string())
}

/// Channel count of each buffer the device exposes in `scope`.
pub fn buffer_shapes(
    device: AudioObjectID,
    scope: AudioObjectPropertyScope,
) -> Result<Vec<BufferShape>, HalError> {
    let raw = get_bytes(device, kAudioDevicePropertyStreamConfiguration, scope)?;
    let list = raw.as_ptr().cast::<AudioBufferList>();
    // SAFETY: the HAL wrote an AudioBufferList with `mNumberBuffers` trailing AudioBuffers into `raw`.
    let shapes = unsafe {
        let count = (*list).mNumberBuffers as usize;
        let first = std::ptr::addr_of!((*list).mBuffers).cast::<AudioBuffer>();
        (0..count)
            .map(|i| BufferShape {
                channels: (*first.add(i)).mNumberChannels as usize,
            })
            .collect()
    };
    Ok(shapes)
}

pub fn device_uid(device: AudioObjectID) -> Result<String, HalError> {
    get_string(device, kAudioDevicePropertyDeviceUID)
}

pub fn object_name(object: AudioObjectID) -> Result<String, HalError> {
    get_string(object, kAudioObjectPropertyName)
}

pub fn default_device(input: bool) -> Result<Option<AudioObjectID>, HalError> {
    let selector = if input {
        kAudioHardwarePropertyDefaultInputDevice
    } else {
        kAudioHardwarePropertyDefaultOutputDevice
    };
    let id: AudioObjectID = get(SYSTEM, selector, GLOBAL)?;
    Ok((id != kAudioObjectUnknown).then_some(id))
}

pub fn all_devices() -> Result<Vec<AudioObjectID>, HalError> {
    get_ids(SYSTEM, kAudioHardwarePropertyDevices)
}

/// Whether the device has at least one channel in `scope`.
pub fn has_channels(device: AudioObjectID, scope: AudioObjectPropertyScope) -> bool {
    buffer_shapes(device, scope).is_ok_and(|shapes| shapes.iter().any(|s| s.channels > 0))
}

pub fn is_builtin(device: AudioObjectID) -> bool {
    get::<u32>(device, kAudioDevicePropertyTransportType, GLOBAL)
        .is_ok_and(|t| t == kAudioDeviceTransportTypeBuiltIn)
}

fn set_value<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    value: T,
) -> Result<(), HalError> {
    let addr = address(selector, scope);
    // SAFETY: valid address and a readable buffer of `size_of::<T>()` bytes for the call.
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(&value).cast(),
        )
    };
    check(status, || {
        format!("set '{}' on object {object}", fourcc(selector))
    })
}

pub fn set_default_device(input: bool, device: AudioObjectID) -> Result<(), HalError> {
    let selector = if input {
        kAudioHardwarePropertyDefaultInputDevice
    } else {
        kAudioHardwarePropertyDefaultOutputDevice
    };
    set_value(SYSTEM, selector, GLOBAL, device)
}

pub fn set_output_volume(device: AudioObjectID, scalar: f32) -> Result<(), HalError> {
    set_value(
        device,
        kAudioDevicePropertyVolumeScalar,
        kAudioObjectPropertyScopeOutput,
        scalar.clamp(0.0, 1.0),
    )
}

pub fn destroy_aggregate(device: AudioObjectID) -> Result<(), HalError> {
    // SAFETY: the id names a device the caller found in the HAL's list.
    let status = unsafe { AudioHardwareDestroyAggregateDevice(device) };
    check(status, || format!("destroy aggregate device {device}"))
}

/// This process's HAL process object. The translation is itself a HAL call, so it registers the
/// process as a HAL client if nothing has yet, and an unknown object is an error, never "none".
pub fn own_process_object() -> Result<AudioObjectID, HalError> {
    let pid = std::process::id() as i32;
    let id = get_with::<AudioObjectID, i32>(
        SYSTEM,
        kAudioHardwarePropertyTranslatePIDToProcessObject,
        GLOBAL,
        Some(&pid),
    )?;
    if id == kAudioObjectUnknown {
        return Err(HalError {
            what: format!("translate pid {pid} to a process object: unknown"),
            status: 0,
        });
    }
    Ok(id)
}

/// True if any process other than `own` is currently sending audio to an output device.
pub fn others_playing(own: AudioObjectID) -> bool {
    let Ok(processes) = get_ids(SYSTEM, kAudioHardwarePropertyProcessObjectList) else {
        return false;
    };
    processes
        .into_iter()
        .filter(|p| *p != own)
        .any(|p| get::<u32>(p, kAudioProcessPropertyIsRunningOutput, GLOBAL).is_ok_and(|v| v != 0))
}

/// A linear-PCM format the IOProc can read as packed native f32.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PcmFormat {
    pub rate_hz: f64,
    pub channels: usize,
    pub non_interleaved: bool,
}

impl PcmFormat {
    pub fn parse(asbd: &AudioStreamBasicDescription) -> Result<Self, String> {
        let float = asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0;
        if asbd.mFormatID != kAudioFormatLinearPCM || !float || asbd.mBitsPerChannel != 32 {
            return Err(format!(
                "unsupported tap format: id {:#x}, flags {:#x}, {} bits",
                asbd.mFormatID, asbd.mFormatFlags, asbd.mBitsPerChannel
            ));
        }
        Ok(Self {
            rate_hz: asbd.mSampleRate,
            channels: asbd.mChannelsPerFrame as usize,
            non_interleaved: asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0,
        })
    }

    /// The buffers this format occupies in an AudioBufferList.
    pub fn buffers(&self) -> Vec<BufferShape> {
        if self.non_interleaved {
            vec![BufferShape { channels: 1 }; self.channels]
        } else {
            vec![BufferShape {
                channels: self.channels,
            }]
        }
    }
}

/// A private global stereo process tap. Destroyed on drop.
pub struct Tap {
    pub id: AudioObjectID,
    pub uuid: String,
    pub format: PcmFormat,
    pub excluded: AudioObjectID,
}

impl Tap {
    /// Taps every process's output except `exclude`, without muting it.
    pub fn create(exclude: AudioObjectID) -> Result<Self, String> {
        let excluded: [Retained<NSNumber>; 1] = [NSNumber::new_u32(exclude)];
        let excluded = NSArray::from_retained_slice(&excluded);
        let uuid = NSUUID::new();
        // SAFETY: plain Objective-C initialisers and setters on a fresh description.
        let description = unsafe {
            let d = CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &excluded,
            );
            d.setUUID(&uuid);
            d.setMuteBehavior(CATapMuteBehavior::Unmuted);
            d.setPrivate(true);
            d.setName(&NSString::from_str("Dory capture tap"));
            d
        };
        let mut id = kAudioObjectUnknown;
        // SAFETY: valid description and out pointer.
        let status = unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut id) };
        check(status, || "AudioHardwareCreateProcessTap".into()).map_err(|e| e.to_string())?;
        let mut tap = Self {
            id,
            uuid: uuid.UUIDString().to_string(),
            excluded: exclude,
            format: PcmFormat {
                rate_hz: 0.0,
                channels: 0,
                non_interleaved: false,
            },
        };
        let asbd: AudioStreamBasicDescription =
            get(id, kAudioTapPropertyFormat, GLOBAL).map_err(|e| e.to_string())?;
        tap.format = PcmFormat::parse(&asbd)?;
        Ok(tap)
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: we created this tap and destroy it once.
        unsafe { AudioHardwareDestroyProcessTap(self.id) };
    }
}

/// One sub-device of the aggregate.
pub struct SubDevice {
    pub uid: String,
    pub drift_compensation: bool,
}

/// A private aggregate device. Destroyed on drop.
pub struct Aggregate {
    pub id: AudioObjectID,
    pub uid: String,
}

fn key(k: &CStr) -> CFRetained<CFString> {
    CFString::from_str(k.to_str().expect("HAL keys are ASCII"))
}

fn dict(pairs: &[(&CStr, CFRetained<CFType>)]) -> CFRetained<CFDictionary<CFString, CFType>> {
    let keys: Vec<CFRetained<CFString>> = pairs.iter().map(|(k, _)| key(k)).collect();
    let key_refs: Vec<&CFString> = keys.iter().map(|k| &**k).collect();
    let value_refs: Vec<&CFType> = pairs.iter().map(|(_, v)| &**v).collect();
    CFDictionary::from_slices(&key_refs, &value_refs)
}

fn cf<T: Type>(value: CFRetained<T>) -> CFRetained<CFType> {
    // SAFETY: every CF object is a CFType.
    unsafe { CFRetained::cast_unchecked(value) }
}

fn number(v: i32) -> CFRetained<CFType> {
    cf(CFNumber::new_i32(v))
}

fn array(items: Vec<CFRetained<CFDictionary<CFString, CFType>>>) -> CFRetained<CFType> {
    let refs: Vec<&CFType> = items.iter().map(|d| d.as_opaque().as_ref()).collect();
    cf(objc2_core_foundation::CFArray::<CFType>::from_objects(
        &refs,
    ))
}

impl Aggregate {
    /// Builds a private aggregate clocked by `main_uid`, with `subdevices` then `tap` as inputs.
    pub fn create(
        main_uid: &str,
        subdevices: &[SubDevice],
        tap: Option<&Tap>,
    ) -> Result<Self, HalError> {
        use objc2_core_audio::{
            kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
            kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
            kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapListKey,
            kAudioAggregateDeviceUIDKey, kAudioSubDeviceDriftCompensationKey,
            kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
        };
        let uid = format!(
            "{}{}",
            super::AGGREGATE_UID_PREFIX,
            NSUUID::new().UUIDString()
        );
        let subs = subdevices
            .iter()
            .map(|s| {
                dict(&[
                    (kAudioSubDeviceUIDKey, cf(CFString::from_str(&s.uid))),
                    (
                        kAudioSubDeviceDriftCompensationKey,
                        number(s.drift_compensation as i32),
                    ),
                ])
            })
            .collect();
        let taps = tap
            .map(|t| {
                dict(&[
                    (kAudioSubTapUIDKey, cf(CFString::from_str(&t.uuid))),
                    (kAudioSubTapDriftCompensationKey, number(1)),
                ])
            })
            .into_iter()
            .collect();
        let description = dict(&[
            (
                kAudioAggregateDeviceNameKey,
                cf(CFString::from_str("Dory capture")),
            ),
            (kAudioAggregateDeviceUIDKey, cf(CFString::from_str(&uid))),
            (
                kAudioAggregateDeviceMainSubDeviceKey,
                cf(CFString::from_str(main_uid)),
            ),
            (kAudioAggregateDeviceIsPrivateKey, number(1)),
            (kAudioAggregateDeviceIsStackedKey, number(0)),
            (kAudioAggregateDeviceSubDeviceListKey, array(subs)),
            (kAudioAggregateDeviceTapListKey, array(taps)),
        ]);
        let mut id = kAudioObjectUnknown;
        // SAFETY: a CFDictionary of CFString keys and CF values, as the HAL documents.
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(description.as_opaque(), NonNull::from(&mut id))
        };
        check(status, || "AudioHardwareCreateAggregateDevice".into())?;
        Ok(Self { id, uid })
    }
}

impl Drop for Aggregate {
    fn drop(&mut self) {
        // SAFETY: we created this aggregate and destroy it once.
        unsafe { AudioHardwareDestroyAggregateDevice(self.id) };
    }
}

pub type IoBlockFn = dyn Fn(
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
);

/// A registered IOProc and the block it runs. `close` stops the device and destroys the IOProc.
pub struct IoProc {
    pub device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    block: Option<RcBlock<IoBlockFn>>,
}

// SAFETY: an IOProc id is an opaque token the HAL lets any thread stop and destroy, and the block
// is only released here or leaked. Its captures are `Send + Sync`, as `IoProc::create` requires.
unsafe impl Send for IoProc {}

impl IoProc {
    /// Registers `block` as an IOProc on `device`. The block's captures must be `Send + Sync`,
    /// because the HAL calls it on its IO thread and `close` may release it on any thread.
    pub fn create(device: AudioObjectID, block: RcBlock<IoBlockFn>) -> Result<Self, HalError> {
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: valid out pointer and device, and `IoProc` keeps the block alive until destroy.
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut proc_id),
                device,
                None,
                RcBlock::as_ptr(&block),
            )
        };
        check(status, || "AudioDeviceCreateIOProcIDWithBlock".into())?;
        Ok(Self {
            device,
            proc_id,
            block: Some(block),
        })
    }

    pub fn start(&self) -> Result<(), HalError> {
        // SAFETY: the IOProc was created on this device and is not destroyed yet.
        let status = unsafe { objc2_core_audio::AudioDeviceStart(self.device, self.proc_id) };
        check(status, || "AudioDeviceStart".into())
    }

    /// Stops the device and destroys the IOProc. If the HAL refuses to destroy it, the block is
    /// leaked, because the HAL may still call it. Idempotent.
    pub fn close(&mut self) -> Result<(), HalError> {
        let Some(block) = self.block.take() else {
            return Ok(());
        };
        // SAFETY: the IOProc was created on this device and is destroyed at most once.
        let (stopped, destroyed) = unsafe {
            (
                AudioDeviceStop(self.device, self.proc_id),
                AudioDeviceDestroyIOProcID(self.device, self.proc_id),
            )
        };
        if destroyed != 0 {
            std::mem::forget(block);
        }
        check(stopped, || "AudioDeviceStop".into())?;
        check(destroyed, || "AudioDeviceDestroyIOProcID".into())
    }
}

impl Drop for IoProc {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl IoProc {
    /// Marks every output stream of the device unused by this IOProc, so this process never
    /// counts as playing audio. Observed on macOS 26.5: with the speakers as a sub-device the
    /// process showed as running output without this call and as input only with it, and
    /// `corespeechd` started running output either way. Only removing the speakers sub-device
    /// kept `corespeechd` idle. A tap plus the built-in mic has no output streams, so this is a
    /// no-op there. It matters for a mic device that also has outputs, such as a headset.
    pub fn disable_output(&self) -> Result<(), HalError> {
        let streams = get_bytes(
            self.device,
            kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeOutput,
        )?;
        let count = as_bytes(&streams).len() / size_of::<AudioObjectID>();
        if count == 0 {
            return Ok(());
        }
        // AudioHardwareIOProcStreamUsage: a pointer, a u32 count, then `count` u32 flags, all zero.
        let size = size_of::<*mut c_void>() + 4 + 4 * count;
        let mut usage = vec![0u64; size.div_ceil(8)];
        let proc_ptr = self
            .proc_id
            .map_or(std::ptr::null_mut(), |f| f as *mut c_void);
        // SAFETY: `usage` holds `size` bytes laid out as the C struct, pointer first.
        unsafe {
            let base = usage.as_mut_ptr().cast::<u8>();
            base.cast::<*mut c_void>().write(proc_ptr);
            base.add(size_of::<*mut c_void>())
                .cast::<u32>()
                .write_unaligned(count as u32);
        }
        let addr = address(
            kAudioDevicePropertyIOProcStreamUsage,
            kAudioObjectPropertyScopeOutput,
        );
        // SAFETY: valid address and a buffer of `size` bytes.
        let status = unsafe {
            AudioObjectSetPropertyData(
                self.device,
                NonNull::from(&addr),
                0,
                std::ptr::null(),
                size as u32,
                NonNull::new_unchecked(usage.as_mut_ptr().cast()),
            )
        };
        check(status, || "set output stream usage".into())
    }
}

pub type ListenerFn = dyn Fn(u32, NonNull<AudioObjectPropertyAddress>);

/// Property listeners that run on one private serial queue. `remove_all` (and drop) unregisters
/// them and then drains the queue, so none runs afterwards.
pub struct Listeners {
    queue: DispatchRetained<DispatchQueue>,
    added: Vec<(
        AudioObjectID,
        AudioObjectPropertyAddress,
        RcBlock<ListenerFn>,
    )>,
}

// SAFETY: every block captures only `Send + Sync` data (enforced by `add`), and the queue is a
// thread-safe dispatch object.
unsafe impl Send for Listeners {}

impl Listeners {
    pub fn new() -> Self {
        Self {
            queue: DispatchQueue::new("dory.capture.listeners", None),
            added: Vec::new(),
        }
    }

    /// Calls `on_change` on the listener queue whenever `selector` on `object` changes.
    pub fn add(
        &mut self,
        object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), HalError> {
        let addr = address(selector, GLOBAL);
        let block: RcBlock<ListenerFn> =
            RcBlock::new(move |_: u32, _: NonNull<AudioObjectPropertyAddress>| on_change());
        // SAFETY: valid address, queue, and block. The block stays alive in `added` until removed.
        let status = unsafe {
            AudioObjectAddPropertyListenerBlock(
                object,
                NonNull::from(&addr),
                Some(&self.queue),
                RcBlock::as_ptr(&block),
            )
        };
        check(status, || {
            format!("listen to '{}' on object {object}", fourcc(selector))
        })?;
        self.added.push((object, addr, block));
        Ok(())
    }

    pub fn remove_all(&mut self) {
        for (object, addr, block) in self.added.drain(..) {
            // SAFETY: the same object, address, queue and block that were added.
            unsafe {
                AudioObjectRemovePropertyListenerBlock(
                    object,
                    NonNull::from(&addr),
                    Some(&self.queue),
                    RcBlock::as_ptr(&block),
                )
            };
        }
        self.queue.exec_sync(|| {});
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.remove_all();
    }
}

#[cfg(test)]
impl IoProc {
    /// Stops the device behind the capture's back, as a hung device would.
    pub fn stall(&self) {
        // SAFETY: the IOProc was created on this device and is still registered.
        unsafe { AudioDeviceStop(self.device, self.proc_id) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_audio::kAudioAggregateDevicePropertyTapList;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Sets the UIDs of the taps in a private aggregate we own.
    fn set_taps(aggregate: &Aggregate, taps: &[&str]) {
        let uids: Vec<CFRetained<CFString>> = taps.iter().map(|t| CFString::from_str(t)).collect();
        let refs: Vec<&CFString> = uids.iter().map(|u| &**u).collect();
        let list = objc2_core_foundation::CFArray::<CFString>::from_objects(&refs);
        let addr = address(kAudioAggregateDevicePropertyTapList, GLOBAL);
        let value: *const objc2_core_foundation::CFArray<CFString> = &*list;
        // SAFETY: the property takes a CFArray of CFString UIDs, passed by reference.
        let status = unsafe {
            AudioObjectSetPropertyData(
                aggregate.id,
                NonNull::from(&addr),
                0,
                std::ptr::null(),
                size_of::<*const c_void>() as u32,
                NonNull::from(&value).cast(),
            )
        };
        check(status, || "set aggregate tap list".into()).unwrap();
    }

    #[test]
    #[ignore = "creates a private tap and aggregate through the Core Audio HAL"]
    fn listeners_run_on_change_and_are_released_on_removal() {
        let tap = Tap::create(own_process_object().unwrap()).unwrap();
        let clock = device_uid(default_device(false).unwrap().unwrap()).unwrap();
        let aggregate = Aggregate::create(&clock, &[], Some(&tap)).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut listeners = Listeners::new();
        listeners
            .add(
                aggregate.id,
                kAudioAggregateDevicePropertyTapList,
                move || {
                    let _ = tx.send(());
                },
            )
            .unwrap();
        set_taps(&aggregate, &[]);
        rx.recv_timeout(Duration::from_secs(2))
            .expect("the listener ran after the property changed");
        listeners.remove_all();
        set_taps(&aggregate, &[&tap.uuid]);
        while rx.try_recv().is_ok() {}
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(500)),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "removal released the listener, so nothing can call it again"
        );
    }
}
