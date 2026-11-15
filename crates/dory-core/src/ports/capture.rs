//! Platform audio capture. Each platform implements this port in its own adapter crate
//! (`dory-macos` first). The adapter hides device enumeration, permission flows, aggregate
//! devices, clock drift, and resampling.
//!
//! Contract the core relies on:
//! 1. Frames go to [`FrameSink::push`], which is wait-free, so the adapter may call it on the
//!    real-time audio thread. The adapter writes no ring buffer and no drain thread of its own.
//! 2. Frames are f32, interleaved in `spec.layout` order, at exactly `spec.rate_hz`. If a device
//!    runs at another rate, the adapter resamples. It never changes a device's rate.
//! 3. A lost source is filled with silence of the right length plus a [`CaptureIssue`], so every
//!    pushed frame has every channel.
//! 4. The adapter never changes the user's default devices and creates only private devices.
//! 5. [`CaptureHandle::stop`] releases the hardware, so the OS recording indicator turns off, and
//!    drops the sink before it returns. The core stops reading the ring once `stop` returns, so a
//!    frame pushed later is lost. Pause and resume are stop and a fresh `start`.
//! 6. If system audio is digital silence for 3 seconds while the output device runs, the adapter
//!    reports [`CaptureIssue::SystemAudioSilent`]. On macOS that is how a denied System Audio
//!    Recording permission shows up.

use thiserror::Error;

use crate::capture::FrameSink;
use crate::domain::{Channel, ChannelLayout};

pub trait CaptureBackend: Send + Sync {
    /// What this platform can capture and the current permission state. Never prompts.
    fn probe(&self) -> CaptureCaps;

    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError>;

    /// Shows the OS prompt if the user has not answered yet. Blocks until they answer.
    fn request_access(&self, source: Source) -> Result<Access, CaptureError>;

    /// Starts delivering frames into `sink`. Returns once the device is running.
    fn start(
        &self,
        spec: &CaptureSpec,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}

pub trait CaptureHandle: Send {
    /// Stops the device and drops the sink before returning. After this returns, no frame is
    /// pushed, and the core does not wait for the sink to drop.
    fn stop(self: Box<Self>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Microphone,
    SystemAudio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Granted,
    /// Not asked yet, or the platform cannot tell (macOS System Audio Recording).
    Undetermined,
    Denied,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureCaps {
    pub microphone: Access,
    pub system_audio: Access,
    /// Set when this adapter cannot run here at all, for example on an older OS.
    pub unsupported: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    pub uid: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicSelection {
    /// Follow the system default input, including changes during the recording.
    SystemDefault,
    Uid(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSpec {
    pub layout: ChannelLayout,
    pub rate_hz: u32,
    pub mic: MicSelection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureIssue {
    SourceLost(Channel),
    SourceRestored(Channel),
    /// The input device changed while following the system default.
    MicChanged {
        name: String,
    },
    /// System audio has been digital silence for 3 seconds while output is running.
    SystemAudioSilent,
    /// The adapter rebuilt its device graph, for example after the output device changed.
    Rebuilt {
        gap_frames: u64,
    },
    Fatal(String),
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CaptureError {
    #[error("{0:?} access denied")]
    AccessDenied(Source),
    #[error("input device {0} not found")]
    DeviceNotFound(String),
    #[error("not supported here: {0}")]
    Unsupported(String),
    #[error("capture platform error: {message}")]
    Platform {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}
