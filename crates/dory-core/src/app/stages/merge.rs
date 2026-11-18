//! One transcript from per-channel segments: ordered by time, with mic bleed removed.
//!
//! Without headphones the mic also hears the remote side through the speakers, so the mic track
//! repeats what the system track says. The system track is the clean copy. A mic segment is bleed
//! when system speech covers most of its time and the mic text closely matches that speech.

use crate::domain::{Channel, Ms, Segment};

/// The share of a mic segment's time that system speech must cover. An echo is the same sound at
/// the same moment, delayed under 100 ms by the room, so its segment lies over the system one.
/// The two channels go through the engine separately and get their own segment boundaries, but an
/// echo still shares well over half its time. A reply comes after the remote side stops and shares
/// little or none.
pub(crate) const BLEED_OVERLAP: f32 = 0.5;

/// The share of a mic segment's words, in order, that must appear in the overlapping system
/// speech. Far-field speech through a room loses words, around a third at worst, so a real echo
/// keeps well over 60%. Someone answering over the remote side shares a few short words, rarely in
/// the same order, and stays well under it.
pub(crate) const BLEED_MATCH: f32 = 0.6;

/// Orders segments by start time, keeping the given order for ties, then drops mic bleed.
pub(crate) fn merge(mut segments: Vec<Segment>) -> Vec<Segment> {
    segments.sort_by_key(|s| s.range.map(|r| r.start()));
    let system: Vec<&Segment> = segments
        .iter()
        .filter(|s| s.channel == Channel::System && s.range.is_some())
        .collect();
    let bleed: Vec<bool> = segments
        .iter()
        .map(|s| s.channel == Channel::Mic && is_bleed(s, &system))
        .collect();
    segments
        .into_iter()
        .zip(bleed)
        .filter_map(|(segment, bleed)| (!bleed).then_some(segment))
        .collect()
}

fn is_bleed(mic: &Segment, system: &[&Segment]) -> bool {
    let Some(range) = mic.range else {
        return false;
    };
    let said = words(&mic.text);
    if said.is_empty() {
        return false;
    }
    let overlapping: Vec<(Ms, &Segment)> = system
        .iter()
        .filter_map(|s| Some((s.range?.overlap(&range), *s)))
        .filter(|(shared, _)| *shared > Ms(0))
        .collect();
    let shared: u64 = overlapping.iter().map(|(shared, _)| shared.0).sum();
    if (shared.min(range.len().0) as f32) < BLEED_OVERLAP * range.len().0 as f32 {
        return false;
    }
    let heard: Vec<String> = overlapping
        .iter()
        .flat_map(|(_, s)| words(&s.text))
        .collect();
    common_in_order(&said, &heard) as f32 >= BLEED_MATCH * said.len() as f32
}

/// Lowercase words with punctuation dropped. Scripts written without spaces count each character
/// as a word.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if is_unspaced(c) {
            out.extend((!word.is_empty()).then(|| std::mem::take(&mut word)));
            out.push(c.to_string());
        } else if c.is_alphanumeric() {
            word.push(c);
        } else if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    out.extend((!word.is_empty()).then_some(word));
    out
}

/// Han, kana, and the CJK compatibility block.
fn is_unspaced(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}

