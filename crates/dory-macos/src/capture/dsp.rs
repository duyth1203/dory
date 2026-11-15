//! Pure signal logic for the Core Audio adapter. Nothing here touches the HAL, so every function
//! is unit-tested without hardware. The real-time path calls only `cycle_frames`,
//! `InputMap::presence`, `render`, `LinearResampler::process` (with `AntiAlias::run`) and
//! `SilenceRun::observe`. None of them allocate or panic: every sample read is bounds-checked and
//! reads silence when out of range.

use std::time::Duration;

use dory_core::domain::{Channel, ChannelLayout};

/// Most channels one mono source may average. A source with more channels reads its first one.
pub const MAX_PICKS: usize = 8;
/// Most buffers the aggregate's input list may hold.
pub const MAX_BUFFERS: usize = 32;

/// The shape of one `AudioBuffer`: how many interleaved channels it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferShape {
    pub channels: usize,
}

/// What a run of input buffers in the aggregate carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Mic,
    System,
}

/// The buffers one sub-device or sub-tap contributes, in the order the aggregate lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub role: Role,
    pub buffers: Vec<BufferShape>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pick {
    buffer: u8,
    channel: u8,
    stride: u8,
}

/// Where one mono source reads in the input buffer list. Its sample is the mean of its picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonoSource {
    picks: [Pick; MAX_PICKS],
    len: usize,
}

impl MonoSource {
    fn from_segment(first_buffer: usize, buffers: &[BufferShape], role: Role) -> Option<Self> {
        let mut all = [Pick::default(); MAX_PICKS];
        let mut total = 0usize;
        for (offset, shape) in buffers.iter().enumerate() {
            for channel in 0..shape.channels {
                if total < MAX_PICKS {
                    all[total] = Pick {
                        buffer: (first_buffer + offset) as u8,
                        channel: channel as u8,
                        stride: shape.channels as u8,
                    };
                }
                total += 1;
            }
        }
        if total == 0 {
            return None;
        }
        // A tap is a stereo mixdown, so both sides are averaged. A mic with one or two channels is
        // averaged too. A multi-input interface would mix unrelated inputs, so it reads input 1.
        let len = match role {
            Role::Mic if total > 2 => 1,
            _ => total.min(MAX_PICKS),
        };
        Some(Self { picks: all, len })
    }

    /// Picked channel count, for diagnostics.
    pub fn width(&self) -> usize {
        self.len
    }

    #[inline]
    fn sample(&self, buffers: &[Option<&[f32]>], frame: usize) -> f32 {
        let mut sum = 0.0f32;
        for pick in self.picks.iter().take(self.len) {
            let index = frame * pick.stride as usize + pick.channel as usize;
            let data = buffers.get(pick.buffer as usize).copied().flatten();
            sum += data.and_then(|d| d.get(index)).copied().unwrap_or(0.0);
        }
        sum / self.len as f32
    }

    /// True when every buffer this source reads holds at least `frames` whole frames.
    fn present(&self, buffers: &[Option<&[f32]>], frames: usize) -> bool {
        self.picks.iter().take(self.len).all(|pick| {
            let data = buffers.get(pick.buffer as usize).copied().flatten();
            data.is_some_and(|d| d.len() / pick.stride.max(1) as usize >= frames)
        })
    }
}

/// Which input buffers feed the mic and system channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputMap {
    pub mic: Option<MonoSource>,
    pub system: Option<MonoSource>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LayoutError {
    Mismatch {
        expected: Vec<BufferShape>,
        actual: Vec<BufferShape>,
    },
    TooManyBuffers(usize),
    MissingSource(Role),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::Mismatch { expected, actual } => {
                let ch = |v: &[BufferShape]| v.iter().map(|b| b.channels).collect::<Vec<_>>();
                write!(
                    f,
                    "aggregate input buffers {:?} do not match its parts {:?}",
                    ch(actual),
                    ch(expected)
                )
            }
            LayoutError::TooManyBuffers(n) => write!(
                f,
                "aggregate has {n} input buffers, at most {MAX_BUFFERS} supported"
            ),
            LayoutError::MissingSource(role) => {
                write!(f, "aggregate has no input channels for {role:?}")
            }
        }
    }
}

