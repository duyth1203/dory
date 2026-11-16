use std::sync::Arc;

use dory_core::ports::capture::CaptureBackend;
use dory_core::ports::{Machine, Playback, Trash};

#[cfg(target_os = "macos")]
pub fn capture() -> Arc<dyn CaptureBackend> {
    Arc::new(dory_macos::CoreAudioCapture::new())
}

#[cfg(target_os = "macos")]
pub fn playback() -> Arc<dyn Playback> {
    Arc::new(dory_macos::AvPlayback::new())
}

#[cfg(target_os = "macos")]
pub fn trash() -> Arc<dyn Trash> {
    Arc::new(dory_macos::MacTrash::new())
}

#[cfg(target_os = "macos")]
pub fn machine() -> Arc<dyn Machine> {
    Arc::new(dory_macos::MacMachine::new())
}

#[cfg(not(target_os = "macos"))]
pub fn machine() -> Arc<dyn Machine> {
    Arc::new(Unsupported)
}

/// Reports nothing it cannot read, so Automatic runs fast.
#[cfg(not(target_os = "macos"))]
impl Machine for Unsupported {
    fn conditions(&self) -> dory_core::ports::Conditions {
        dory_core::ports::Conditions::default()
    }
}

#[cfg(not(target_os = "macos"))]
pub fn trash() -> Arc<dyn Trash> {
    Arc::new(Unsupported)
}

#[cfg(not(target_os = "macos"))]
impl Trash for Unsupported {
    fn move_to_trash(
        &self,
        _: &[std::path::PathBuf],
    ) -> Result<dory_core::ports::TrashReceipt, dory_core::ports::TrashError> {
        Err(dory_core::ports::TrashError::Unsupported)
    }

    fn put_back(
        &self,
        _: &dory_core::ports::TrashReceipt,
    ) -> Result<(), dory_core::ports::TrashError> {
        Err(dory_core::ports::TrashError::Unsupported)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn playback() -> Arc<dyn Playback> {
    Arc::new(Unsupported)
}

#[cfg(not(target_os = "macos"))]
impl Playback for Unsupported {
    fn open(
        &self,
        _: &dory_core::ports::PlaybackSource,
    ) -> Result<Box<dyn dory_core::ports::Player>, dory_core::ports::PlaybackError> {
        Err(dory_core::ports::PlaybackError::Unsupported)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn capture() -> Arc<dyn CaptureBackend> {
    Arc::new(Unsupported)
}

/// The backend on a platform with no adapter yet. Everything says so.
#[cfg(not(target_os = "macos"))]
struct Unsupported;

#[cfg(not(target_os = "macos"))]
const WHY: &str = "audio capture has no adapter for this platform yet";

#[cfg(not(target_os = "macos"))]
impl CaptureBackend for Unsupported {
    fn probe(&self) -> dory_core::ports::capture::CaptureCaps {
        use dory_core::ports::capture::{Access, CaptureCaps};
        CaptureCaps {
            microphone: Access::Unsupported,
            system_audio: Access::Unsupported,
            unsupported: Some(WHY.into()),
        }
    }

    fn input_devices(
        &self,
    ) -> Result<Vec<dory_core::ports::capture::InputDevice>, dory_core::ports::capture::CaptureError>
    {
        Err(dory_core::ports::capture::CaptureError::Unsupported(
            WHY.into(),
        ))
    }

    fn request_access(
        &self,
        _: dory_core::ports::capture::Source,
    ) -> Result<dory_core::ports::capture::Access, dory_core::ports::capture::CaptureError> {
        Ok(dory_core::ports::capture::Access::Unsupported)
    }

    fn start(
        &self,
        _: &dory_core::ports::capture::CaptureSpec,
        _: dory_core::capture::FrameSink,
    ) -> Result<
        Box<dyn dory_core::ports::capture::CaptureHandle>,
        dory_core::ports::capture::CaptureError,
    > {
        Err(dory_core::ports::capture::CaptureError::Unsupported(
            WHY.into(),
        ))
    }
}
