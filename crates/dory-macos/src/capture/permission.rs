//! Microphone permission through AVFoundation. System Audio Recording has no public query API.

use std::sync::mpsc;

use block2::RcBlock;
use dory_core::ports::capture::Access;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

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
