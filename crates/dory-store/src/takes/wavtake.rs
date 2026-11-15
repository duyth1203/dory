//! The WAV that capture appends to. 16-bit PCM: half the disk of float, lossless into the FLAC
//! that follows (which is 16-bit too), and the 44-byte header `hound` writes is what the crash
//! recovery reads.
//!
//! Every `write` reaches the OS before it returns, so a killed process loses nothing it wrote.
//! The header's length fields are stale until a checkpoint, which is why readers take the length
//! from the file size instead.

use std::fs::{File, OpenOptions};
use std::num::NonZeroU32;
use std::path::Path;

use dory_core::domain::{Ms, TakeContainer, TakeInfo};
use dory_core::ports::{TakeError, TakeWriter};
use hound::{SampleFormat, WavSpec, WavWriter};

use super::io_err;
use super::pcm::hound_err;
use super::resample::to_i16;

const WRITE_CHUNK: usize = 16_384;

pub(crate) struct WavTake {
    writer: Option<WavWriter<File>>,
    sync: File,
    channels: usize,
    rate_hz: NonZeroU32,
    frames: u64,
}

impl WavTake {
    /// Creates `path` and fails with `AlreadyExists` (as `Io`) if it is there.
    pub(crate) fn create(path: &Path, channels: usize, rate_hz: u32) -> Result<Self, TakeError> {
        let rate = NonZeroU32::new(rate_hz).ok_or_else(|| {
            io_err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "sample rate is zero",
            ))
        })?;
        let channels_u16 = u16::try_from(channels)
            .ok()
            .filter(|c| *c > 0)
            .ok_or_else(|| {
                io_err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "bad channel count",
                ))
            })?;
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(path)
            .map_err(io_err)?;
        let sync = file.try_clone().map_err(io_err)?;
        let writer = WavWriter::new(
            file,
            WavSpec {
                channels: channels_u16,
                sample_rate: rate_hz,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .map_err(hound_err)?;
        Ok(Self {
            writer: Some(writer),
            sync,
            channels,
            rate_hz: rate,
            frames: 0,
        })
    }

    fn writer(&mut self) -> &mut WavWriter<File> {
        self.writer
            .as_mut()
            .expect("the writer is present until finish")
    }

    pub(crate) fn write_i16(&mut self, samples: &[i16]) -> Result<(), TakeError> {
        if !samples.len().is_multiple_of(self.channels) {
            return Err(io_err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "samples are not whole frames",
            )));
        }
        for chunk in samples.chunks(WRITE_CHUNK) {
            let mut sink = self.writer().get_i16_writer(chunk.len() as u32);
            for s in chunk {
                sink.write_sample(*s);
            }
            sink.flush().map_err(hound_err)?;
        }
        self.frames += (samples.len() / self.channels) as u64;
        Ok(())
    }

    pub(crate) fn finish_info(&mut self) -> Result<TakeInfo, TakeError> {
        let writer = self.writer.take().expect("finish runs once");
        writer.finalize().map_err(hound_err)?;
        self.sync.sync_all().map_err(io_err)?;
        Ok(TakeInfo {
            container: TakeContainer::Wav,
            rate_hz: self.rate_hz.get(),
            duration: Ms::from_frames(self.frames, self.rate_hz),
        })
    }
}

impl TakeWriter for WavTake {
    fn write(&mut self, interleaved: &[f32]) -> Result<(), TakeError> {
        if !interleaved.len().is_multiple_of(self.channels) {
            return Err(io_err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "samples are not whole frames",
            )));
        }
        let mut quantized = Vec::with_capacity(interleaved.len().min(WRITE_CHUNK));
        for chunk in interleaved.chunks(WRITE_CHUNK - WRITE_CHUNK % self.channels) {
            quantized.clear();
            quantized.extend(chunk.iter().map(|s| to_i16(*s)));
            self.write_i16(&quantized)?;
        }
        Ok(())
    }

    fn checkpoint(&mut self) -> Result<(), TakeError> {
        self.writer().flush().map_err(hound_err)?;
        self.sync.sync_data().map_err(io_err)
    }

    fn frames_written(&self) -> u64 {
        self.frames
    }

    fn finish(mut self: Box<Self>) -> Result<TakeInfo, TakeError> {
        self.finish_info()
    }
}