/// Maps the aggregate's actual input buffers onto the parts it was built from. The parts are
/// concatenated in aggregate order. Any difference from the actual list is an error, not a guess.
pub fn resolve_inputs(actual: &[BufferShape], parts: &[Segment]) -> Result<InputMap, LayoutError> {
    let expected: Vec<BufferShape> = parts
        .iter()
        .flat_map(|s| s.buffers.iter().copied())
        .collect();
    if expected != actual {
        return Err(LayoutError::Mismatch {
            expected,
            actual: actual.to_vec(),
        });
    }
    if actual.len() > MAX_BUFFERS {
        return Err(LayoutError::TooManyBuffers(actual.len()));
    }
    let mut map = InputMap {
        mic: None,
        system: None,
    };
    let mut first = 0;
    for part in parts {
        let source = MonoSource::from_segment(first, &part.buffers, part.role);
        match part.role {
            Role::Mic => map.mic = map.mic.or(source),
            Role::System => map.system = map.system.or(source),
        }
        first += part.buffers.len();
    }
    for part in parts {
        let missing = match part.role {
            Role::Mic => map.mic.is_none(),
            Role::System => map.system.is_none(),
        };
        if missing {
            return Err(LayoutError::MissingSource(part.role));
        }
    }
    Ok(map)
}

/// Whether each source has data for the whole cycle. A source with a missing, mis-shaped, or
/// short buffer is lost for the cycle and renders as silence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presence {
    pub mic: bool,
    pub system: bool,
}

impl InputMap {
    pub fn presence(&self, buffers: &[Option<&[f32]>], frames: usize) -> Presence {
        Presence {
            mic: self.mic.is_some_and(|m| m.present(buffers, frames)),
            system: self.system.is_some_and(|s| s.present(buffers, frames)),
        }
    }
}

/// Frames in this cycle: the most any valid buffer holds, so one lost source cannot shorten the
/// timeline. `None` when no buffer is valid. `buffers` must already match `shapes`.
pub fn cycle_frames(buffers: &[Option<&[f32]>], shapes: &[BufferShape]) -> Option<usize> {
    buffers
        .iter()
        .zip(shapes)
        .filter_map(|(b, shape)| b.and_then(|d| d.len().checked_div(shape.channels)))
        .max()
}

/// Fills `out` with whole frames starting at input frame `start`, interleaved in `layout` order.
/// A source absent from `presence` writes silence. Returns whether every system sample was zero.
pub fn render(
    map: &InputMap,
    layout: ChannelLayout,
    buffers: &[Option<&[f32]>],
    presence: Presence,
    start: usize,
    out: &mut [f32],
) -> bool {
    let slots = layout.channels();
    let mut system_zero = true;
    for (i, frame) in out.chunks_exact_mut(slots.len().max(1)).enumerate() {
        let at = start + i;
        for (value, channel) in frame.iter_mut().zip(slots) {
            *value = match channel {
                Channel::Mic if presence.mic => map.mic.map_or(0.0, |m| m.sample(buffers, at)),
                Channel::System if presence.system => {
                    let v = map.system.map_or(0.0, |s| s.sample(buffers, at));
                    system_zero &= v == 0.0;
                    v
                }
                _ => 0.0,
            };
        }
    }
    system_zero
}

/// Streaming linear-interpolation resampler over interleaved frames. It keeps the last input frame
/// across calls, so block boundaries are seamless, and its state is fixed-size, so `process` never
/// allocates. When downsampling, an [`AntiAlias`] low-pass runs first, because linear
/// interpolation alone folds everything above the output Nyquist frequency back into the band.
#[derive(Clone, Debug)]
pub struct LinearResampler {
    channels: usize,
    /// Input frames advanced per output frame.
    step: f64,
    /// Position of the next output frame, in input frames, relative to the first frame of the next
    /// block. `-1.0` is the previous block's last frame.
    pos: f64,
    prev: [f32; 2],
    anti_alias: Option<AntiAlias>,
}

