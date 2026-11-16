//! Audio bytes on disk. The store holds facts about a take. This port holds the samples. One
//! adapter owns the formats: crash-tolerant WAV while recording, FLAC after sealing, legacy WebM
//! read-only. The core never sees a container, only a [`Clip`] that is already what engines want.

use std::any::Any;
use std::path::{Path, PathBuf};

use super::TakeError;
use crate::domain::{Channel, ChannelLayout, Ms, RecordingId, TakeInfo, TimeRange};

pub trait Takes: Send + Sync {
    /// Creates the WAV that capture appends to. Fails with `Exists` if the id has a take.
    fn begin(
        &self,
        id: &RecordingId,
        layout: ChannelLayout,
        rate_hz: u32,
    ) -> Result<Box<dyn TakeWriter>, TakeError>;

    /// WAV to FLAC. Idempotent: sealing a sealed take returns its info, and a crash at any point
    /// leaves a state that a second call completes.
    fn seal(&self, id: &RecordingId) -> Result<TakeInfo, TakeError>;

    /// One channel as mono audio ready for an engine, optionally cut to `span` (take time). The
    /// clip's `origin` says where it starts in the take.
    fn clip(
        &self,
        id: &RecordingId,
        channel: Channel,
        span: Option<TimeRange>,
    ) -> Result<Clip, TakeError>;

    /// Takes still open as WAV, left by a crash before sealing.
    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError>;

    /// Every id with any file here: open, sealing, sealed, or a legacy pointer. Startup deletes
    /// those whose recording is gone, whatever state the files are in.
    fn stored(&self) -> Result<Vec<RecordingId>, TakeError>;

    /// What is on disk, with the duration read from the file. `None` if there is no take.
    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError>;

    /// Copies an external audio file in as a take. Imports are always `ChannelLayout::Mixed`.
    fn import(&self, id: &RecordingId, source: &Path) -> Result<TakeInfo, TakeError>;

    /// A stable path for playback and "reveal in Finder". Not for engines.
    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError>;

    fn delete(&self, id: &RecordingId) -> Result<(), TakeError>;
}

/// Appends to an open take. The same writer survives pause and the undo grace, because a resume
/// continues the same file.
pub trait TakeWriter: Send {
    /// `interleaved` holds whole frames in the take's layout order.
    fn write(&mut self, interleaved: &[f32]) -> Result<(), TakeError>;

    /// Makes what was written so far recoverable after a crash.
    fn checkpoint(&mut self) -> Result<(), TakeError>;

    fn frames_written(&self) -> u64;

    /// Closes the WAV. Sealing is a separate step so a stop can still be undone.
    fn finish(self: Box<Self>) -> Result<TakeInfo, TakeError>;
}

/// Mono audio an engine can read by path. Dropping it removes whatever backs it.
pub struct Clip {
    pub path: PathBuf,
    pub channel: Channel,
    pub duration: Ms,
    /// Where this clip starts in the take. Engines report times relative to the clip, and the core
    /// shifts them by this.
    pub origin: Ms,
    pub activity: Activity,
    _guard: Option<Box<dyn Any + Send + Sync>>,
}

impl Clip {
    /// `guard` is dropped with the clip, for example to delete a temporary file.
    pub fn new(
        path: PathBuf,
        channel: Channel,
        duration: Ms,
        origin: Ms,
        activity: Activity,
        guard: Option<Box<dyn Any + Send + Sync>>,
    ) -> Clip {
        Clip {
            path,
            channel,
            duration,
            origin,
            activity,
            _guard: guard,
        }
    }
}

/// Where someone talks in a clip. The core cuts windows at the silences between spans and skips
/// clips and windows with no speech.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// The adapter did not measure it, so the core treats the whole clip as speech.
    Unmeasured,
    /// Spans relative to the clip, in order and disjoint. Empty means the clip is silent.
    Speech(Vec<TimeRange>),
}

impl std::fmt::Debug for Clip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clip")
            .field("path", &self.path)
            .field("channel", &self.channel)
            .field("duration", &self.duration)
            .field("origin", &self.origin)
            .field("activity", &self.activity)
            .finish_non_exhaustive()
    }
}
