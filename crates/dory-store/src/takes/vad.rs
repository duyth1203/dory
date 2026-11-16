//! A cheap energy voice activity detector over the mono clip an engine reads.
//!
//! The core only needs to know where nobody talks, so it can skip those stretches and cut windows
//! there. A false positive costs one engine call on noise, a false negative loses speech, so every
//! threshold leans toward calling a frame speech.
//!
//! Each 30 ms frame's RMS level in dBFS is compared with a noise floor that follows the quiet
//! parts of the clip. A frame is voiced when it is both above an absolute gate and well above the
//! floor. Runs of voiced frames too short to be a word are dropped, the rest get a lead-in and a
//! hangover, and spans separated by less than a breath are merged.

use dory_core::domain::{Ms, TimeRange};

const FRAME_MS: u64 = 30;
/// Below this nothing is speech: digital silence, a quiet room, a call's comfort noise.
const GATE_DBFS: f32 = -60.0;
/// Speech sits 15 to 30 dB above room noise. Half the low end still catches a quiet talker, and it
/// is far above how much a 30 ms frame of steady noise varies.
const MARGIN_DB: f32 = 10.0;
/// The floor starts at this percentile of the clip's frame levels, so a clip that opens with
/// speech does not take speech for the floor. Meetings spend far more than a tenth of their time
/// between words.
const FLOOR_PERCENTILE: usize = 10;
/// Room noise on a capture device sits well below this. A clip whose quietest tenth is louder has
/// someone or something playing nearly throughout, such as a short voice note, so the floor starts
/// where a room's noise would be and adapts from there.
const FLOOR_START_MAX_DBFS: f32 = -40.0;
/// Per frame, the floor drops quickly into a quieter frame and rises with a time constant of
/// about ten seconds, so the pauses inside speech keep pulling it back down.
const FLOOR_FALL: f32 = 0.3;
const FLOOR_RISE: f32 = 0.003;
const FLOOR_MIN_DBFS: f32 = -90.0;
/// Shorter voiced runs are clicks, bumps, and keyboard noise.
const MIN_SPEECH_FRAMES: usize = 4;
/// Soft word onsets start under the threshold.
const LEAD_FRAMES: usize = 3;
/// Word endings and plosive decay fall under the threshold before the word is over.
const HANGOVER_FRAMES: usize = 8;
/// Pauses shorter than this are inside a phrase.
const MERGE_GAP_FRAMES: usize = 10;

pub(crate) struct Vad {
    frame_len: usize,
    sum_sq: f64,
    in_frame: usize,
    levels: Vec<f32>,
}

impl Vad {
    pub fn new(rate_hz: u32) -> Vad {
        let frame_len = usize::try_from(u64::from(rate_hz) * FRAME_MS / 1000)
            .unwrap_or(usize::MAX)
            .max(1);
        Vad {
            frame_len,
            sum_sq: 0.0,
            in_frame: 0,
            levels: Vec::new(),
        }
    }

    pub fn feed(&mut self, samples: &[i16]) {
        for &sample in samples {
            let x = f64::from(sample) / f64::from(i16::MAX);
            self.sum_sq += x * x;
            self.in_frame += 1;
            if self.in_frame == self.frame_len {
                self.close_frame();
            }
        }
    }

    fn close_frame(&mut self) {
        let mean = self.sum_sq / self.in_frame as f64;
        let db = if mean > 0.0 {
            (10.0 * mean.log10()) as f32
        } else {
            f32::NEG_INFINITY
        };
        self.levels.push(db.max(FLOOR_MIN_DBFS - 10.0));
        self.sum_sq = 0.0;
        self.in_frame = 0;
    }

    /// Spans where someone talks, relative to the first sample fed, cut to `duration`.
    pub fn finish(mut self, duration: Ms) -> Vec<TimeRange> {
        if self.in_frame > 0 {
            self.close_frame();
        }
        let frame_ms = |frame: usize| Ms((frame as u64 * FRAME_MS).min(duration.0));
        spans(&voiced(&self.levels))
            .into_iter()
            .filter_map(|(start, end)| TimeRange::new(frame_ms(start), frame_ms(end)))
            .filter(|range| !range.is_empty())
            .collect()
    }
}