impl LinearResampler {
    pub fn new(channels: usize, from_hz: f64, to_hz: f64) -> Self {
        assert!(channels <= 2, "resampler holds at most two channels");
        Self {
            channels,
            step: from_hz / to_hz,
            pos: 0.0,
            prev: [0.0; 2],
            anti_alias: (from_hz > to_hz).then(|| AntiAlias::new(channels, to_hz / from_hz)),
        }
    }

    /// Output frames the next `input_frames` can produce at most. Size output buffers with this.
    pub fn max_output(&self, input_frames: usize) -> usize {
        ((input_frames as f64 + 1.0) / self.step).ceil() as usize + 1
    }

    /// Resamples `input` into `out` and returns the number of frames written. `out` must hold
    /// `max_output(input frames)` frames. When downsampling, `input` is low-passed in place first.
    pub fn process(&mut self, input: &mut [f32], out: &mut [f32]) -> usize {
        if let Some(filter) = &mut self.anti_alias {
            filter.run(input);
        }
        let input = &*input;
        let ch = self.channels;
        let Some(n) = input.len().checked_div(ch).filter(|n| *n > 0) else {
            return 0;
        };
        let prev = self.prev;
        let at = |i: isize, c: usize| {
            let v = if i < 0 {
                prev.get(c)
            } else {
                input.get(i as usize * ch + c)
            };
            v.copied().unwrap_or(0.0)
        };
        let mut pos = self.pos;
        let mut written = 0;
        for frame in out.chunks_exact_mut(ch) {
            if pos >= (n - 1) as f64 {
                break;
            }
            let base = pos.floor();
            let frac = (pos - base) as f32;
            let i = base as isize;
            for (c, sample) in frame.iter_mut().enumerate() {
                let a = at(i, c);
                let b = at(i + 1, c);
                *sample = a + (b - a) * frac;
            }
            written += 1;
            pos += self.step;
        }
        self.pos = pos - n as f64;
        if let Some(last) = input.get((n - 1) * ch..) {
            for (p, s) in self.prev.iter_mut().zip(last) {
                *p = *s;
            }
        }
        written
    }
}

/// Stopband attenuation the anti-alias filter is designed for. The requirement is 40 dB; the
/// margin covers the Kaiser length estimate and f32 rounding.
const ANTI_ALIAS_DB: f64 = 60.0;

/// A linear-phase low-pass FIR (Kaiser-windowed sinc) at the input rate. Its passband ends at 0.42
/// and its stopband starts at 0.5 of the output rate, so everything that would alias is at least
/// [`ANTI_ALIAS_DB`] down.
///
/// An FIR over a biquad cascade: it is unconditionally stable, its design is closed-form for any
/// rate pair, and its constant delay is the same on both channels. It costs one multiply-add per
/// tap per sample: 93 taps from 96 kHz to 48 kHz, about 18 M per second for two channels.
#[derive(Clone, Debug)]
pub struct AntiAlias {
    taps: Box<[f32]>,
    /// Per channel, the last `taps.len()` inputs written twice, so the newest window is always one
    /// contiguous slice: `[at + 1, at + len]`.
    history: Box<[f32]>,
    at: usize,
    channels: usize,
}

impl AntiAlias {
    /// `ratio` is the output rate over the input rate, below 1.
    pub fn new(channels: usize, ratio: f64) -> Self {
        let taps = kaiser_lowpass(0.46 * ratio, 0.08 * ratio, ANTI_ALIAS_DB);
        let len = taps.len();
        Self {
            taps: taps.into_iter().map(|t| t as f32).collect(),
            history: vec![0.0; 2 * len * channels].into_boxed_slice(),
            at: 0,
            channels,
        }
    }

