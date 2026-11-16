//! How long processing still takes (UX-11, UX-12) as plain data and pure functions: the rate this
//! Mac transcribes at for each pace, the raw time left, and the smoothing that keeps the number
//! the card shows calm. Every function takes the clock as an input, so the rules are tested
//! without waiting.

use dory_core::domain::{Fraction, Ms};
use dory_core::ports::Pace;

/// Time left appears once this much audio is transcribed, so the rate has settled.
pub const ESTIMATE_AFTER: Ms = Ms(60_000);
/// The shown estimate changes at most this often.
pub const UPDATE_EVERY_MS: u64 = 10_000;
/// A new estimate this much higher than the shown one may replace it, once per job.
pub const RAISE_OVER: f64 = 1.3;

/// Audio milliseconds processed per wall-clock millisecond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate(f64);

impl Rate {
    /// A guess for a Mac Dory has not watched transcribe yet: a quarter of real time.
    pub const GUESS: Rate = Rate(4.0);

    /// The rate over `elapsed_ms` in which `audio` went from `from` to `to` done. `None` until
    /// some audio and some time have passed.
    pub fn observed(audio: Ms, from: Fraction, to: Fraction, elapsed_ms: u64) -> Option<Rate> {
        let done = f64::from(to.get() - from.get()) * audio.0 as f64;
        (done > 0.0 && elapsed_ms > 0).then(|| Rate(done / elapsed_ms as f64))
    }

    pub fn time_for(self, audio: Ms) -> Ms {
        Ms((audio.0 as f64 / self.0).round() as u64)
    }
}

/// The rate seen at each pace, so a job that turns light is estimated at the light rate (UX-30).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rates {
    fast: Option<Rate>,
    light: Option<Rate>,
}

impl Rates {
    pub fn observe(&mut self, pace: Pace, rate: Rate) {
        match pace {
            Pace::Fast => self.fast = Some(rate),
            Pace::Light => self.light = Some(rate),
        }
    }

    pub fn at(&self, pace: Pace) -> Option<Rate> {
        match pace {
            Pace::Fast => self.fast,
            Pace::Light => self.light,
        }
    }

    /// The most recent rate at any pace, for estimates made before a job starts.
    pub fn any(&self) -> Option<Rate> {
        self.fast.or(self.light)
    }
}

/// The time the rest of `audio` takes at `rate`, once [`ESTIMATE_AFTER`] of it is done.
pub fn transcribe_left(audio: Ms, done: Fraction, rate: Option<Rate>) -> Option<Ms> {
    let done_ms = f64::from(done.get()) * audio.0 as f64;
    if done_ms < ESTIMATE_AFTER.0 as f64 {
        return None;
    }
    Some(rate?.time_for(Ms((audio.0 as f64 - done_ms).max(0.0) as u64)))
}

/// The summary's time left, extrapolated from how far it got in `elapsed_ms` since `from`.
pub fn summary_left(from: Fraction, to: Fraction, elapsed_ms: u64) -> Option<Ms> {
    let moved = f64::from(to.get() - from.get());
    (moved > 0.0 && elapsed_ms > 0)
        .then(|| Ms((elapsed_ms as f64 * f64::from(1.0 - to.get()) / moved).round() as u64))
}

/// What the card says about the time left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// Too early to say.
    Estimating,
    About(Ms),
}

impl Left {
    /// "about 4 min left", rounded up so it never promises too much, or "less than 1 min left".
    pub fn short(self) -> String {
        match self {
            Left::Estimating => "estimating time left".into(),
            Left::About(ms) if ms.0 < 60_000 => "less than 1 min left".into(),
            Left::About(ms) => format!("about {} min left", ms.0.div_ceil(60_000)),
        }
    }

