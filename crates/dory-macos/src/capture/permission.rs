//! Microphone permission through AVFoundation. System Audio Recording has no public API, so it
//! goes through the private TCC framework, loaded at run time: if Apple removes these calls,
//! the caller falls back to what a recording has heard.

use std::ffi::{c_char, c_int, c_void};
use std::sync::{OnceLock, mpsc};

use block2::{Block, RcBlock};
use dory_core::ports::capture::Access;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_foundation::CFString;

pub fn microphone() -> Access {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Access::Unsupported;
    };
    // SAFETY: AVMediaTypeAudio is one of the two media types this call accepts.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) };
    match status {
        AVAuthorizationStatus::Authorized => Access::Granted,
        AVAuthorizationStatus::NotDetermined => Access::Undetermined,
        _ => Access::Denied,
    }
}

/// Shows the OS prompt if the user has not answered, and blocks until they do.
pub fn request_microphone() -> Access {
    let current = microphone();
    if current != Access::Undetermined {
        return current;
    }
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Access::Unsupported;
    };
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    // SAFETY: valid media type; the block is retained by AVFoundation until it is called.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
    match rx.recv() {
        Ok(true) => Access::Granted,
        Ok(false) => Access::Denied,
        Err(_) => microphone(),
    }
}

const TCC_PATH: &[u8] = b"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC\0";
const AUDIO_CAPTURE: &str = "kTCCServiceAudioCapture";
const RTLD_NOW: c_int = 2;

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type Preflight = unsafe extern "C" fn(service: &CFString, options: *const c_void) -> c_int;
type Request =
    unsafe extern "C" fn(service: &CFString, options: *const c_void, callback: &Block<dyn Fn(u8)>);

struct Tcc {
    preflight: Preflight,
    request: Request,
}

fn tcc() -> Option<&'static Tcc> {
    static TCC: OnceLock<Option<Tcc>> = OnceLock::new();
    TCC.get_or_init(|| {
        // SAFETY: a NUL-terminated path; dlopen returns null when the framework is missing.
        let handle = unsafe { dlopen(TCC_PATH.as_ptr().cast(), RTLD_NOW) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: NUL-terminated names on a live handle that is never closed.
        let preflight = unsafe { dlsym(handle, c"TCCAccessPreflight".as_ptr()) };
        let request = unsafe { dlsym(handle, c"TCCAccessRequest".as_ptr()) };
        if preflight.is_null() || request.is_null() {
            return None;
        }
        // SAFETY: both symbols are C functions with these signatures since macOS 10.14, as
        // AudioCap and other tap-based recorders call them.
        Some(unsafe {
            Tcc {
                preflight: std::mem::transmute::<*mut c_void, Preflight>(preflight),
                request: std::mem::transmute::<*mut c_void, Request>(request),
            }
        })
    })
    .as_ref()
}

/// System Audio Recording as TCC records it, or `None` when TCC cannot be asked.
pub fn system_audio() -> Option<Access> {
    let tcc = tcc()?;
    let service = CFString::from_static_str(AUDIO_CAPTURE);
    // SAFETY: a valid CFString and no options; the call only reads the TCC database.
    let answer = unsafe { (tcc.preflight)(&service, std::ptr::null()) };
    Some(match answer {
        0 => Access::Granted,
        1 => Access::Denied,
        _ => Access::Undetermined,
    })
}

/// Shows the System Audio Recording prompt if the user has not answered, and blocks until they
/// do. `None` when TCC cannot be asked.
pub fn request_system_audio() -> Option<Access> {
    let current = system_audio()?;
    if current != Access::Undetermined {
        return Some(current);
    }
    let tcc = tcc()?;
    let service = CFString::from_static_str(AUDIO_CAPTURE);
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |granted: u8| {
        let _ = tx.send(granted != 0);
    });
    // SAFETY: a valid CFString, no options, and a block TCC copies before it returns.
    unsafe { (tcc.request)(&service, std::ptr::null(), &handler) };
    match rx.recv() {
        Ok(true) => Some(Access::Granted),
        Ok(false) => Some(Access::Denied),
        Err(_) => system_audio(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only reads TCC. The answer is whichever app is responsible for the test process.
    #[test]
    fn tcc_answers_for_system_audio_recording() {
        let access = system_audio();
        eprintln!("system audio recording: {access:?}");
        assert!(
            access.is_some(),
            "TCC.framework or its calls could not be loaded"
        );
    }
}
