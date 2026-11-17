//! Everything the core needs from the outside world, as traits. Adapters implement them.

mod audit;
mod cancel;
pub mod capture;
mod clock;
mod diarize;
mod error;
mod llm;
mod machine;
mod playback;
mod provider;
mod remote;
mod speech;
mod store;
mod takes;
mod trash;

pub use audit::{AuditRead, AuditSink};
pub use cancel::CancelToken;
pub use clock::Clock;
pub use diarize::{Diarizer, Turn};
pub use error::{AuditError, Classify, EngineError, ProvisionError, StoreError, TakeError};
pub use llm::{CompletionRequest, LanguageModel, TextSink};
pub use machine::{Conditions, Machine, Pace, Power, Thermal};
pub use playback::{
    PlayState, Playback, PlaybackError, PlaybackSource, Player, PlayerStatus, SYSTEM_GAIN,
    channel_gains, mix_down,
};
pub use provider::{ModelProvider, ProvisionSink};
pub use remote::{Consent, Remote};
pub use speech::{RawSegment, SegmentSink, TranscribeOutcome, TranscribeRequest, Transcriber};
pub use store::{Change, Checkpoint, CheckpointKey, Store, StoreRead};
pub use takes::LegacyFile;
pub use takes::{Activity, Clip, TakeWriter, Takes};
pub use trash::{Moved, Trash, TrashError, TrashReceipt};
