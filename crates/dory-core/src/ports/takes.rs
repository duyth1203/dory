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

    /// The part of `whole`, a clip of the whole channel from [`Takes::clip`], that lies in `span`
    /// (take time), without decoding the take again. Windows of a long take are cut this way, so
    /// each costs its own length, not everything before it.
    fn slice(&self, whole: &Clip, span: TimeRange) -> Result<Clip, TakeError>;

    /// Takes still open as WAV, left by a crash before sealing.
    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError>;

    /// Every id with any file here: open, sealing, sealed, or a legacy pointer. Startup deletes
    /// those whose recording is gone, whatever state the files are in.
    fn stored(&self) -> Result<Vec<RecordingId>, TakeError>;

    /// What is on disk, with the duration read from the file. `None` if there is no take.
    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError>;

    /// Copies an external audio file in as a take. Imports are always `ChannelLayout::Mixed`.
    fn import(&self, id: &RecordingId, source: &Path) -> Result<TakeInfo, TakeError>;

    /// The old app's audio file a legacy pointer refers to, or `None` when `id` has no pointer.
    /// The file may be gone, which [`LegacyFile::bytes`] says.
    fn legacy_original(&self, id: &RecordingId) -> Result<Option<LegacyFile>, TakeError>;

    /// Copies the audio a legacy pointer refers to in as this adapter's own take, then removes the
    /// pointer. The original is never touched. The pointer goes last, so a crash at any point
    /// leaves the legacy take readable and a second call completes. With no pointer and an own
    /// take present, returns that take's info.
    fn adopt(&self, id: &RecordingId) -> Result<TakeInfo, TakeError>;

    /// A stable path for playback and "reveal in Finder". Not for engines.
    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError>;

    fn delete(&self, id: &RecordingId) -> Result<(), TakeError>;

    /// Every file this adapter keeps for `id`, in whatever state: the files [`Takes::delete`]
    /// would remove, and no others. A legacy take lists only Dory's own pointer, never the old
    /// app's audio it points at. Empty when `id` has no files.
    fn files(&self, id: &RecordingId) -> Result<Vec<PathBuf>, TakeError>;
}

/// What a legacy pointer refers to. The adapter reads the size, so the core never touches the
/// file system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyFile {
    pub path: PathBuf,
    /// `None` when the file is gone.
    pub bytes: Option<u64>,
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

impl Activity {
    /// The spans inside `span`, which is on the same axis as these spans, moved to start at it.
    pub fn within(&self, span: TimeRange) -> Activity {
        match self {
            Activity::Unmeasured => Activity::Unmeasured,
            Activity::Speech(spans) => Activity::Speech(
                spans
                    .iter()
                    .filter_map(|s| {
                        TimeRange::new(
                            Ms(s.start().max(span.start()).0 - span.start().0),
                            Ms(s.end().min(span.end()).0.saturating_sub(span.start().0)),
                        )
                    })
                    .filter(|s| !s.is_empty())
                    .collect(),
            ),
        }
    }
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
