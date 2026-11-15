//! Streaming downsampler to the 16 kHz mono that speech engines read. An FFT resampler with a
//! Blackman-Harris window cuts everything above the new Nyquist before it can alias, so a 12 kHz
//! tone does not fold down to 4 kHz.

use dory_core::ports::TakeError;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

pub(crate) const TARGET_RATE_HZ: u32 = 16_000;
const CHUNK: usize = 1024;

pub(crate) struct Downsampler {
    inner: Option<Inner>,
}

struct Inner {
    rs: Fft<f32>,
    in_rate: u32,
    pending: Vec<f32>,
    scratch: Vec<f32>,
    to_trim: usize,
    in_frames: u64,
    out_frames: u64,
}

fn resample_err(e: impl std::fmt::Display) -> TakeError {
    TakeError::Decode(format!("resampling failed: {e}"))
}

pub(crate) fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

impl Downsampler {
    pub(crate) fn new(in_rate_hz: u32) -> Result<Self, TakeError> {
        if in_rate_hz == 0 {
            return Err(TakeError::Decode("sample rate is zero".into()));
        }
        if in_rate_hz == TARGET_RATE_HZ {
            return Ok(Self { inner: None });
        }
        let rs = Fft::<f32>::new(
            in_rate_hz as usize,
            TARGET_RATE_HZ as usize,
            CHUNK,
            1,
            FixedSync::Both,
        )
        .map_err(resample_err)?;
        Ok(Self {
            inner: Some(Inner {
                to_trim: rs.output_delay(),
                rs,
                in_rate: in_rate_hz,
                pending: Vec::new(),
                scratch: Vec::new(),
                in_frames: 0,
                out_frames: 0,
            }),
        })
    }

    /// Appends the resampled audio for `mono` to `out`.
    pub(crate) fn feed(&mut self, mono: &[f32], out: &mut Vec<i16>) -> Result<(), TakeError> {
        let Some(inner) = &mut self.inner else {
            out.extend(mono.iter().map(|s| to_i16(*s)));
            return Ok(());
        };
        inner.in_frames += mono.len() as u64;
        inner.pending.extend_from_slice(mono);
        loop {
            let need = inner.rs.input_frames_next();
            if inner.pending.len() < need {
                return Ok(());
            }
            let chunk: Vec<f32> = inner.pending.drain(..need).collect();
            inner.run(&chunk, None, out, u64::MAX)?;
        }
    }

    /// Flushes the filter delay and appends the tail, so the total is exactly the input length
    /// scaled to 16 kHz.
    pub(crate) fn finish(&mut self, out: &mut Vec<i16>) -> Result<(), TakeError> {
        let Some(inner) = &mut self.inner else {
            return Ok(());
        };
        let expected = (u128::from(inner.in_frames) * u128::from(TARGET_RATE_HZ)
            + u128::from(inner.in_rate) / 2)
            / u128::from(inner.in_rate);
        let expected = u64::try_from(expected).unwrap_or(u64::MAX);
        let need = inner.rs.input_frames_next();
        if !inner.pending.is_empty() {
            let mut padded = std::mem::take(&mut inner.pending);
            let valid = padded.len();
            padded.resize(need, 0.0);
            inner.run(&padded, Some(valid), out, expected)?;
        }
        let silence = vec![0.0f32; inner.rs.input_frames_next()];
        let mut guard = 0;
        while inner.out_frames < expected {
            inner.run(&silence, Some(0), out, expected)?;
            guard += 1;
            if guard > 64 {
                return Err(resample_err("the filter never produced the tail"));
            }
        }
        Ok(())
    }
}

impl Inner {
    fn run(
        &mut self,
        chunk: &[f32],
        partial: Option<usize>,
        out: &mut Vec<i16>,
        limit: u64,
    ) -> Result<(), TakeError> {
        let input = InterleavedSlice::new(chunk, 1, chunk.len()).map_err(resample_err)?;
        let capacity = self.rs.output_frames_max();
        self.scratch.resize(capacity, 0.0);
        let mut output =
            InterleavedSlice::new_mut(&mut self.scratch[..], 1, capacity).map_err(resample_err)?;
        let indexing = partial.map(|n| Indexing::new().partial_len(n));
        let (_, produced) = self
            .rs
            .process_into_buffer(&input, &mut output, indexing.as_ref())
            .map_err(resample_err)?;
        let skip = self.to_trim.min(produced);
        self.to_trim -= skip;
        let room = limit.saturating_sub(self.out_frames);
        let keep = (produced - skip).min(usize::try_from(room).unwrap_or(usize::MAX));
        out.extend(self.scratch[skip..skip + keep].iter().map(|s| to_i16(*s)));
        self.out_frames += keep as u64;
        Ok(())
    }
}