    /// The same for VoiceOver, with the unit spelled out.
    pub fn spoken(self) -> String {
        match self {
            Left::Estimating => "estimating time left".into(),
            Left::About(ms) if ms.0 < 60_000 => "less than 1 minute left".into(),
            Left::About(ms) => match ms.0.div_ceil(60_000) {
                1 => "about 1 minute left".into(),
                n => format!("about {n} minutes left"),
            },
        }
    }

    /// "in about 9 min", for a queued job's start.
    pub fn starts(ms: Ms) -> String {
        if ms.0 < 60_000 {
            "in less than 1 min".into()
        } else {
            format!("in about {} min", ms.0.div_ceil(60_000))
        }
    }
}

/// UX-12's smoothing of one job's time left. It shows the first estimate it gets, then changes
/// at most every [`UPDATE_EVERY_MS`], only downward, except once when a new estimate is more
/// than [`RAISE_OVER`] times the shown one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    /// The shown estimate and the monotonic millisecond it was taken.
    shown: Option<(Ms, u64)>,
    raised: bool,
}

impl Estimate {
    /// Takes the raw estimate at `now_ms`, `None` while none can be made, and returns what to show.
    pub fn update(&mut self, raw: Option<Ms>, now_ms: u64) -> Left {
        let Some(raw) = raw else {
            return self.left();
        };
        match self.shown {
            None => self.shown = Some((raw, now_ms)),
            Some((shown, at)) if now_ms.saturating_sub(at) >= UPDATE_EVERY_MS => {
                if raw <= shown {
                    self.shown = Some((raw, now_ms));
                } else if !self.raised && raw.0 as f64 > shown.0 as f64 * RAISE_OVER {
                    self.raised = true;
                    self.shown = Some((raw, now_ms));
                }
            }
            Some(_) => {}
        }
        self.left()
    }

    pub fn left(&self) -> Left {
        self.shown
            .map_or(Left::Estimating, |(ms, _)| Left::About(ms))
    }
}

