//! `<id>.levels`: a take's measured loudness, kept so the player never reads the take twice.
//!
//! Version 1 is the 4-byte magic `DLV1`, a little-endian `u32` bucket count, then that many
//! little-endian `f32` levels. A file that does not parse is measured again and replaced.

use dory_core::domain::{LevelMeter, Levels};
use dory_core::ports::TakeError;

use super::pcm::FrameReader;

const MAGIC: &[u8; 4] = b"DLV1";

pub(crate) fn encode(levels: &Levels) -> Vec<u8> {
    let values = levels.as_slice();
    let mut bytes = Vec::with_capacity(8 + values.len() * 4);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&u32::try_from(values.len()).unwrap_or(0).to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(crate) fn decode(bytes: &[u8]) -> Option<Levels> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (count, values) = rest.split_first_chunk::<4>()?;
    let count = usize::try_from(u32::from_le_bytes(*count)).ok()?;
    if values.len() != count.checked_mul(4)? {
        return None;
    }
    let (chunks, _) = values.as_chunks::<4>();
    Some(Levels::new(
        chunks.iter().map(|b| f32::from_le_bytes(*b)).collect(),
    ))
}

/// Feeds 16-bit frames to `meter`, through `scratch` so a long take allocates once.
pub(crate) fn feed(meter: &mut LevelMeter, pcm: &[i16], scratch: &mut Vec<f32>) {
    scratch.clear();
    scratch.extend(pcm.iter().map(|s| f32::from(*s) / 32768.));
    meter.feed(scratch);
}

/// Reads `reader` to the end, a block at a time.
pub(crate) fn measure(reader: &mut dyn FrameReader) -> Result<Levels, TakeError> {
    let mut meter = LevelMeter::new(reader.length_frames().unwrap_or(0), reader.channels());
    let (mut pcm, mut scratch) = (Vec::new(), Vec::new());
    while reader.read_frames(&mut pcm, 16_384)? > 0 {
        feed(&mut meter, &pcm, &mut scratch);
    }
    Ok(meter.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_file_reads_back_the_same_levels() {
        let levels = Levels::new(vec![0., 0.25, 1.]);
        assert_eq!(decode(&encode(&levels)), Some(levels));
    }

    #[test]
    fn a_torn_or_foreign_file_is_not_read() {
        let bytes = encode(&Levels::new(vec![0.5; 10]));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(b"RIFF\x00\x00\x00\x00"), None);
        assert_eq!(decode(&[]), None);
    }
}
