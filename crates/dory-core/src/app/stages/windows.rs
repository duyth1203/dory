//! Which stretches of a clip go to the speech engine, one call each.

use crate::domain::{LangTag, LanguagePref, Ms, Recovery, TimeRange};
use crate::models::SttCaps;
use crate::ports::Activity;

/// The least speech an uncovered stretch must hold before it is decoded again. Shorter gaps are
/// the edges of sentences, and a decode of a silent tail writes words that were never said.
pub const MIN_UNCOVERED: Ms = Ms(1_500);

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

/// How much of `window` is speech. An unmeasured clip counts as speech throughout.
pub(crate) fn speech_time(activity: &Activity, window: TimeRange) -> Ms {
    match activity {
        Activity::Unmeasured => window.len(),
        Activity::Speech(spans) => Ms(spans.iter().map(|s| s.overlap(&window).0).sum()),
    }
}

/// Whether a transcription run decodes again the speech its windows leave without text.
///
/// `Auto` recovers only for a recording on Automatic in a meeting held in two or more languages,
/// because a model keeps one language per window and a pinned or one-language meeting has no
/// switch to drop. Neither switch recovers without sentence times, which show what was dropped.
pub fn recovers(
    caps: &SttCaps,
    recording: &LanguagePref,
    meeting: &[LangTag],
    switch: Recovery,
) -> bool {
    caps.sentence_times
        && match switch {
            Recovery::Off => false,
            Recovery::On => true,
            Recovery::Auto => *recording == LanguagePref::Auto && meeting.len() >= 2,
        }
}

/// The stretches of `speech` that no range in `said` overlaps, earliest first.
///
/// Speech left between two sentences that only a pause separates joins one stretch, which runs
/// from its first uncovered speech to its last. A stretch is kept when it holds at least `min` of
/// speech, so a sentence's ragged edge or a silent tail is never decoded again.
pub fn uncovered(speech: &[TimeRange], said: &[TimeRange], min: Ms) -> Vec<TimeRange> {
    let mut said: Vec<TimeRange> = said.iter().copied().filter(|s| !s.is_empty()).collect();
    said.sort_by_key(|s| s.start());
    let mut pieces: Vec<TimeRange> = Vec::new();
    for span in speech {
        let mut at = span.start();
        for s in said.iter().filter(|s| s.overlap(span) > Ms(0)) {
            pieces.extend(TimeRange::new(at, s.start()).filter(|p| !p.is_empty()));
            at = at.max(s.end());
        }
        pieces.extend(TimeRange::new(at, span.end()).filter(|p| !p.is_empty()));
    }
    pieces.sort_by_key(|p| p.start());

    let mut stretches = Vec::new();
    let mut open: Option<(TimeRange, Ms)> = None;
    for piece in pieces {
        open = match open {
            Some((stretch, held))
                if !said
                    .iter()
                    .any(|s| s.start() < piece.start() && s.end() > stretch.end()) =>
            {
                let grown = TimeRange::new(stretch.start(), piece.end().max(stretch.end()))
                    .expect("pieces run forward");
                Some((grown, Ms(held.0 + piece.len().0)))
            }
            closed => {
                stretches.extend(closed.filter(|(_, held)| *held >= min).map(|(s, _)| s));
                Some((piece, piece.len()))
            }
        };
    }
    stretches.extend(open.filter(|(_, held)| *held >= min).map(|(s, _)| s));
    stretches
}

/// How many rounds of stretches a window decodes again after its first pass. A stretch decoded
/// again can itself drop the turn after a switch, so its own leftover goes in the next round.
pub const RECOVERY_DEPTH: usize = 3;

/// Decodes again the stretches of `speech` that `said` leaves without text, then the stretches
/// each of those calls leaves, for at most [`RECOVERY_DEPTH`] rounds. Returns how many stretches
/// went to `decode`.
///
/// `decode` takes a stretch and returns the ranges its sentences cover, on the axis of `speech`.
/// A stretch whose call covers none of its speech is not decoded again, because the same audio
/// decodes the same way.
pub fn recover<E>(
    speech: &[TimeRange],
    said: &[TimeRange],
    mut decode: impl FnMut(TimeRange) -> Result<Vec<TimeRange>, E>,
) -> Result<usize, E> {
    let mut round = uncovered(speech, said, MIN_UNCOVERED);
    let mut decoded = 0;
    for _ in 0..RECOVERY_DEPTH {
        let mut next = Vec::new();
        for stretch in round {
            let said = decode(stretch)?;
            decoded += 1;
            next.extend(
                uncovered(&cut(speech, stretch), &said, MIN_UNCOVERED)
                    .into_iter()
                    .filter(|left| *left != stretch),
            );
        }
        round = next;
    }
    Ok(decoded)
}