    /// Filters interleaved frames in place.
    pub fn run(&mut self, frames: &mut [f32]) {
        let len = self.taps.len();
        let span = 2 * len;
        for frame in frames.chunks_exact_mut(self.channels.max(1)) {
            for (c, sample) in frame.iter_mut().enumerate() {
                let Some(history) = self.history.get_mut(c * span..(c + 1) * span) else {
                    continue;
                };
                if let Some(slot) = history.get_mut(self.at) {
                    *slot = *sample;
                }
                if let Some(slot) = history.get_mut(self.at + len) {
                    *slot = *sample;
                }
                if let Some(window) = history.get(self.at + 1..self.at + 1 + len) {
                    *sample = window
                        .iter()
                        .zip(self.taps.iter())
                        .map(|(x, t)| x * t)
                        .sum();
                }
            }
            self.at = if self.at + 1 >= len { 0 } else { self.at + 1 };
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.taps.len()
    }
}

/// Kaiser-windowed sinc low-pass taps with unity DC gain. Frequencies are in cycles per sample:
/// `cutoff` is the half-amplitude point and `transition` the width from passband to stopband.
fn kaiser_lowpass(cutoff: f64, transition: f64, attenuation_db: f64) -> Vec<f64> {
    let beta = 0.1102 * (attenuation_db - 8.7);
    let estimate = (attenuation_db - 8.0) / (2.285 * std::f64::consts::TAU * transition);
    let len = (estimate.ceil() as usize + 1) | 1;
    let middle = (len / 2) as f64;
    let mut taps: Vec<f64> = (0..len)
        .map(|k| {
            let x = k as f64 - middle;
            let sinc = if x == 0.0 {
                1.0
            } else {
                let a = std::f64::consts::TAU * cutoff * x;
                a.sin() / a
            };
            let r = x / middle;
            let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
            2.0 * cutoff * sinc * window
        })
        .collect();
    let gain: f64 = taps.iter().sum();
    taps.iter_mut().for_each(|t| *t /= gain);
    taps
}

/// Modified Bessel function of the first kind, order zero, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= half / k as f64;
        let add = term * term;
        sum += add;
        if add < sum * 1e-16 {
            break;
        }
    }
    sum
}

/// Counts consecutive digitally silent system frames. Lives on the audio thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct SilenceRun {
    frames: u64,
}

impl SilenceRun {
    pub fn observe(&mut self, frames: usize, all_zero: bool) -> u64 {
        self.frames = if all_zero {
            self.frames + frames as u64
        } else {
            0
        };
        self.frames
    }
}

/// Decides when to report `SystemAudioSilent`: once per silent stretch that reaches the threshold
/// while some other process is playing audio. Lives on the monitor thread.
#[derive(Clone, Copy, Debug)]
pub struct SilenceAlarm {
    threshold_frames: u64,
    reported: bool,
}

impl SilenceAlarm {
    pub fn new(rate_hz: f64, seconds: f64) -> Self {
        Self {
            threshold_frames: (rate_hz * seconds) as u64,
            reported: false,
        }
    }

    pub fn threshold_frames(&self) -> u64 {
        self.threshold_frames
    }

    /// Returns true exactly when the issue should be reported now.
    pub fn update(&mut self, silent_frames: u64, output_playing: bool) -> bool {
        if silent_frames < self.threshold_frames {
            self.reported = false;
            return false;
        }
        if output_playing && !self.reported {
            self.reported = true;
            return true;
        }
        false
    }
}

/// Decides when the IOProc has stalled: its cycle count has not moved for `limit` while the
/// capture is started. Reports once. Lives on the monitor thread.
#[derive(Clone, Copy, Debug)]
pub struct Watchdog {
    limit: Duration,
    cycles: u64,
    moved_at: Duration,
    reported: bool,
}

