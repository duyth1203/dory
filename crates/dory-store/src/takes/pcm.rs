//! Readers that turn a take file into interleaved 16-bit frames, whatever the container.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use dory_core::domain::{Channel, ChannelLayout};
use dory_core::ports::TakeError;

use super::io_err;

/// Where a decoder stands: interleaved `i16` frames at `rate_hz`.
pub(crate) trait FrameReader {
    fn channels(&self) -> usize;
    fn rate_hz(&self) -> u32;
    /// Replaces `out` with up to `max_frames` interleaved frames. Returns the frame count, zero
    /// at the end of the file.
    fn read_frames(&mut self, out: &mut Vec<i16>, max_frames: usize) -> Result<usize, TakeError>;

    fn skip_frames(&mut self, mut frames: u64) -> Result<(), TakeError> {
        let mut scratch = Vec::new();
        while frames > 0 {
            let want = usize::try_from(frames.min(16_384)).unwrap_or(16_384);
            let got = self.read_frames(&mut scratch, want)?;
            if got == 0 {
                break;
            }
            frames -= got as u64;
        }
        Ok(())
    }
}

/// Length of the header `hound` writes for 16-bit PCM.
pub(crate) const WAV_HEADER_LEN: u64 = 44;

/// The header of a WAV this crate wrote: canonical 44 bytes, 16-bit PCM.
pub(crate) struct OwnWavHeader {
    pub channels: u16,
    pub rate_hz: u32,
}

pub(crate) fn read_own_wav_header(file: &mut File) -> Result<OwnWavHeader, TakeError> {
    let mut header = [0u8; WAV_HEADER_LEN as usize];
    file.seek(SeekFrom::Start(0)).map_err(io_err)?;
    file.read_exact(&mut header).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            TakeError::Decode("WAV file is shorter than its header".into())
        } else {
            io_err(e)
        }
    })?;
    let u16_at = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
    let u32_at = |at: usize| {
        u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let canonical = &header[0..4] == b"RIFF"
        && &header[8..12] == b"WAVE"
        && &header[12..16] == b"fmt "
        && u32_at(16) == 16
        && u16_at(20) == 1
        && u16_at(34) == 16
        && &header[36..40] == b"data";
    let channels = u16_at(22);
    let rate_hz = u32_at(24);
    if !canonical || channels == 0 || rate_hz == 0 {
        return Err(TakeError::Decode(
            "not a 16-bit PCM WAV written by Dory".into(),
        ));
    }
    Ok(OwnWavHeader { channels, rate_hz })
}

/// Frames in a WAV this crate wrote, from the file length. The header's own length field is stale
/// after a crash (it is updated only at a checkpoint), and `hound` writes nothing after the data,
/// so the file length is the truth. A torn last frame is ignored.
pub(crate) fn own_wav_frames(file: &File, header: &OwnWavHeader) -> Result<u64, TakeError> {
    let len = file.metadata().map_err(io_err)?.len();
    let data = len.saturating_sub(WAV_HEADER_LEN);
    Ok(data / (2 * u64::from(header.channels)))
}

pub(crate) struct OwnWavReader {
    file: BufReader<File>,
    channels: usize,
    rate_hz: u32,
    frames_left: u64,
    bytes: Vec<u8>,
}

impl OwnWavReader {
    pub(crate) fn open(path: &Path) -> Result<Self, TakeError> {
        let mut file = File::open(path).map_err(io_err)?;
        let header = read_own_wav_header(&mut file)?;
        let frames_left = own_wav_frames(&file, &header)?;
        file.seek(SeekFrom::Start(WAV_HEADER_LEN)).map_err(io_err)?;
        Ok(Self {
            file: BufReader::with_capacity(1 << 16, file),
            channels: usize::from(header.channels),
            rate_hz: header.rate_hz,
            frames_left,
            bytes: Vec::new(),
        })
    }

    pub(crate) fn total_frames(&self) -> u64 {
        self.frames_left
    }
}

impl FrameReader for OwnWavReader {
    fn channels(&self) -> usize {
        self.channels
    }

    fn rate_hz(&self) -> u32 {
        self.rate_hz
    }

    fn read_frames(&mut self, out: &mut Vec<i16>, max_frames: usize) -> Result<usize, TakeError> {
        out.clear();
        let frames = usize::try_from(self.frames_left)
            .unwrap_or(usize::MAX)
            .min(max_frames);
        if frames == 0 {
            return Ok(0);
        }
        self.bytes.resize(frames * self.channels * 2, 0);
        self.file.read_exact(&mut self.bytes).map_err(io_err)?;
        out.extend(
            self.bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b)),
        );
        self.frames_left -= frames as u64;
        Ok(frames)
    }

    fn skip_frames(&mut self, frames: u64) -> Result<(), TakeError> {
        let frames = frames.min(self.frames_left);
        let bytes = i64::try_from(frames * self.channels as u64 * 2).unwrap_or(i64::MAX);
        self.file.seek_relative(bytes).map_err(io_err)?;
        self.frames_left -= frames;
        Ok(())
    }
}

/// Any well-formed WAV (an imported file, ffmpeg output). Every sample format becomes `i16`.
pub(crate) struct HoundReader {
    reader: hound::WavReader<BufReader<File>>,
    spec: hound::WavSpec,
}

pub(crate) fn hound_err(e: hound::Error) -> TakeError {
    match e {
        hound::Error::IoError(e) => io_err(e),
        other => TakeError::Decode(other.to_string()),
    }
}