fn voiced(levels: &[f32]) -> Vec<bool> {
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let Some(&start) = sorted.get(sorted.len() * FLOOR_PERCENTILE / 100) else {
        return Vec::new();
    };
    let mut floor = start.clamp(FLOOR_MIN_DBFS, FLOOR_START_MAX_DBFS);
    levels
        .iter()
        .map(|&level| {
            let voiced = level > GATE_DBFS && level > floor + MARGIN_DB;
            let rate = if level < floor {
                FLOOR_FALL
            } else {
                FLOOR_RISE
            };
            floor = (floor + (level - floor) * rate).max(FLOOR_MIN_DBFS);
            voiced
        })
        .collect()
}

/// Frame ranges `[start, end)` of speech.
fn spans(voiced: &[bool]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    for (i, &v) in voiced.iter().chain([&false]).enumerate() {
        match (v, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= MIN_SPEECH_FRAMES {
                    runs.push((s.saturating_sub(LEAD_FRAMES), i + HANGOVER_FRAMES));
                }
                start = None;
            }
            _ => {}
        }
    }
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in runs {
        match merged.last_mut() {
            Some(last) if s <= last.1 + MERGE_GAP_FRAMES => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// A signal built from pieces, each `(ms, dbfs)`. A level of `None` is digital silence.
    struct Signal {
        samples: Vec<i16>,
        noise_dbfs: Option<f32>,
        seed: u32,
    }

    impl Signal {
        fn new() -> Signal {
            Signal {
                samples: Vec::new(),
                noise_dbfs: None,
                seed: 1,
            }
        }

        /// Adds steady white noise at `dbfs` under everything that follows.
        fn noise(mut self, dbfs: f32) -> Signal {
            self.noise_dbfs = Some(dbfs);
            self
        }

        fn rand(&mut self) -> f32 {
            self.seed = self
                .seed
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            (self.seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }

        fn piece(mut self, ms: u64, tone_dbfs: Option<f32>) -> Signal {
            let n = (u64::from(RATE) * ms / 1000) as usize;
            let noise_amp = self.noise_dbfs.map_or(0.0, |db| amp(db) * 3f32.sqrt());
            let tone_amp = tone_dbfs.map_or(0.0, |db| amp(db) * 2f32.sqrt());
            for i in 0..n {
                let t = (self.samples.len() + i) as f32 / RATE as f32;
                let x = tone_amp * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + noise_amp * self.rand();
                self.samples
                    .push((x.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
            }
            self
        }

        fn tone(self, ms: u64, dbfs: f32) -> Signal {
            self.piece(ms, Some(dbfs))
        }

        fn quiet(self, ms: u64) -> Signal {
            self.piece(ms, None)
        }

        fn activity(&self) -> Vec<(u64, u64)> {
            let mut vad = Vad::new(RATE);
            for chunk in self.samples.chunks(777) {
                vad.feed(chunk);
            }
            let duration = Ms(self.samples.len() as u64 * 1000 / u64::from(RATE));
            vad.finish(duration)
                .into_iter()
                .map(|r| (r.start().0, r.end().0))
                .collect()
        }
    }

    /// RMS amplitude for a level in dBFS.
    fn amp(dbfs: f32) -> f32 {
        10f32.powf(dbfs / 20.0)
    }

    const LEAD: u64 = LEAD_FRAMES as u64 * FRAME_MS;
    const HANG: u64 = HANGOVER_FRAMES as u64 * FRAME_MS;

    /// The span covers the tone, starts no earlier than the lead-in plus one frame, and ends no
    /// later than the hangover plus one frame.
    fn assert_covers(span: (u64, u64), tone: (u64, u64)) {
        assert!(
            span.0 <= tone.0 && span.0 + LEAD + FRAME_MS >= tone.0,
            "{span:?} starts badly for {tone:?}"
        );
        assert!(
            span.1 >= tone.1 && span.1 <= tone.1 + HANG + FRAME_MS,
            "{span:?} ends badly for {tone:?}"
        );
    }

    #[test]
    fn a_tone_between_silences_is_one_span_with_lead_in_and_hangover() {
        let spans = Signal::new()
            .quiet(1_000)
            .tone(2_000, -20.0)
            .quiet(2_000)
            .activity();
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert_covers(spans[0], (1_000, 3_000));
    }

    #[test]
    fn digital_silence_and_steady_noise_have_no_speech() {
        assert_eq!(Signal::new().quiet(5_000).activity(), vec![]);
        assert_eq!(Signal::new().noise(-35.0).quiet(5_000).activity(), vec![]);
    }

    #[test]
    fn the_floor_follows_the_noise_so_only_the_tone_counts() {
        let spans = Signal::new()
            .noise(-40.0)
            .quiet(2_000)
            .tone(1_500, -18.0)
            .quiet(3_000)
            .activity();
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert_covers(spans[0], (2_000, 3_500));
    }

    #[test]
    fn a_level_just_under_the_margin_above_the_noise_is_not_speech() {
        let under = Signal::new()
            .noise(-40.0)
            .quiet(2_000)
            .tone(1_500, -40.0 + MARGIN_DB - 7.0)
            .quiet(2_000)
            .activity();
        assert_eq!(under, vec![], "noise plus a tone 3 dB over it");
        let over = Signal::new()
            .noise(-40.0)
            .quiet(2_000)
            .tone(1_500, -40.0 + MARGIN_DB + 3.0)
            .quiet(2_000)
            .activity();
        assert_eq!(over.len(), 1, "{over:?}");
    }

    #[test]
    fn a_tone_under_the_absolute_gate_is_not_speech_even_in_digital_silence() {
        let quiet = Signal::new()
            .quiet(1_000)
            .tone(2_000, GATE_DBFS - 3.0)
            .quiet(1_000)
            .activity();
        assert_eq!(quiet, vec![]);
        let audible = Signal::new()
            .quiet(1_000)
            .tone(2_000, GATE_DBFS + 3.0)
            .quiet(1_000)
            .activity();
        assert_eq!(audible.len(), 1);
    }

    #[test]
    fn clicks_shorter_than_a_word_are_dropped() {
        let min = MIN_SPEECH_FRAMES as u64 * FRAME_MS;
        let click = Signal::new()
            .quiet(1_000)
            .tone(min - 2 * FRAME_MS, -10.0)
            .quiet(2_000)
            .activity();
        assert_eq!(click, vec![]);
        let word = Signal::new()
            .quiet(1_000)
            .tone(min + FRAME_MS, -10.0)
            .quiet(2_000)
            .activity();
        assert_eq!(word.len(), 1, "{word:?}");
    }

    #[test]
    fn short_pauses_merge_and_long_ones_split() {
        let merge_ms = MERGE_GAP_FRAMES as u64 * FRAME_MS;
        let short = Signal::new()
            .quiet(1_000)
            .tone(1_000, -20.0)
            .quiet(HANG + merge_ms - 2 * FRAME_MS)
            .tone(1_000, -20.0)
            .quiet(2_000)
            .activity();
        assert_eq!(short.len(), 1, "{short:?}");
        let long = Signal::new()
            .quiet(1_000)
            .tone(1_000, -20.0)
            .quiet(1_500)
            .tone(1_000, -20.0)
            .quiet(2_000)
            .activity();
        assert_eq!(long.len(), 2, "{long:?}");
        assert_covers(long[0], (1_000, 2_000));
        assert_covers(long[1], (3_500, 4_500));
    }

    #[test]
    fn speech_with_syllable_dips_stays_one_span_for_half_a_minute() {
        let mut signal = Signal::new().noise(-45.0).quiet(2_000);
        for _ in 0..100 {
            signal = signal.tone(200, -20.0).quiet(100);
        }
        let spans = signal.quiet(2_000).activity();
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert_covers(spans[0], (2_000, 31_900));
    }

    #[test]
    fn speech_at_the_very_start_and_end_is_kept_inside_the_clip() {
        let spans = Signal::new()
            .tone(1_000, -20.0)
            .quiet(3_000)
            .tone(500, -20.0)
            .activity();
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans[1].1, 4_500, "cut to the clip's end");
    }

    #[test]
    fn a_clip_with_no_quiet_part_at_all_is_speech() {
        let spans = Signal::new().tone(3_000, -15.0).activity();
        assert_eq!(spans, vec![(0, 3_000)]);
    }

    #[test]
    fn an_empty_clip_has_no_speech() {
        assert_eq!(Signal::new().activity(), vec![]);
    }
}
