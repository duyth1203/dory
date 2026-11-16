//! Which stretches of a clip go to the speech engine, one call each.

use crate::domain::{Ms, TimeRange};
use crate::ports::Activity;

/// Windows over a clip of `duration`, on the clip's time axis, earliest first.
///
/// With no `cap` the whole clip is one window, because an engine's own speaker indices only hold
/// within one call. With a cap, windows hold whole runs of speech and end at a silence, each at
/// most `cap` long, and silence between windows is never sent. Speech longer than the cap with no
/// silence in it is cut into equal pieces. A clip with no speech has no windows.
pub(crate) fn plan_windows(duration: Ms, activity: &Activity, cap: Option<Ms>) -> Vec<TimeRange> {
    let Some(whole) = TimeRange::new(Ms(0), duration) else {
        return Vec::new();
    };
    let speech: Vec<TimeRange> = match activity {
        Activity::Unmeasured => vec![whole],
        Activity::Speech(spans) => spans
            .iter()
            .filter_map(|s| TimeRange::new(s.start().min(duration), s.end().min(duration)))
            .filter(|s| !s.is_empty())
            .collect(),
    };
    if whole.is_empty() || speech.is_empty() {
        return Vec::new();
    }
    let Some(cap) = cap.filter(|c| c.0 > 0) else {
        return vec![whole];
    };
    let mut windows = Vec::new();
    let mut open: Option<TimeRange> = None;
    for span in speech {
        if let Some(window) = open.take() {
            match TimeRange::new(window.start(), span.end()).filter(|w| w.len() <= cap) {
                Some(grown) => {
                    open = Some(grown);
                    continue;
                }
                None => windows.push(window),
            }
        }
        if span.len() <= cap {
            open = Some(span);
        } else {
            windows.extend(equal_pieces(span, cap));
        }
    }
    windows.extend(open);
    windows
}

fn equal_pieces(span: TimeRange, cap: Ms) -> impl Iterator<Item = TimeRange> {
    let (start, len) = (span.start().0, span.len().0);
    let pieces = len.div_ceil(cap.0);
    (0..pieces).filter_map(move |i| {
        TimeRange::new(
            Ms(start + len * i / pieces),
            Ms(start + len * (i + 1) / pieces),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: u64, b: u64) -> TimeRange {
        TimeRange::new(Ms(a), Ms(b)).unwrap()
    }

    fn speech(spans: &[(u64, u64)]) -> Activity {
        Activity::Speech(spans.iter().map(|&(a, b)| r(a, b)).collect())
    }

    const CAP: Option<Ms> = Some(Ms(30_000));

    #[test]
    fn an_unmeasured_clip_with_no_cap_is_one_window() {
        assert_eq!(
            plan_windows(Ms(90_000), &Activity::Unmeasured, None),
            vec![r(0, 90_000)]
        );
    }

    #[test]
    fn a_measured_silent_clip_has_no_windows() {
        for cap in [None, CAP] {
            assert_eq!(plan_windows(Ms(90_000), &speech(&[]), cap), vec![]);
        }
        assert_eq!(plan_windows(Ms(0), &Activity::Unmeasured, CAP), vec![]);
    }

    #[test]
    fn with_no_cap_any_speech_sends_the_whole_clip() {
        assert_eq!(
            plan_windows(Ms(90_000), &speech(&[(40_000, 41_000)]), None),
            vec![r(0, 90_000)]
        );
    }

    #[test]
    fn silence_between_windows_is_skipped() {
        assert_eq!(
            plan_windows(
                Ms(120_000),
                &speech(&[(1_000, 5_000), (8_000, 12_000), (40_000, 50_000)]),
                CAP
            ),
            vec![r(1_000, 12_000), r(40_000, 50_000)]
        );
    }

    #[test]
    fn a_window_ends_at_the_last_silence_that_keeps_it_under_the_cap() {
        assert_eq!(
            plan_windows(
                Ms(60_000),
                &speech(&[(0, 10_000), (12_000, 25_000), (28_000, 35_000)]),
                CAP
            ),
            vec![r(0, 25_000), r(28_000, 35_000)]
        );
    }

    #[test]
    fn a_window_exactly_the_cap_long_is_kept_whole() {
        assert_eq!(
            plan_windows(Ms(60_000), &speech(&[(0, 10_000), (20_000, 30_000)]), CAP),
            vec![r(0, 30_000)]
        );
        assert_eq!(
            plan_windows(Ms(60_000), &speech(&[(0, 10_000), (20_000, 30_001)]), CAP),
            vec![r(0, 10_000), r(20_000, 30_001)]
        );
    }

    #[test]
    fn speech_longer_than_the_cap_is_cut_into_equal_pieces() {
        assert_eq!(
            plan_windows(Ms(100_000), &speech(&[(0, 70_000), (80_000, 90_000)]), CAP),
            vec![
                r(0, 23_333),
                r(23_333, 46_666),
                r(46_666, 70_000),
                r(80_000, 90_000)
            ]
        );
        assert_eq!(
            plan_windows(Ms(65_000), &Activity::Unmeasured, CAP),
            vec![r(0, 21_666), r(21_666, 43_333), r(43_333, 65_000)],
            "no activity means the whole clip is speech"
        );
    }

    #[test]
    fn speech_past_the_end_is_cut_to_the_clip() {
        assert_eq!(
            plan_windows(
                Ms(10_000),
                &speech(&[(2_000, 15_000), (12_000, 13_000)]),
                CAP
            ),
            vec![r(2_000, 10_000)]
        );
    }

    #[test]
    fn a_zero_cap_means_no_cap_instead_of_endless_windows() {
        assert_eq!(
            plan_windows(Ms(10_000), &speech(&[(2_000, 3_000)]), Some(Ms(0))),
            vec![r(0, 10_000)]
        );
    }
}
