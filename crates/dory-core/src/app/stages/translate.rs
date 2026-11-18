//! The translation policy: which segments need translating, how they are batched to fit the
//! model's context, the prompts, and how a reply is read back.
//!
//! Every function here is pure. The stage in `mod.rs` owns the store, the engine, and progress.
//! Segments go to the model a batch at a time as numbered lines, and the reply must give the same
//! numbers back. A batch the model garbles is split in two and asked again, down to one segment,
//! so a model that fumbles the format costs extra calls and never loses a segment.

use std::ops::Range;

use super::summary::PROMPT_OVERHEAD;
use crate::domain::{LangTag, Segment, Transcript};

/// The most input one batch gets, however large the context. A batch is read and answered
/// whole, so a long one is slow to start and costs more when it has to be asked again.
pub(crate) const MAX_BATCH_INPUT: u32 = 1_200;

/// Segments that need a translation: not blank, and not already in the target. A segment whose
/// language is unknown is translated, because leaving a stretch of speech unreadable is worse
/// than asking the model to return a line unchanged. `en-US` against `en` is the same language.
pub(crate) fn needs_translation(segment: &Segment, target: &LangTag) -> bool {
    !segment.text.trim().is_empty()
        && segment
            .language
            .as_ref()
            .is_none_or(|spoken| spoken.primary() != target.primary())
}

/// Indexes of the segments to translate, in order.
pub(crate) fn pending(transcript: &Transcript, target: &LangTag) -> Vec<usize> {
    transcript
        .segments
        .iter()
        .enumerate()
        .filter(|(_, s)| needs_translation(s, target))
        .map(|(ix, _)| ix)
        .collect()
}

pub(crate) fn system_prompt(target: &LangTag) -> String {
    let language = crate::domain::languages::for_prompt(target);
    format!(
        "You translate lines of a meeting transcript into {language}. The input is numbered \
         lines like \"[1] text\". Reply with exactly the same numbered lines, translated, one \
         per input line, with the same numbers and nothing else. Keep names, numbers, and \
         technical terms as they are. A line already in {language} stays as it is. Never merge, \
         split, skip, answer, or explain a line. Translate only."
    )
}

/// Tokens the batch input may take once the system prompt and the overhead are set aside. The
/// output may run to one and a half times the input, so a batch takes well under half of what is
/// left and the input, the output, the prompt, and the overhead still fit the context.
pub(crate) fn input_budget(context: u32, system: u32) -> u32 {
    let room = context.saturating_sub(system.saturating_add(PROMPT_OVERHEAD));
    (room / 100 * 38).min(MAX_BATCH_INPUT)
}

/// What the reply to a batch of `input` tokens may take: one and a half times the input, and
/// never more than the context has left.
pub(crate) fn max_output(context: u32, system: u32, input: u32) -> u32 {
    let wanted = input.saturating_mul(3) / 2 + 32;
    let left = context.saturating_sub(system.saturating_add(input).saturating_add(PROMPT_OVERHEAD));
    wanted.min(left).max(16)
}

/// The line one segment becomes in a batch, with `n` its number.
pub(crate) fn line(n: usize, text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("[{n}] {one_line}\n")
}

/// Groups consecutive items into batches of at most `budget` tokens, given each item's cost. An
/// item over the budget on its own gets a batch of its own, because a segment is never split.
pub(crate) fn plan_batches(costs: &[u32], budget: u32) -> Vec<Range<usize>> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut used = 0u32;
    for (ix, cost) in costs.iter().enumerate() {
        if ix > start && used.saturating_add(*cost) > budget {
            batches.push(start..ix);
            start = ix;
            used = 0;
        }
        used = used.saturating_add(*cost);
    }
    if start < costs.len() {
        batches.push(start..costs.len());
    }
    batches
}

/// The translated lines of a reply, when it numbers exactly `1..=expected`, each once and in
/// order. Chatter before the first number and code fences are ignored. A line with no number
/// continues the one before it. Anything else is `None`.
pub(crate) fn parse_reply(reply: &str, expected: usize) -> Option<Vec<String>> {
    let mut entries: Vec<(usize, String)> = Vec::new();
    for raw in reply.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        match marker(line) {
            Some((n, rest)) => entries.push((n, rest.to_owned())),
            None => {
                if let Some((_, text)) = entries.last_mut() {
                    text.push(' ');
                    text.push_str(line);
                }
            }
        }
    }
    let in_order = entries.iter().enumerate().all(|(i, (n, _))| *n == i + 1);
    (entries.len() == expected && in_order)
        .then(|| entries.into_iter().map(|(_, text)| text).collect())
}

