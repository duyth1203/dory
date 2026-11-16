//! Playing a take through the speakers. The facade resolves the file and the layout, the adapter
//! decodes and outputs it. How a take's channels become what the listener hears is decided here,
//! not in the adapter, so every platform mixes the same way.

use std::path::PathBuf;

use thiserror::Error;

use crate::domain::{ChannelLayout, Ms};

/// The system channel is recorded quieter than the mic, so playback lifts it.
pub const SYSTEM_GAIN: f32 = 1.85;

/// What an adapter needs to play one take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackSource {
    pub path: PathBuf,
    pub layout: ChannelLayout,
    /// What the store knows. The adapter's own reading of the file wins for position.
    pub duration: Ms,
}

pub trait Playback: Send + Sync {
    /// Prepares a player, paused at the start. Opening makes no sound and does not start audio
    /// hardware.
    fn open(&self, source: &PlaybackSource) -> Result<Box<dyn Player>, PlaybackError>;
}

/// One take being played. Dropping it stops the sound. Position is polled with [`Player::status`].
pub trait Player: Send + Sync {
    /// Starts or resumes. From [`PlayState::Ended`] it plays again from the start.
    fn play(&self) -> Result<(), PlaybackError>;

    fn pause(&self);

    /// Moves to `to`, clamped to the end, and keeps playing or staying paused. Seeking to the end
    /// of a playing take ends it.
    fn seek(&self, to: Ms) -> Result<(), PlaybackError>;

    fn status(&self) -> PlayerStatus;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Paused,
    Playing,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStatus {
    pub state: PlayState,
    /// Where in the take the listener is, at most the take's duration.
    pub position: Ms,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlaybackError {
    #[error("playback has no adapter for this platform yet")]
    Unsupported,
    #[error("the audio cannot be played: {0}")]
    Open(String),
    #[error("the audio output failed: {0}")]
    Output(String),
}

/// How much of each take channel reaches the listener, in layout order. Every channel goes to
/// both ears: a hard pan of mic left and system right would make a meeting tiring to hear.
pub fn channel_gains(layout: ChannelLayout) -> &'static [f32] {
    match layout {
        ChannelLayout::MicSystem => &[1.0, SYSTEM_GAIN],
        ChannelLayout::MicOnly | ChannelLayout::SystemOnly | ChannelLayout::Mixed => &[1.0],
    }
}

/// Sums planar `channels` weighted by `gains` into `out`, clamped to full scale so a loud mic and
/// a lifted system channel cannot clip the output. Every channel must hold `out.len()` samples.
pub fn mix_down(gains: &[f32], channels: &[&[f32]], out: &mut [f32]) {
    debug_assert_eq!(gains.len(), channels.len());
    for (i, sample) in out.iter_mut().enumerate() {
        let sum: f32 = gains.iter().zip(channels).map(|(g, c)| g * c[i]).sum();
        *sample = sum.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_channel_takes_lift_the_system_channel_and_one_channel_takes_pass_through() {
        assert_eq!(channel_gains(ChannelLayout::MicSystem), [1.0, SYSTEM_GAIN]);
        for layout in [
            ChannelLayout::MicOnly,
            ChannelLayout::SystemOnly,
            ChannelLayout::Mixed,
        ] {
            assert_eq!(channel_gains(layout), [1.0]);
        }
    }

    #[test]
    fn mixing_sums_weighted_channels_and_clamps() {
        let mic = [0.1, 0.8, -0.9];
        let system = [0.2, 0.5, -0.5];
        let mut out = [0.0; 3];
        mix_down(
            channel_gains(ChannelLayout::MicSystem),
            &[&mic, &system],
            &mut out,
        );
        assert!((out[0] - 0.47).abs() < 1e-6);
        assert_eq!(out[1], 1.0);
        assert_eq!(out[2], -1.0);
    }
}
