use std::num::NonZeroU32;

/// Milliseconds of recorded media. Pauses are never written, so they never appear on this axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ms(pub u64);

impl Ms {
    /// The media time of `frames` at `rate_hz`, rounded down. Convert a running total, never a
    /// delta: flooring each chunk would drop up to a millisecond every time.
    pub fn from_frames(frames: u64, rate_hz: NonZeroU32) -> Ms {
        let ms = u128::from(frames) * 1000 / u128::from(rate_hz.get());
        Ms(u64::try_from(ms).unwrap_or(u64::MAX))
    }
}

/// One mono source inside a take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Mic,
    System,
    /// Mic and system audio already summed, as in legacy WebM recordings.
    Mixed,
}

/// Which sources a take holds and in what interleaved order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChannelLayout {
    /// Two channels, always `[mic, system]`.
    MicSystem,
    MicOnly,
    SystemOnly,
    /// One channel with mic and system summed. Legacy recordings and imports.
    Mixed,
}

impl ChannelLayout {
    pub fn channels(self) -> &'static [Channel] {
        match self {
            ChannelLayout::MicSystem => &[Channel::Mic, Channel::System],
            ChannelLayout::MicOnly => &[Channel::Mic],
            ChannelLayout::SystemOnly => &[Channel::System],
            ChannelLayout::Mixed => &[Channel::Mixed],
        }
    }

    pub fn channel_count(self) -> usize {
        self.channels().len()
    }

    pub fn index_of(self, channel: Channel) -> Option<usize> {
        self.channels().iter().position(|c| *c == channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hz(rate: u32) -> NonZeroU32 {
        NonZeroU32::new(rate).unwrap()
    }

    #[test]
    fn from_frames_table() {
        let cases = [
            (0, 48_000, 0),
            (47, 48_000, 0),
            (48, 48_000, 1),
            (959, 48_000, 19),
            (48_000, 48_000, 1_000),
            (345_600_000, 48_000, 7_200_000),
            (1, 1, 1_000),
            (u64::MAX, 1, u64::MAX),
        ];
        for (frames, rate, want) in cases {
            assert_eq!(
                Ms::from_frames(frames, hz(rate)),
                Ms(want),
                "{frames} at {rate}"
            );
        }
    }

    #[test]
    fn a_zero_rate_cannot_be_built() {
        assert!(NonZeroU32::new(0).is_none());
    }
}