/// The longest common subsequence of two word lists.
fn common_in_order(a: &[String], b: &[String]) -> usize {
    let mut row = vec![0usize; b.len() + 1];
    for x in a {
        let mut diagonal = 0;
        for (j, y) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if x == y {
                diagonal + 1
            } else {
                above.max(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SpeakerId, TimeRange};

    fn seg(channel: Channel, a: u64, b: u64, text: &str) -> Segment {
        Segment {
            range: TimeRange::new(Ms(a), Ms(b)),
            speaker: SpeakerId(u16::from(channel != Channel::Mic)),
            channel,
            text: text.into(),
            language: None,
        }
    }

    fn mic(a: u64, b: u64, text: &str) -> Segment {
        seg(Channel::Mic, a, b, text)
    }

    fn sys(a: u64, b: u64, text: &str) -> Segment {
        seg(Channel::System, a, b, text)
    }

    fn texts(segments: &[Segment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn channels_merge_by_start_time_and_ties_keep_their_order() {
        let merged = merge(vec![
            mic(0, 1_000, "one"),
            mic(5_000, 6_000, "four"),
            mic(9_000, 9_500, "six"),
            sys(2_000, 3_000, "two"),
            sys(4_000, 4_500, "three"),
            sys(5_000, 7_000, "five"),
        ]);
        assert_eq!(
            texts(&merged),
            ["one", "two", "three", "four", "five", "six"]
        );
    }

    #[test]
    fn a_mic_segment_that_repeats_the_remote_side_is_dropped() {
        let merged = merge(vec![
            sys(10_000, 14_000, "We should ship the beta on Friday, right?"),
            mic(10_300, 14_200, "we should ship the beta on friday right"),
            mic(15_000, 16_000, "Sounds good to me."),
        ]);
        assert_eq!(
            texts(&merged),
            [
                "We should ship the beta on Friday, right?",
                "Sounds good to me."
            ]
        );
    }

    #[test]
    fn bleed_needs_most_of_the_mic_segment_to_overlap_system_speech() {
        let words = "the quarterly numbers look strong this time";
        let shared = |ms: u64| {
            let start = 5_000 - ms;
            merge(vec![sys(0, 5_000, words), mic(start, start + 4_000, words)]).len()
        };
        let at = (BLEED_OVERLAP * 4_000.0) as u64;
        assert_eq!(
            shared(at + 100),
            1,
            "sharing just over the threshold is bleed"
        );
        assert_eq!(shared(at - 100), 2, "sharing just under it is the user");
    }

    #[test]
    fn a_reply_right_after_similar_remote_words_is_the_user() {
        let merged = merge(vec![
            mic(0, 900, "Hello from the fake microphone."),
            sys(1_000, 1_900, "Hello from the fake system audio."),
        ]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn bleed_needs_most_of_the_mic_words_in_order() {
        let said = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let heard = |kept: usize| {
            let mut words: Vec<&str> = said.split(' ').take(kept).collect();
            words.extend(
                ["one", "two", "three", "four", "five", "six"]
                    .iter()
                    .take(10 - kept),
            );
            merge(vec![sys(0, 5_000, said), mic(0, 5_000, &words.join(" "))]).len()
        };
        let at = (BLEED_MATCH * 10.0).round() as usize;
        assert_eq!(heard(at), 1, "{at} of 10 words match");
        assert_eq!(heard(at - 1), 2, "{} of 10 words match", at - 1);
    }

    #[test]
    fn overlapping_system_segments_are_read_as_one_stretch_of_speech() {
        let merged = merge(vec![
            sys(0, 2_000, "let's look at"),
            sys(2_000, 4_000, "the hiring plan next"),
            mic(500, 4_000, "Let's look at the hiring plan next."),
        ]);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|s| s.channel == Channel::System));
    }

    #[test]
    fn scripts_without_spaces_compare_by_character() {
        let merged = merge(vec![
            sys(0, 3_000, "我们周五发布吧。"),
            mic(0, 3_000, "我们周五发布"),
        ]);
        assert_eq!(texts(&merged), ["我们周五发布吧。"]);
    }

    #[test]
    fn only_mic_segments_with_a_time_and_words_can_be_bleed() {
        let untimed = Segment {
            range: None,
            ..mic(0, 1, "same words here")
        };
        let merged = merge(vec![
            sys(0, 3_000, "same words here"),
            untimed.clone(),
            mic(0, 3_000, "..."),
            seg(Channel::Mixed, 0, 3_000, "same words here"),
        ]);
        assert_eq!(merged.len(), 4, "{merged:?}");
        assert!(merged.contains(&untimed));
    }
}