impl HoundReader {
    pub(crate) fn open(path: &Path) -> Result<Self, TakeError> {
        let reader = hound::WavReader::open(path).map_err(hound_err)?;
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(TakeError::Decode(
                "WAV has no channels or no sample rate".into(),
            ));
        }
        match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Int, 8 | 16 | 24 | 32) | (hound::SampleFormat::Float, 32) => {}
            (format, bits) => {
                return Err(TakeError::Decode(format!(
                    "unsupported WAV sample format {format:?} {bits} bit"
                )));
            }
        }
        Ok(Self { reader, spec })
    }
}

impl FrameReader for HoundReader {
    fn channels(&self) -> usize {
        usize::from(self.spec.channels)
    }

    fn rate_hz(&self) -> u32 {
        self.spec.sample_rate
    }

    fn read_frames(&mut self, out: &mut Vec<i16>, max_frames: usize) -> Result<usize, TakeError> {
        out.clear();
        let want = max_frames * self.channels();
        let spec = self.spec;
        match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, _) => {
                for sample in self.reader.samples::<f32>().take(want) {
                    let s = sample.map_err(hound_err)?;
                    out.push((s.clamp(-1.0, 1.0) * 32767.0).round() as i16);
                }
            }
            (hound::SampleFormat::Int, bits) => {
                for sample in self.reader.samples::<i32>().take(want) {
                    let s = sample.map_err(hound_err)?;
                    out.push(match bits {
                        8 => (s << 8) as i16,
                        16 => s as i16,
                        bits => (s >> (bits - 16)) as i16,
                    });
                }
            }
        }
        let whole = out.len() / self.channels();
        out.truncate(whole * self.channels());
        Ok(whole)
    }
}

/// A FLAC file. `claxon` decodes one block at a time.
pub(crate) struct FlacReader {
    reader: claxon::FlacReader<BufReader<File>>,
    channels: usize,
    rate_hz: u32,
    shift: i32,
    spare: Vec<i32>,
    carry: Vec<i16>,
    pos: usize,
}

pub(crate) fn flac_err(e: claxon::Error) -> TakeError {
    match e {
        claxon::Error::IoError(e) => io_err(e),
        other => TakeError::Decode(format!("FLAC: {other}")),
    }
}

impl FlacReader {
    pub(crate) fn open(path: &Path) -> Result<Self, TakeError> {
        let file = File::open(path).map_err(io_err)?;
        let reader =
            claxon::FlacReader::new(BufReader::with_capacity(1 << 16, file)).map_err(flac_err)?;
        let info = reader.streaminfo();
        if info.channels == 0 || info.sample_rate == 0 || !(4..=32).contains(&info.bits_per_sample)
        {
            return Err(TakeError::Decode("FLAC stream info is not usable".into()));
        }
        Ok(Self {
            channels: info.channels as usize,
            rate_hz: info.sample_rate,
            shift: info.bits_per_sample as i32 - 16,
            reader,
            spare: Vec::new(),
            carry: Vec::new(),
            pos: 0,
        })
    }

    pub(crate) fn total_frames(&self) -> Option<u64> {
        self.reader.streaminfo().samples
    }
}

impl FrameReader for FlacReader {
    fn channels(&self) -> usize {
        self.channels
    }

    fn rate_hz(&self) -> u32 {
        self.rate_hz
    }

    fn read_frames(&mut self, out: &mut Vec<i16>, max_frames: usize) -> Result<usize, TakeError> {
        out.clear();
        let want = max_frames * self.channels;
        while self.carry.len() - self.pos < want {
            let buffer = std::mem::take(&mut self.spare);
            let Some(block) = self
                .reader
                .blocks()
                .read_next_or_eof(buffer)
                .map_err(flac_err)?
            else {
                break;
            };
            self.carry.drain(..self.pos);
            self.pos = 0;
            for i in 0..block.duration() {
                for ch in 0..block.channels() {
                    let s = block.sample(ch, i);
                    self.carry.push(if self.shift >= 0 {
                        (s >> self.shift) as i16
                    } else {
                        (s << -self.shift) as i16
                    });
                }
            }
            self.spare = block.into_buffer();
        }
        let take = want.min(self.carry.len() - self.pos);
        out.extend_from_slice(&self.carry[self.pos..self.pos + take]);
        self.pos += take;
        Ok(take / self.channels)
    }
}

/// Which samples of an interleaved frame make the requested mono channel.
pub(crate) enum Pick {
    One(usize),
    Average,
}

impl Pick {
    /// `layout` is `None` when the sidecar is missing, which only a single-channel file survives.
    pub(crate) fn new(
        layout: Option<ChannelLayout>,
        channels: usize,
        wanted: Channel,
    ) -> Result<Pick, TakeError> {
        if channels == 1 {
            return Ok(Pick::One(0));
        }
        match layout {
            Some(ChannelLayout::Mixed) => Ok(Pick::Average),
            Some(layout) if layout.channel_count() == channels => match layout.index_of(wanted) {
                Some(index) => Ok(Pick::One(index)),
                None if wanted == Channel::Mixed => Ok(Pick::Average),
                None => Err(TakeError::Decode(format!("take has no {wanted:?} channel"))),
            },
            _ if wanted == Channel::Mixed => Ok(Pick::Average),
            _ => Err(TakeError::Decode(format!(
                "cannot tell which of the {channels} channels is {wanted:?}"
            ))),
        }
    }

    pub(crate) fn mono(&self, interleaved: &[i16], channels: usize, out: &mut Vec<f32>) {
        out.clear();
        match self {
            Pick::One(index) => out.extend(
                interleaved
                    .chunks_exact(channels)
                    .map(|frame| f32::from(frame[*index]) / 32768.0),
            ),
            Pick::Average => out.extend(interleaved.chunks_exact(channels).map(|frame| {
                frame.iter().map(|s| f32::from(*s)).sum::<f32>() / (channels as f32 * 32768.0)
            })),
        }
    }
}