impl Watchdog {
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            cycles: 0,
            moved_at: Duration::ZERO,
            reported: false,
        }
    }

    /// `cycles` is the IO cycle count and `now` the time since start. Returns true exactly when
    /// the stall should be reported.
    pub fn update(&mut self, cycles: u64, now: Duration) -> bool {
        if cycles != self.cycles {
            self.cycles = cycles;
            self.moved_at = now;
            return false;
        }
        let stalled = now.saturating_sub(self.moved_at) >= self.limit;
        if stalled && !self.reported {
            self.reported = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes(ch: &[usize]) -> Vec<BufferShape> {
        ch.iter()
            .map(|&channels| BufferShape { channels })
            .collect()
    }

    fn seg(role: Role, ch: &[usize]) -> Segment {
        Segment {
            role,
            buffers: shapes(ch),
        }
    }

    const BOTH: Presence = Presence {
        mic: true,
        system: true,
    };

    fn render_all(
        map: &InputMap,
        layout: ChannelLayout,
        buffers: &[Option<&[f32]>],
        start: usize,
        out: &mut [f32],
    ) -> bool {
        let frames = out.len() / layout.channel_count();
        render(
            map,
            layout,
            buffers,
            map.presence(buffers, start + frames),
            start,
            out,
        )
    }

    #[test]
    fn mic_then_interleaved_stereo_tap_maps_to_mic_and_system() {
        let map = resolve_inputs(
            &shapes(&[1, 2]),
            &[seg(Role::Mic, &[1]), seg(Role::System, &[2])],
        )
        .unwrap();
        let mic = [0.5f32, 0.25];
        let tap = [0.2f32, 0.4, -1.0, 1.0];
        let buffers = [Some(&mic[..]), Some(&tap[..])];
        assert_eq!(map.presence(&buffers, 2), BOTH);
        let mut out = [9.0f32; 4];
        let silent = render(&map, ChannelLayout::MicSystem, &buffers, BOTH, 0, &mut out);
        assert_eq!(out, [0.5, 0.3, 0.25, 0.0]);
        assert!(!silent);
    }

    #[test]
    fn non_interleaved_tap_is_averaged_across_buffers() {
        let map = resolve_inputs(&shapes(&[1, 1]), &[seg(Role::System, &[1, 1])]).unwrap();
        let l = [1.0f32, 0.0];
        let r = [0.0f32, -0.5];
        let mut out = [0.0f32; 2];
        render_all(
            &map,
            ChannelLayout::SystemOnly,
            &[Some(&l[..]), Some(&r[..])],
            0,
            &mut out,
        );
        assert_eq!(out, [0.5, -0.25]);
    }

    #[test]
    fn multi_input_interface_reads_its_first_input() {
        let map = resolve_inputs(&shapes(&[4]), &[seg(Role::Mic, &[4])]).unwrap();
        assert_eq!(map.mic.unwrap().width(), 1);
        let data = [0.1f32, 0.9, 0.9, 0.9];
        let mut out = [0.0f32; 1];
        render_all(
            &map,
            ChannelLayout::MicOnly,
            &[Some(&data[..])],
            0,
            &mut out,
        );
        assert_eq!(out, [0.1]);
    }

    #[test]
    fn layout_that_differs_from_the_parts_is_rejected() {
        let err = resolve_inputs(
            &shapes(&[2, 1]),
            &[seg(Role::Mic, &[1]), seg(Role::System, &[2])],
        )
        .unwrap_err();
        assert!(matches!(err, LayoutError::Mismatch { .. }));
    }

    #[test]
    fn a_role_with_no_channels_is_rejected() {
        let err = resolve_inputs(
            &shapes(&[2]),
            &[seg(Role::Mic, &[]), seg(Role::System, &[2])],
        )
        .unwrap_err();
        assert_eq!(err, LayoutError::MissingSource(Role::Mic));
    }

    #[test]
    fn render_honours_the_start_offset() {
        let map = resolve_inputs(&shapes(&[1]), &[seg(Role::Mic, &[1])]).unwrap();
        let mic = [0.1f32, 0.2, 0.3];
        let mut out = [0.0f32; 2];
        render_all(&map, ChannelLayout::MicOnly, &[Some(&mic[..])], 1, &mut out);
        assert_eq!(out, [0.2, 0.3]);
    }

    fn run(resampler: &mut LinearResampler, input: &[f32], block: usize) -> Vec<f32> {
        let ch = resampler.channels;
        let mut all = Vec::new();
        for chunk in input.chunks(block * ch) {
            let mut chunk = chunk.to_vec();
            let mut out = vec![0.0; resampler.max_output(chunk.len() / ch) * ch];
            let n = resampler.process(&mut chunk, &mut out);
            all.extend_from_slice(&out[..n * ch]);
        }
        all
    }

    #[test]
    fn resampler_rate_ratio_holds_over_many_blocks() {
        let mut r = LinearResampler::new(2, 48_000.0, 16_000.0);
        let input = vec![0.0f32; 48_000 * 2 * 10];
        let out = run(&mut r, &input, 509);
        let frames = out.len() / 2;
        assert!((frames as i64 - 160_000).abs() <= 1, "got {frames} frames");
    }

    #[test]
    fn resampler_upsamples_44k1_to_48k_with_exact_count() {
        let mut r = LinearResampler::new(1, 44_100.0, 48_000.0);
        let out = run(&mut r, &vec![0.0f32; 44_100 * 3], 512);
        assert!((out.len() as i64 - 144_000).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn resampler_is_seamless_across_block_boundaries() {
        let ramp: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.37).sin()).collect();
        for (from, to) in [(3.0, 2.0), (2.0, 3.0)] {
            let a = run(&mut LinearResampler::new(1, from, to), &ramp, 1000);
            let b = run(&mut LinearResampler::new(1, from, to), &ramp, 7);
            assert_eq!(a.len(), b.len(), "{from} -> {to}");
            for (x, y) in a.iter().zip(&b) {
                assert!((x - y).abs() < 1e-5, "{from} -> {to}: {x} vs {y}");
            }
        }
    }

    #[test]
    fn upsampling_interpolates_linearly() {
        let ramp: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let out = run(&mut LinearResampler::new(1, 2.0, 3.0), &ramp, 7);
        assert!(out.len() > 1400);
        for (k, v) in out.iter().enumerate() {
            assert!((v - k as f32 * 2.0 / 3.0).abs() < 1e-3, "frame {k} = {v}");
        }
    }

    #[test]
    fn resampler_preserves_a_speech_band_tone() {
        let w = 440.0 * std::f64::consts::TAU;
        let tone: Vec<f32> = (0..44_100)
            .map(|i| (w * i as f64 / 44_100.0).sin() as f32)
            .collect();
        let mut r = LinearResampler::new(1, 44_100.0, 48_000.0);
        let out = run(&mut r, &tone, 441);
        assert!(out.len() > 47_000);
        for (k, v) in out.iter().enumerate().take(47_000) {
            let expected = (w * k as f64 / 48_000.0).sin() as f32;
            assert!((v - expected).abs() < 1e-3, "frame {k}: {v} vs {expected}");
        }
    }

    /// Output level of a full-scale sine at `hz` resampled `from` -> `to`, in dB relative to the
    /// input level, after the filter has filled.
    fn gain_db(from: f64, to: f64, hz: f64) -> f64 {
        let frames = (from * 0.25) as usize;
        let w = std::f64::consts::TAU * hz / from;
        let tone: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = (w * i as f64).sin() as f32;
                [v, -v]
            })
            .collect();
        let mut r = LinearResampler::new(2, from, to);
        let warmup = r.anti_alias.as_ref().map_or(0, |f| f.len()) * 2;
        let out = run(&mut r, &tone, 509);
        let rms = |v: &[f32]| {
            (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let skip_in = warmup * 2;
        let skip_out = ((warmup as f64 * to / from) as usize + 2) * 2;
        20.0 * (rms(&out[skip_out..]) / rms(&tone[skip_in..])).log10()
    }

    #[test]
    fn downsampling_attenuates_everything_above_the_output_nyquist_by_40_db() {
        for (from, to) in [
            (96_000.0, 48_000.0),
            (88_200.0, 48_000.0),
            (192_000.0, 48_000.0),
            (48_000.0, 16_000.0),
            (48_000.0, 44_100.0),
        ] {
            let nyquist_out: f64 = to / 2.0;
            let top = from / 2.0;
            let mut worst = f64::NEG_INFINITY;
            for k in 0..=12 {
                let hz = nyquist_out + (top - nyquist_out) * (0.001 + 0.99 * k as f64 / 12.0);
                let db = gain_db(from, to, hz);
                worst = worst.max(db);
                assert!(
                    db <= -40.0,
                    "{from} -> {to}: {hz:.0} Hz comes out at {db:.1} dB"
                );
            }
            eprintln!("{from} -> {to}: worst stopband gain {worst:.1} dB");
        }
    }

    #[test]
    fn downsampling_keeps_the_speech_band() {
        for (from, to) in [(96_000.0, 48_000.0), (48_000.0, 16_000.0)] {
            for hz in [200.0, 1_000.0, 3_000.0] {
                let db = gain_db(from, to, hz);
                assert!(db.abs() < 0.1, "{from} -> {to}: {hz} Hz at {db:.3} dB");
            }
        }
    }

    #[test]
    fn the_anti_alias_filter_has_unity_dc_gain_and_odd_symmetric_length() {
        let taps = kaiser_lowpass(0.23, 0.04, ANTI_ALIAS_DB);
        assert_eq!(taps.len() % 2, 1);
        assert!((taps.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        for (a, b) in taps.iter().zip(taps.iter().rev()) {
            assert!((a - b).abs() < 1e-15);
        }
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008).abs() < 1e-12);
    }

    #[test]
    fn silence_run_resets_on_any_signal() {
        let mut run = SilenceRun::default();
        assert_eq!(run.observe(512, true), 512);
        assert_eq!(run.observe(512, true), 1024);
        assert_eq!(run.observe(512, false), 0);
    }

    #[test]
    fn alarm_fires_once_per_silent_stretch_while_output_plays() {
        let mut alarm = SilenceAlarm::new(48_000.0, 3.0);
        assert!(!alarm.update(100_000, true));
        assert!(
            !alarm.update(144_000, false),
            "nothing playing, silence is expected"
        );
        assert!(alarm.update(144_000, true));
        assert!(!alarm.update(200_000, true), "already reported");
        assert!(!alarm.update(0, true));
        assert!(
            alarm.update(150_000, true),
            "re-armed after signal returned"
        );
    }

    #[test]
    fn watchdog_fires_once_after_two_seconds_without_a_cycle() {
        let ms = Duration::from_millis;
        let mut dog = Watchdog::new(ms(2000));
        assert!(
            !dog.update(0, ms(1750)),
            "not yet two seconds without a cycle"
        );
        assert!(dog.update(0, ms(2000)), "the IOProc never ran");
        assert!(!dog.update(0, ms(2250)), "already reported");

        let mut dog = Watchdog::new(ms(2000));
        assert!(!dog.update(10, ms(1000)));
        assert!(
            !dog.update(10, ms(2750)),
            "two seconds since the last cycle is not up yet"
        );
        assert!(dog.update(10, ms(3000)));
    }

    #[test]
    fn watchdog_stays_quiet_while_cycles_advance() {
        let ms = Duration::from_millis;
        let mut dog = Watchdog::new(ms(2000));
        for tick in 1..100u64 {
            assert!(!dog.update(tick, ms(tick * 250)));
        }
    }
}
