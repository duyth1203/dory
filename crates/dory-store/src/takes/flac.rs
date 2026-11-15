//! WAV to FLAC with `flacenc`, frame by frame so a two-hour take never sits in memory, and a
//! decode with `claxon` that proves the result before the WAV is deleted.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use dory_core::ports::TakeError;
use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Stream};
use flacenc::config::Encoder;
use flacenc::error::Verify;
use flacenc::source::{Context, Fill, FrameBuf};

use super::io_err;
use super::pcm::{FlacReader, FrameReader, OwnWavReader};

const BLOCK: usize = 4096;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn enc_err(e: impl std::fmt::Debug) -> TakeError {
    TakeError::Decode(format!("FLAC encoder: {e:?}"))
}

/// What the encoder read, to compare with what the decoder gives back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Digest {
    pub frames: u64,
    pub checksum: u64,
}

impl Digest {
    pub(crate) fn new() -> Self {
        Self {
            frames: 0,
            checksum: FNV_OFFSET,
        }
    }

    pub(crate) fn add(&mut self, interleaved: &[i16], channels: usize) {
        for s in interleaved {
            self.checksum = (self.checksum ^ u64::from(*s as u16)).wrapping_mul(FNV_PRIME);
        }
        self.frames += (interleaved.len() / channels) as u64;
    }
}

pub(crate) fn encode(src: &mut OwnWavReader, dest: &Path) -> Result<Digest, TakeError> {
    let channels = src.channels();
    let rate = src.rate_hz();
    let config = Encoder::default().into_verified().map_err(enc_err)?;
    let mut stream = Stream::new(rate as usize, channels, 16).map_err(enc_err)?;
    stream
        .stream_info_mut()
        .set_block_sizes(BLOCK, BLOCK)
        .map_err(enc_err)?;
    let mut buffers = (
        FrameBuf::with_size(channels, BLOCK).map_err(enc_err)?,
        Context::new(16, channels),
    );

    let mut out = BufWriter::new(File::create(dest).map_err(io_err)?);
    let placeholder = header_bytes(&stream)?;
    out.write_all(&placeholder).map_err(io_err)?;

    let mut digest = Digest::new();
    let mut pcm = Vec::new();
    let mut wide: Vec<i32> = Vec::new();
    loop {
        if src.read_frames(&mut pcm, BLOCK)? == 0 {
            break;
        }
        digest.add(&pcm, channels);
        wide.clear();
        wide.extend(pcm.iter().map(|s| i32::from(*s)));
        buffers.fill_interleaved(&wide).map_err(enc_err)?;
        let number = buffers.1.current_frame_number().unwrap_or(0);
        let frame =
            flacenc::encode_fixed_size_frame(&config, &buffers.0, number, stream.stream_info())
                .map_err(enc_err)?;
        stream.stream_info_mut().update_frame_info(&frame);
        let mut sink = ByteSink::new();
        frame.write(&mut sink).map_err(enc_err)?;
        out.write_all(sink.as_slice()).map_err(io_err)?;
    }
    // `update_frame_info` lets a short last frame lower the minimum block size, but the FLAC
    // format excludes the last block from it and decoders reject a minimum below 16.
    let (min_block, max_block) = if digest.frames >= BLOCK as u64 {
        (BLOCK, BLOCK)
    } else {
        let only = usize::try_from(digest.frames).unwrap_or(BLOCK).max(16);
        (only, only)
    };
    stream
        .stream_info_mut()
        .set_block_sizes(min_block, max_block)
        .map_err(enc_err)?;
    stream
        .stream_info_mut()
        .set_md5_digest(&buffers.1.md5_digest());
    stream
        .stream_info_mut()
        .set_total_samples(buffers.1.total_samples());

    let header = header_bytes(&stream)?;
    debug_assert_eq!(header.len(), placeholder.len());
    out.flush().map_err(io_err)?;
    let mut file = out.into_inner().map_err(|e| io_err(e.into_error()))?;
    file.seek(SeekFrom::Start(0)).map_err(io_err)?;
    file.write_all(&header).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    Ok(digest)
}

/// The `fLaC` marker and the STREAMINFO block, which has a fixed size, so it can be rewritten in
/// place once the totals are known.
fn header_bytes(stream: &Stream) -> Result<Vec<u8>, TakeError> {
    let mut sink = ByteSink::new();
    stream.write(&mut sink).map_err(enc_err)?;
    Ok(sink.as_slice().to_vec())
}

/// Decodes `path` and checks it against what the encoder read: stream info, frame count, and a
/// checksum over every sample.
pub(crate) fn verify(
    path: &Path,
    want: Digest,
    channels: usize,
    rate_hz: u32,
) -> Result<(), TakeError> {
    let mut reader = FlacReader::open(path)?;
    if reader.channels() != channels || reader.rate_hz() != rate_hz {
        return Err(TakeError::Decode(
            "sealed FLAC has a different format than its WAV".into(),
        ));
    }
    // A stream info that says zero means "unknown" to the decoder, which is right for an empty take.
    if reader.total_frames().unwrap_or(0) != want.frames {
        return Err(TakeError::Decode(format!(
            "sealed FLAC claims {:?} frames, the WAV has {}",
            reader.total_frames(),
            want.frames
        )));
    }
    let mut got = Digest::new();
    let mut pcm = Vec::new();
    while reader.read_frames(&mut pcm, 16_384)? > 0 {
        got.add(&pcm, channels);
    }
    if got != want {
        return Err(TakeError::Decode(format!(
            "sealed FLAC decodes to {} frames with checksum {:x}, the WAV had {} with {:x}",
            got.frames, got.checksum, want.frames, want.checksum
        )));
    }
    Ok(())
}