/// `[12] text` as `(12, "text")`.
fn marker(line: &str) -> Option<(usize, &str)> {
    let inner = line.strip_prefix('[')?;
    let (digits, rest) = inner.split_once(']')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, rest.trim()))
}

/// What a lone segment's reply means when it is not numbered: the reply, with a leading number
/// dropped when there is one.
fn bare(reply: &str) -> String {
    let text = reply.trim();
    match marker(text) {
        Some((_, rest)) => rest.to_owned(),
        None => text.to_owned(),
    }
}

/// One request to the model.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Call<'a> {
    pub system: &'a str,
    pub user: &'a str,
    pub max_output_tokens: u32,
}

/// Translates `texts` in one request, or in two halves when the reply does not number them back.
/// `call` runs one request and returns the reply. `output` is how many tokens a reply to this
/// user message may take.
pub(crate) fn translate_batch<E>(
    texts: &[&str],
    system: &str,
    output: &dyn Fn(&str) -> u32,
    call: &mut dyn FnMut(Call<'_>) -> Result<String, E>,
) -> Result<Vec<String>, E> {
    let user: String = texts
        .iter()
        .enumerate()
        .map(|(i, text)| line(i + 1, text))
        .collect();
    let reply = call(Call {
        system,
        user: &user,
        max_output_tokens: output(&user),
    })?;
    if let Some(lines) = parse_reply(&reply, texts.len()) {
        return Ok(lines);
    }
    if let [_] = texts {
        return Ok(vec![bare(&reply)]);
    }
    let (first, second) = texts.split_at(texts.len() / 2);
    let mut lines = translate_batch(first, system, output, call)?;
    lines.extend(translate_batch(second, system, output, call)?);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Channel, Revision, SpeakerId};
    use crate::models::ModelRef;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn segment(text: &str, language: Option<&str>) -> Segment {
        Segment {
            range: None,
            speaker: SpeakerId(1),
            channel: Channel::System,
            text: text.into(),
            language: language.map(tag),
        }
    }

    #[test]
    fn only_speech_in_another_or_unknown_language_needs_translating() {
        let en = tag("en");
        let cases = [
            ("hello", Some("en"), false),
            ("hello", Some("en-US"), false),
            ("kumusta", Some("tl"), true),
            ("kumusta", Some("fil"), true),
            ("???", None, true),
            ("   ", Some("tl"), false),
            ("", None, false),
        ];
        for (text, language, want) in cases {
            assert_eq!(
                needs_translation(&segment(text, language), &en),
                want,
                "{text:?} {language:?}"
            );
        }
        assert!(
            !needs_translation(&segment("hi", Some("en")), &tag("en-GB")),
            "a regional target still matches its language"
        );
    }

    #[test]
    fn a_mostly_english_meeting_translated_to_english_keeps_its_english_parts() {
        let transcript = Transcript {
            revision: Revision(1),
            segments: vec![
                segment("Ship it Friday.", Some("en")),
                segment("Sige, ako na bahala.", Some("tl")),
                segment("Sounds good.", Some("en")),
                segment("Salamat po.", Some("tl")),
            ],
            detected_languages: vec![tag("en"), tag("tl")],
            produced_by: ModelRef::parse("sona/whisper-small").unwrap(),
        };
        assert_eq!(pending(&transcript, &tag("en")), vec![1, 3]);
        assert_eq!(pending(&transcript, &tag("tl")), vec![0, 2]);
    }

    fn spans(costs: &[u32], budget: u32) -> Vec<(usize, usize)> {
        plan_batches(costs, budget)
            .into_iter()
            .map(|r| (r.start, r.end))
            .collect()
    }

    #[test]
    fn batches_pack_in_order_and_never_split_a_segment() {
        assert_eq!(spans(&[], 10), []);
        assert_eq!(spans(&[3, 3, 3], 10), [(0, 3)]);
        assert_eq!(spans(&[4, 4, 4], 10), [(0, 2), (2, 3)]);
        assert_eq!(spans(&[4, 6, 1], 10), [(0, 2), (2, 3)]);
        assert_eq!(spans(&[50, 2, 2], 10), [(0, 1), (1, 3)]);
        assert_eq!(spans(&[2, 50, 2], 10), [(0, 1), (1, 2), (2, 3)]);
    }

    #[test]
    fn a_batch_input_and_its_reply_fit_the_context_with_the_prompt() {
        for context in [2_048u32, 4_096, 8_192, 32_768] {
            let system = 90;
            let input = input_budget(context, system);
            let total = system + PROMPT_OVERHEAD + input + max_output(context, system, input);
            assert!(total <= context, "{context}: {total}");
            assert!(input <= MAX_BATCH_INPUT);
        }
        assert_eq!(input_budget(100, 200), 0, "no room, no input");
        assert!(max_output(100, 90, 50) >= 16);
    }

    #[test]
    fn a_reply_must_number_every_line_once_in_order() {
        let ok = "[1] hello\n[2] there\n";
        assert_eq!(
            parse_reply(ok, 2),
            Some(vec!["hello".into(), "there".into()])
        );
        let cases = [
            ("[1] hello\n", 2),
            ("[1] hello\n[2] a\n[3] b", 2),
            ("[2] a\n[1] b", 2),
            ("[1] a\n[1] b", 2),
            ("hello\nthere", 2),
            ("", 1),
        ];
        for (reply, expected) in cases {
            assert_eq!(parse_reply(reply, expected), None, "{reply:?}");
        }
    }

    #[test]
    fn a_reply_with_chatter_fences_and_wrapped_lines_still_parses() {
        let reply = "Here is the translation:\n```\n[1] hello\n[2] first half\nsecond half\n```\n";
        assert_eq!(
            parse_reply(reply, 2),
            Some(vec!["hello".into(), "first half second half".into()])
        );
        assert_eq!(
            parse_reply("[1] [laughs] ok", 1),
            Some(vec!["[laughs] ok".into()])
        );
    }

    /// A model that translates by uppercasing, and garbles any batch longer than `limit` lines.
    fn model(limit: usize) -> impl FnMut(Call<'_>) -> Result<String, ()> {
        move |call| {
            let lines: Vec<&str> = call.user.lines().collect();
            if lines.len() > limit {
                return Ok("I would be happy to help with that.".into());
            }
            Ok(lines
                .iter()
                .map(|l| l.to_uppercase())
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }

    #[test]
    fn a_batch_the_model_garbles_is_split_until_it_answers() {
        let texts = ["a", "b", "c", "d", "e"];
        let mut calls = 0;
        let mut inner = model(2);
        let out = translate_batch(&texts, "system", &|_| 100, &mut |call| {
            calls += 1;
            inner(call)
        })
        .unwrap();
        assert_eq!(out, ["A", "B", "C", "D", "E"].map(String::from));
        assert!(calls > 1 && calls <= 2 * texts.len(), "{calls} calls");
    }

    #[test]
    fn one_segment_the_model_will_not_number_takes_its_whole_reply() {
        let out = translate_batch(&["kumusta"], "system", &|_| 100, &mut |_| {
            Ok::<_, ()>("  hello  ".into())
        })
        .unwrap();
        assert_eq!(out, ["hello"]);
        let out = translate_batch(&["kumusta"], "system", &|_| 100, &mut |_| {
            Ok::<_, ()>("[7] hello".into())
        })
        .unwrap();
        assert_eq!(out, ["hello"]);
    }

    #[test]
    fn an_engine_error_stops_the_batch() {
        let out = translate_batch(&["a", "b"], "system", &|_| 100, &mut |_| Err("cancelled"));
        assert_eq!(out, Err("cancelled"));
    }

    #[test]
    fn the_request_numbers_lines_and_flattens_each_segment_to_one_line() {
        let mut seen = String::new();
        translate_batch(
            &["one\ntwo  words", "three"],
            "system",
            &|_| 100,
            &mut |call| {
                seen = call.user.to_owned();
                Ok::<_, ()>("[1] x\n[2] y".into())
            },
        )
        .unwrap();
        assert_eq!(seen, "[1] one two words\n[2] three\n");
    }

    #[test]
    fn the_prompt_names_the_target_and_asks_for_the_same_numbers_back() {
        let prompt = system_prompt(&tag("en"));
        assert!(prompt.contains("into English (BCP 47 tag en)"), "{prompt}");
        let tagalog = system_prompt(&tag("tl"));
        assert!(
            tagalog.contains("into Tagalog (BCP 47 tag tl)"),
            "{tagalog}"
        );
        assert!(system_prompt(&tag("xq")).contains("into the language with BCP 47 tag xq"));
        assert!(prompt.contains("same numbers"));
    }
}