/// The finish card's sentence with the estimated time (UX-11).
pub fn finish_estimate(audio: Ms, rate: Option<Rate>) -> String {
    let minutes = rate
        .unwrap_or(Rate::GUESS)
        .time_for(audio)
        .0
        .div_ceil(60_000)
        .max(1);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!("Transcribing takes about {minutes} {unit} on this Mac.")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    #[test]
    fn nothing_is_estimated_before_the_first_minute_of_audio_is_transcribed() {
        let audio = Ms(10 * MIN);
        let rate = Some(Rate(4.0));
        assert_eq!(transcribe_left(audio, Fraction::new(0.09), rate), None);
        assert_eq!(
            transcribe_left(audio, Fraction::new(0.1), rate),
            Some(Ms(9 * MIN / 4)),
            "a minute of 10 is done, 9 left at 4x"
        );
        assert_eq!(
            transcribe_left(audio, Fraction::new(0.5), None),
            None,
            "no rate yet"
        );
        let mut estimate = Estimate::default();
        assert_eq!(
            estimate.update(transcribe_left(audio, Fraction::new(0.03), rate), 0),
            Left::Estimating
        );
        assert_eq!(Left::Estimating.short(), "estimating time left");
    }

    #[test]
    fn the_estimate_rounds_up_to_whole_minutes_and_says_less_than_1_min_below_a_minute() {
        assert_eq!(Left::About(Ms(4 * MIN)).short(), "about 4 min left");
        assert_eq!(Left::About(Ms(4 * MIN + 1)).short(), "about 5 min left");
        assert_eq!(Left::About(Ms(MIN)).short(), "about 1 min left");
        assert_eq!(Left::About(Ms(MIN - 1)).short(), "less than 1 min left");
        assert_eq!(Left::About(Ms(0)).short(), "less than 1 min left");
        assert_eq!(Left::About(Ms(4 * MIN)).spoken(), "about 4 minutes left");
        assert_eq!(Left::About(Ms(MIN)).spoken(), "about 1 minute left");
        assert_eq!(Left::About(Ms(30_000)).spoken(), "less than 1 minute left");
    }

    #[test]
    fn the_estimate_changes_at_most_every_10_seconds() {
        let mut estimate = Estimate::default();
        assert_eq!(
            estimate.update(Some(Ms(5 * MIN)), 1_000),
            Left::About(Ms(5 * MIN))
        );
        assert_eq!(
            estimate.update(Some(Ms(4 * MIN)), 10_999),
            Left::About(Ms(5 * MIN)),
            "9.999 s later"
        );
        assert_eq!(
            estimate.update(Some(Ms(4 * MIN)), 11_000),
            Left::About(Ms(4 * MIN)),
            "10 s later"
        );
        assert_eq!(
            estimate.update(None, 30_000),
            Left::About(Ms(4 * MIN)),
            "no new estimate keeps the shown one"
        );
    }

    #[test]
    fn the_estimate_only_goes_down_unless_it_jumps_by_more_than_30_percent_and_then_only_once() {
        let mut estimate = Estimate::default();
        estimate.update(Some(Ms(4 * MIN)), 0);
        assert_eq!(
            estimate.update(Some(Ms(5 * MIN)), 10_000),
            Left::About(Ms(4 * MIN)),
            "25% higher is held"
        );
        assert_eq!(
            estimate.update(Some(Ms(6 * MIN)), 20_000),
            Left::About(Ms(6 * MIN)),
            "50% higher may go up"
        );
        assert_eq!(
            estimate.update(Some(Ms(10 * MIN)), 30_000),
            Left::About(Ms(6 * MIN)),
            "but only once"
        );
        assert_eq!(
            estimate.update(Some(Ms(3 * MIN)), 40_000),
            Left::About(Ms(3 * MIN)),
            "down is always allowed"
        );
    }

    #[test]
    fn the_estimate_uses_the_rate_of_the_current_pace() {
        let audio = Ms(10 * MIN);
        let fast = Rate::observed(audio, Fraction::new(0.0), Fraction::new(0.2), 30_000).unwrap();
        let light = Rate::observed(audio, Fraction::new(0.2), Fraction::new(0.3), 60_000).unwrap();
        let mut rates = Rates::default();
        rates.observe(Pace::Fast, fast);
        rates.observe(Pace::Light, light);
        let at = |pace| transcribe_left(audio, Fraction::new(0.5), rates.at(pace));
        assert_eq!(at(Pace::Fast), Some(Ms(75_000)), "5 min of audio at 4x");
        assert_eq!(at(Pace::Light), Some(Ms(300_000)), "5 min of audio at 1x");
        assert_eq!(rates.any(), Some(fast));
        assert_eq!(Rates::default().at(Pace::Light), None);
    }

    #[test]
    fn the_summary_is_estimated_from_its_own_progress() {
        assert_eq!(
            summary_left(Fraction::new(0.0), Fraction::new(0.25), 20_000),
            Some(Ms(60_000))
        );
        assert_eq!(
            summary_left(Fraction::new(0.5), Fraction::new(0.5), 20_000),
            None
        );
    }

    #[test]
    fn a_queued_start_reads_in_whole_minutes() {
        assert_eq!(Left::starts(Ms(8 * MIN + 1)), "in about 9 min");
        assert_eq!(Left::starts(Ms(20_000)), "in less than 1 min");
    }

    #[test]
    fn the_finish_estimate_uses_the_observed_rate_or_a_guess() {
        let audio = Ms(12 * MIN);
        assert_eq!(
            finish_estimate(audio, None),
            "Transcribing takes about 3 minutes on this Mac."
        );
        let slow = Rate::observed(audio, Fraction::new(0.0), Fraction::new(0.5), 6 * MIN);
        assert_eq!(
            finish_estimate(audio, slow),
            "Transcribing takes about 12 minutes on this Mac."
        );
        assert_eq!(
            finish_estimate(Ms(10_000), None),
            "Transcribing takes about 1 minute on this Mac."
        );
    }
}
