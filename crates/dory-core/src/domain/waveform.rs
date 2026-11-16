//! How loud a take is over time, for the player's waveform.
//!
//! A take adapter measures [`Levels`] with a [`LevelMeter`] in one pass over the samples. The
//! core turns them into a [`Waveform`], the heights the player draws.

/// Buckets per take, whatever its length. A two-hour take gets one per 7.2 seconds.
pub const BUCKETS: usize = 1_000;

/// The RMS of each bucket of a take, 0 for digital silence to 1 for a full-scale square wave,
/// taken from whichever channel is louder in that bucket. At most [`BUCKETS`] long. A take
/// shorter than that many frames gets one bucket per frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels(Vec<f32>);

impl Levels {
    /// Values outside 0..=1, and NaN, are clamped, so a bad cache cannot draw a bar past the card.
    pub fn new(values: Vec<f32>) -> Levels {
        Levels(
            values
                .into_iter()
                .map(|v| if v.is_nan() { 0. } else { v.clamp(0., 1.) })
                .collect(),
        )
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

/// Measures [`Levels`] from interleaved frames fed in order, in constant memory. The frame count
/// must be known up front, so each bucket covers an even share of the take.
pub struct LevelMeter {
    channels: usize,
    total: u64,
    buckets: usize,
    seen: u64,
    next_edge: u64,
    squares: Vec<f64>,
    frames: u64,
    levels: Vec<f32>,
}

impl LevelMeter {
    /// `total_frames` is the take's length. Frames past it count toward the last bucket, and
    /// buckets never reached stay silent.
    pub fn new(total_frames: u64, channels: usize) -> LevelMeter {
        let buckets = usize::try_from(total_frames).map_or(BUCKETS, |t| t.min(BUCKETS));
        let mut meter = LevelMeter {
            channels: channels.max(1),
            total: total_frames,
            buckets,
            seen: 0,
            next_edge: 0,
            squares: vec![0.; channels.max(1)],
            frames: 0,
            levels: Vec::with_capacity(buckets),
        };
        meter.next_edge = meter.edge(0);
        meter
    }

    /// The first frame after bucket `index`.
    fn edge(&self, index: usize) -> u64 {
        let edge = (index as u128 + 1) * u128::from(self.total) / self.buckets.max(1) as u128;
        u64::try_from(edge).unwrap_or(u64::MAX)
    }

    /// `interleaved` holds whole frames, each sample in -1..=1.
    pub fn feed(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(self.channels) {
            if self.seen >= self.next_edge && self.levels.len() + 1 < self.buckets {
                self.close();
            }
            for (sum, sample) in self.squares.iter_mut().zip(frame) {
                *sum += f64::from(*sample) * f64::from(*sample);
            }
            self.frames += 1;
            self.seen += 1;
        }
    }

    fn close(&mut self) {
        let frames = self.frames.max(1) as f64;
        let loudest = self
            .squares
            .iter()
            .map(|sum| (sum / frames).sqrt())
            .fold(0., f64::max);
        self.levels.push(loudest as f32);
        self.squares.iter_mut().for_each(|sum| *sum = 0.);
        self.frames = 0;
        self.next_edge = self.edge(self.levels.len());
    }

    pub fn finish(mut self) -> Levels {
        if self.frames > 0 {
            self.close();
        }
        self.levels.resize(self.buckets, 0.);
        Levels::new(self.levels)
    }
}

/// Bar heights for the player, each 0..=1 of the waveform's height, one per bucket of the take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waveform(Vec<f32>);

impl Waveform {
    /// The height of a silent bucket, so silence still draws a thin bar.
    pub const FLOOR: f32 = 0.06;
    /// Levels this far below the top of the scale draw at the floor.
    const RANGE_DB: f32 = 40.;
    /// The top of the scale is the take's loudest bucket, but never below this, so a take of
    /// room noise stays low instead of being blown up to full height.
    const MIN_TOP_DB: f32 = -30.;

    /// Loudness on a decibel scale, which matches how loud speech sounds better than amplitude
    /// does: the loudest bucket is full height, and each bucket falls toward the floor as it
    /// gets quieter than that.
    pub fn from_levels(levels: &Levels) -> Waveform {
        let db = |level: f32| 20. * level.max(1e-9).log10();
        let top = levels
            .as_slice()
            .iter()
            .copied()
            .map(db)
            .fold(Self::MIN_TOP_DB, f32::max);
        let bottom = top - Self::RANGE_DB;
        Waveform(
            levels
                .as_slice()
                .iter()
                .map(|level| {
                    let above = ((db(*level) - bottom) / Self::RANGE_DB).clamp(0., 1.);
                    Self::FLOOR + (1. - Self::FLOOR) * above
                })
                .collect(),
        )
    }

    pub fn heights(&self) -> &[f32] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `count` bar heights spread over the whole take. A bar that covers several buckets takes
    /// the loudest, so a short burst of speech never disappears at a narrow width.
    pub fn bars(&self, count: usize) -> Vec<f32> {
        let buckets = self.0.len();
        if buckets == 0 {
            return vec![Self::FLOOR; count];
        }
        (0..count)
            .map(|bar| {
                let start = bar * buckets / count;
                let end = ((bar + 1) * buckets / count).max(start + 1);
                self.0[start..end].iter().copied().fold(0., f32::max)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, amplitude: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| amplitude * (i as f32 * 0.05).sin())
            .collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_long_take_gets_the_fixed_bucket_count() {
        let mut meter = LevelMeter::new(100_000, 1);
        meter.feed(&sine(100_000, 0.5));
        assert_eq!(meter.finish().as_slice().len(), BUCKETS);
    }

    #[test]
    fn a_take_shorter_than_the_bucket_count_gets_a_bucket_per_frame() {
        let mut meter = LevelMeter::new(3, 1);
        meter.feed(&[0.5, -0.25, 0.]);
        assert_eq!(meter.finish().as_slice(), &[0.5, 0.25, 0.]);
    }

    #[test]
    fn each_bucket_is_the_rms_of_its_share_of_the_take() {
        let mut meter = LevelMeter::new(4_000, 1);
        let mut samples = vec![0.5f32; 2_000];
        samples.extend([0.; 2_000]);
        meter.feed(&samples);
        let levels = meter.finish();
        assert!(levels.as_slice()[..500].iter().all(|l| close(*l, 0.5)));
        assert!(levels.as_slice()[500..].iter().all(|l| *l == 0.));
    }

    #[test]
    fn the_louder_channel_sets_the_bucket() {
        let mut meter = LevelMeter::new(2_000, 2);
        let frames: Vec<f32> = (0..2_000)
            .flat_map(|i| if i < 1_000 { [0.1, 0.] } else { [0., 0.8] })
            .collect();
        meter.feed(&frames);
        let levels = meter.finish();
        assert!(close(levels.as_slice()[0], 0.1));
        assert!(close(levels.as_slice()[999], 0.8));
    }

    #[test]
    fn feeding_in_odd_chunks_measures_the_same() {
        let samples = sine(10_007, 0.3);
        let mut whole = LevelMeter::new(10_007, 1);
        whole.feed(&samples);
        let mut pieces = LevelMeter::new(10_007, 1);
        for chunk in samples.chunks(333) {
            pieces.feed(chunk);
        }
        assert_eq!(whole.finish(), pieces.finish());
    }

    #[test]
    fn a_take_that_ends_early_leaves_silent_buckets_and_one_that_runs_long_fills_the_last() {
        let mut short = LevelMeter::new(2_000, 1);
        short.feed(&vec![0.5; 1_000]);
        let short = short.finish();
        assert_eq!(short.as_slice().len(), BUCKETS);
        assert_eq!(short.as_slice()[BUCKETS - 1], 0.);

        let mut long = LevelMeter::new(1_000, 1);
        long.feed(&vec![0.5; 1_500]);
        let long = long.finish();
        assert_eq!(long.as_slice().len(), BUCKETS);
        assert!(close(long.as_slice()[BUCKETS - 1], 0.5));
    }

    #[test]
    fn an_unknown_length_gives_no_buckets() {
        let mut meter = LevelMeter::new(0, 1);
        meter.feed(&[0.5; 10]);
        assert!(meter.finish().as_slice().is_empty());
    }

    #[test]
    fn levels_are_clamped_to_full_scale() {
        let levels = Levels::new(vec![-1., 2., f32::NAN, 0.5]);
        assert_eq!(levels.as_slice(), &[0., 1., 0., 0.5]);
    }

    #[test]
    fn silence_draws_at_the_floor_and_the_loudest_bucket_at_full_height() {
        let wave = Waveform::from_levels(&Levels::new(vec![0., 0.3, 0.]));
        assert_eq!(wave.heights(), &[Waveform::FLOOR, 1., Waveform::FLOOR]);
    }

    #[test]
    fn heights_follow_decibels_below_the_loudest_bucket() {
        let wave = Waveform::from_levels(&Levels::new(vec![0.1, 0.01, 0.001]));
        let [loud, minus_20, minus_40] = wave.heights() else {
            panic!("three heights");
        };
        assert!(close(*loud, 1.));
        assert!(close(
            *minus_20,
            Waveform::FLOOR + (1. - Waveform::FLOOR) * 0.5
        ));
        assert!(close(*minus_40, Waveform::FLOOR));
    }

    #[test]
    fn a_take_of_quiet_room_noise_stays_low() {
        let noise = 10f32.powf(-50. / 20.);
        let wave = Waveform::from_levels(&Levels::new(vec![noise; 4]));
        assert!(wave.heights().iter().all(|h| *h < 0.6), "{wave:?}");
        assert!(wave.heights().iter().all(|h| *h > Waveform::FLOOR));
    }

    #[test]
    fn a_silent_take_is_all_floor() {
        let wave = Waveform::from_levels(&Levels::new(vec![0.; 5]));
        assert!(wave.heights().iter().all(|h| *h == Waveform::FLOOR));
    }

    #[test]
    fn fewer_bars_than_buckets_keep_the_loudest() {
        let wave = Waveform(vec![0.1, 0.9, 0.1, 0.1, 0.2, 0.3]);
        assert_eq!(wave.bars(2), vec![0.9, 0.3]);
    }

    #[test]
    fn more_bars_than_buckets_repeat_them() {
        let wave = Waveform(vec![0.2, 0.8]);
        assert_eq!(wave.bars(4), vec![0.2, 0.2, 0.8, 0.8]);
    }

    #[test]
    fn an_empty_waveform_draws_flat_floor_bars() {
        assert_eq!(Waveform::default().bars(3), vec![Waveform::FLOOR; 3]);
    }
}