/// The speech inside `window`, on the same axis. An unmeasured clip is speech throughout.
pub(crate) fn speech_spans(activity: &Activity, window: TimeRange) -> Vec<TimeRange> {
    match activity {
        Activity::Unmeasured => vec![window],
        Activity::Speech(spans) => cut(spans, window),
    }
}

/// The parts of `spans` inside `window`.
fn cut(spans: &[TimeRange], window: TimeRange) -> Vec<TimeRange> {
    spans
        .iter()
        .filter_map(|s| TimeRange::new(s.start().max(window.start()), s.end().min(window.end())))
        .filter(|s| !s.is_empty())
        .collect()
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

    #[test]
    fn speech_time_counts_only_the_speech_inside_the_window() {
        let activity = speech(&[(1_000, 5_000), (8_000, 12_000), (40_000, 50_000)]);
        assert_eq!(speech_time(&activity, r(0, 10_000)), Ms(6_000));
        assert_eq!(speech_time(&activity, r(20_000, 30_000)), Ms(0));
        assert_eq!(speech_time(&Activity::Unmeasured, r(0, 10_000)), Ms(10_000));
    }

    fn caps(sentence_times: bool) -> SttCaps {
        SttCaps {
            languages: crate::models::LangSupport::Multilingual {
                auto_detect: true,
                tags: crate::domain::LangSet::Any,
            },
            diarization: crate::models::DiarizationSupport::None,
            max_input: None,
            window: CAP,
            sentence_times,
            lead_in: Ms(0),
            vocabulary: false,
        }
    }

    #[test]
    fn recovers_needs_sentence_times_and_follows_the_switch() {
        let tag = |t: &str| LangTag::parse(t).unwrap();
        let meetings: [&[LangTag]; 3] = [&[], &[tag("en")], &[tag("tl"), tag("en")]];
        let recordings = [LanguagePref::Auto, LanguagePref::Pinned(tag("tl"))];
        for switch in [Recovery::Off, Recovery::On, Recovery::Auto] {
            for recording in &recordings {
                for meeting in meetings {
                    for capable in [false, true] {
                        let want = capable
                            && match switch {
                                Recovery::Off => false,
                                Recovery::On => true,
                                Recovery::Auto => {
                                    *recording == LanguagePref::Auto && meeting.len() == 2
                                }
                            };
                        assert_eq!(
                            recovers(&caps(capable), recording, meeting, switch),
                            want,
                            "{switch:?} {recording:?} {} meeting languages, sentence times {capable}",
                            meeting.len()
                        );
                    }
                }
            }
        }
    }

    const MIN: Ms = MIN_UNCOVERED;

    #[test]
    fn speech_a_sentence_covers_is_not_uncovered() {
        assert_eq!(
            uncovered(&[r(0, 20_000)], &[r(0, 9_000), r(9_200, 20_000)], MIN),
            vec![]
        );
    }

    #[test]
    fn a_dropped_tail_is_uncovered_from_its_first_speech() {
        assert_eq!(
            uncovered(&[r(0, 9_000), r(9_400, 20_000)], &[r(0, 8_800)], MIN),
            vec![r(8_800, 20_000)],
            "the speech the sentence left at its end joins the turn after the pause"
        );
    }

    #[test]
    fn a_dropped_head_is_uncovered() {
        assert_eq!(
            uncovered(&[r(1_000, 20_000)], &[r(12_000, 20_000)], MIN),
            vec![r(1_000, 12_000)]
        );
    }

    #[test]
    fn speech_across_pauses_with_no_sentence_between_is_one_stretch() {
        assert_eq!(
            uncovered(
                &[r(0, 5_000), r(6_000, 7_000), r(9_000, 10_000)],
                &[r(0, 5_000)],
                MIN
            ),
            vec![r(6_000, 10_000)]
        );
    }

    #[test]
    fn a_gap_with_less_speech_than_min_is_left_alone() {
        assert_eq!(
            uncovered(&[r(0, 20_000)], &[r(0, 10_000), r(11_400, 20_000)], MIN),
            vec![]
        );
        assert_eq!(
            uncovered(&[r(0, 20_000)], &[r(0, 10_000), r(11_500, 20_000)], MIN),
            vec![r(10_000, 11_500)]
        );
    }

    #[test]
    fn silence_between_sentences_is_never_uncovered() {
        assert_eq!(
            uncovered(
                &[r(0, 5_000), r(15_000, 20_000)],
                &[r(0, 5_000), r(15_000, 20_000)],
                MIN
            ),
            vec![],
            "ten seconds between the sentences that the VAD heard as silence"
        );
    }

    #[test]
    fn a_sentence_between_two_gaps_keeps_them_apart() {
        assert_eq!(
            uncovered(&[r(0, 30_000)], &[r(2_000, 10_000), r(12_000, 20_000)], MIN),
            vec![r(0, 2_000), r(10_000, 12_000), r(20_000, 30_000)]
        );
    }

    /// Turns two seconds apart, each its own run of speech.
    fn turns(n: u64) -> Vec<TimeRange> {
        (0..n).map(|i| r(i * 6_000, i * 6_000 + 4_000)).collect()
    }

    /// Recovers a window of `turns` with an engine that writes only the first turn starting in
    /// each call, the way Whisper keeps the first language and drops the turns after a switch.
    /// Returns the stretches decoded and the turns said.
    fn recover_dropping_tails(turns: &[TimeRange]) -> (Vec<TimeRange>, Vec<TimeRange>) {
        let first_turn = |span: TimeRange| {
            turns
                .iter()
                .copied()
                .find(|t| t.start() >= span.start() && t.start() < span.end())
        };
        let mut said: Vec<TimeRange> = first_turn(r(0, 60_000)).into_iter().collect();
        let mut calls = Vec::new();
        let first = said.clone();
        let decoded = recover(turns, &first, |stretch| {
            calls.push(stretch);
            let heard: Vec<TimeRange> = first_turn(stretch).into_iter().collect();
            said.extend(&heard);
            Ok::<_, ()>(heard)
        })
        .unwrap();
        assert_eq!(decoded, calls.len());
        (calls, said)
    }

    #[test]
    fn a_stretch_that_drops_its_own_tail_is_decoded_again_within_the_depth() {
        let turns = turns(4);
        let (calls, said) = recover_dropping_tails(&turns);
        assert_eq!(said, turns, "every turn is said");
        assert_eq!(
            calls,
            vec![r(6_000, 22_000), r(12_000, 22_000), r(18_000, 22_000)]
        );
    }

    #[test]
    fn recovery_stops_after_the_depth() {
        let turns = turns(6);
        let (calls, said) = recover_dropping_tails(&turns);
        assert_eq!(calls.len(), RECOVERY_DEPTH);
        assert_eq!(
            said,
            turns[..=RECOVERY_DEPTH],
            "the turns after the last round stay unsaid"
        );
    }

    #[test]
    fn a_stretch_that_comes_back_empty_is_not_decoded_again() {
        let mut calls = Vec::new();
        let decoded = recover(
            &[r(0, 4_000), r(6_000, 10_000)],
            &[r(0, 4_000)],
            |stretch| {
                calls.push(stretch);
                Ok::<_, ()>(Vec::new())
            },
        )
        .unwrap();
        assert_eq!(decoded, 1);
        assert_eq!(calls, vec![r(6_000, 10_000)]);
    }

    #[test]
    fn a_failed_decode_stops_recovery() {
        let mut calls = 0;
        let result = recover(&turns(4), &[r(0, 4_000)], |_| {
            calls += 1;
            Err("engine failed")
        });
        assert_eq!(result, Err("engine failed"));
        assert_eq!(calls, 1);
    }

    #[test]
    fn speech_spans_are_cut_to_the_window() {
        let activity = speech(&[(1_000, 5_000), (8_000, 12_000), (40_000, 50_000)]);
        assert_eq!(
            speech_spans(&activity, r(2_000, 10_000)),
            vec![r(2_000, 5_000), r(8_000, 10_000)]
        );
        assert_eq!(
            speech_spans(&Activity::Unmeasured, r(2_000, 10_000)),
            vec![r(2_000, 10_000)]
        );
    }
}
