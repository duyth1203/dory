//! The translation policy: which segments need translating, how they are batched to fit the
//! model's context, the prompts, and how a reply is read back. A summary is translated the same
//! way, a line at a time, with the Markdown around each line kept by code.
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
    prompt("a meeting transcript", target, "")
}

/// The same instructions for the lines of a summary, which may carry Markdown emphasis.
pub(crate) fn summary_system_prompt(target: &LangTag) -> String {
    prompt(
        "a meeting summary",
        target,
        " Keep Markdown emphasis marks such as ** where they are.",
    )
}

fn prompt(of: &str, target: &LangTag, extra: &str) -> String {
    let language = crate::domain::languages::for_prompt(target);
    format!(
        "You translate lines of {of} into {language}. The input is numbered \
         lines like \"[1] text\". Reply with exactly the same numbered lines, translated, one \
         per input line, with the same numbers and nothing else. Keep names, numbers, and \
         technical terms as they are. A line already in {language} stays as it is. Never merge, \
         split, skip, answer, or explain a line. Translate only.{extra}"
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

/// One line of summary Markdown. `markup` is what the line keeps whatever the language: its
/// indent, a heading's `#` run, a bullet or a number, a quote mark, a task box. `text` is what is
/// translated. A line with nothing to read, a blank line or a rule or a line in a code fence, is
/// all `markup` and has no text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SummaryLine {
    pub markup: String,
    pub text: String,
}

impl SummaryLine {
    fn verbatim(line: &str) -> SummaryLine {
        SummaryLine {
            markup: line.to_owned(),
            text: String::new(),
        }
    }
}

/// Splits `markdown` at every `\n`, so joining the lines untranslated gives the input back byte
/// for byte, a trailing newline and a `\r` included.
pub(crate) fn summary_lines(markdown: &str) -> Vec<SummaryLine> {
    let mut fenced = false;
    markdown
        .split('\n')
        .map(|line| {
            let start = line.trim_start();
            if start.starts_with("```") || start.starts_with("~~~") {
                fenced = !fenced;
                return SummaryLine::verbatim(line);
            }
            if fenced {
                return SummaryLine::verbatim(line);
            }
            let (markup, text) = line.split_at(markup_len(line));
            if text.chars().any(char::is_alphanumeric) {
                SummaryLine {
                    markup: markup.to_owned(),
                    text: text.to_owned(),
                }
            } else {
                SummaryLine::verbatim(line)
            }
        })
        .collect()
}

/// Spaces and tabs at the start of `s`.
fn blanks(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}

/// How much of `line` is markup: the indent, quote marks, then one heading, bullet, or number.
fn markup_len(line: &str) -> usize {
    let mut at = blanks(line);
    while line[at..].starts_with('>') {
        at += 1;
        at += blanks(&line[at..]);
    }
    let rest = &line[at..];
    let hashes = rest.bytes().take_while(|b| *b == b'#').count();
    let after_hashes = &rest[hashes..];
    if (1..=6).contains(&hashes)
        && (after_hashes.is_empty() || after_hashes.starts_with([' ', '\t']))
    {
        return at + hashes + blanks(after_hashes);
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (marker, bullet) = if (1..=9).contains(&digits) && rest[digits..].starts_with(['.', ')']) {
        (digits + 1, false)
    } else if rest.starts_with(['-', '*', '+']) {
        (1, true)
    } else {
        return at;
    };
    let after = &rest[marker..];
    if !after.starts_with([' ', '\t']) {
        return at;
    }
    let mut len = at + marker + blanks(after);
    let task = &line[len..];
    if bullet && ["[ ] ", "[x] ", "[X] "].iter().any(|b| task.starts_with(b)) {
        len += 4 + blanks(&task[4..]);
    }
    len
}

/// The lines back as Markdown, each text part replaced by the next of `translated`, in order. A
/// blank reply keeps the line's own text. A reply is one line whatever the model wrote, so the
/// number of lines, their order, and all markup are what the summary had. A line that was bold
/// from end to end stays bold.
pub(crate) fn join_lines(lines: &[SummaryLine], translated: &[String]) -> String {
    let mut replies = translated.iter();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.text.is_empty() {
            out.push(line.markup.clone());
            continue;
        }
        let reply = replies.next().map_or("", String::as_str);
        let one_line = if reply.contains(['\n', '\r']) {
            reply
                .split(['\n', '\r'])
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            reply.to_owned()
        };
        let text = if one_line.trim().is_empty() {
            line.text.clone()
        } else {
            let original = line.text.trim();
            let bold = original.len() > 4 && original.starts_with("**") && original.ends_with("**");
            let mut text = if bold && !(one_line.starts_with("**") && one_line.ends_with("**")) {
                format!("**{}**", one_line.trim_matches('*'))
            } else {
                one_line
            };
            if line.text.ends_with('\r') {
                text.push('\r');
            }
            text
        };
        out.push(format!("{}{text}", line.markup));
    }
    out.join("\n")
}

/// What translating a summary needs from its surroundings: sizes taken from the model's
/// context, and the calls to the model.
pub(crate) struct SummaryRun<'a, E> {
    pub system: &'a str,
    /// Tokens of input one batch may take.
    pub budget: u32,
    pub cost: &'a dyn Fn(&str) -> u32,
    pub output: &'a dyn Fn(&str) -> u32,
    /// Called before each batch with the batches done and all of them. An error stops the run.
    pub before_batch: &'a mut dyn FnMut(usize, usize) -> Result<(), E>,
    pub call: &'a mut dyn FnMut(Call<'_>) -> Result<String, E>,
}

/// The summary in another language with the Markdown of every line as it was, or `None` when no
/// line has text to translate.
pub(crate) fn translate_summary<E>(
    markdown: &str,
    run: SummaryRun<'_, E>,
) -> Result<Option<String>, E> {
    let lines = summary_lines(markdown);
    let texts: Vec<&str> = lines
        .iter()
        .filter(|l| !l.text.is_empty())
        .map(|l| l.text.as_str())
        .collect();
    if texts.is_empty() {
        return Ok(None);
    }
    let costs: Vec<u32> = texts.iter().map(|t| (run.cost)(&line(99, t))).collect();
    let batches = plan_batches(&costs, run.budget);
    let mut translated = Vec::with_capacity(texts.len());
    for (done, range) in batches.iter().enumerate() {
        (run.before_batch)(done, batches.len())?;
        translated.extend(translate_batch(
            &texts[range.clone()],
            run.system,
            run.output,
            run.call,
        )?);
    }
    Ok(Some(join_lines(&lines, &translated)))
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
    fn a_taglish_segment_is_decided_whole_by_its_main_language() {
        let taglish = segment("Bagaman wala ako present, I will join later.", Some("tl"));
        assert!(needs_translation(&taglish, &tag("en")), "translated whole");
        assert!(!needs_translation(&taglish, &tag("tl")), "kept whole");
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

    const SUMMARY: &str = "# Summary\n\n## Key points\n- Ship it **Friday**.\n* Login is fixed.\n+ Docs next.\n\n### Decisions\nNone\n\n## Action items\n1. Maria reviews invoices.\n12) Ana books the room.\n   - nested under Ana\n\t* tab nested\n- [ ] open task\n- [x] done task\n> Quoted note\n---\n**Owners**\n";

    fn parts(markdown: &str) -> Vec<(String, String)> {
        summary_lines(markdown)
            .into_iter()
            .map(|l| (l.markup, l.text))
            .collect()
    }

    fn untranslated(lines: &[SummaryLine]) -> Vec<String> {
        lines
            .iter()
            .filter(|l| !l.text.is_empty())
            .map(|l| l.text.clone())
            .collect()
    }

    #[test]
    fn markup_is_the_headings_bullets_numbers_and_indents_and_the_text_is_the_rest() {
        let got = parts(SUMMARY);
        let pair = |m: &str, t: &str| (m.to_owned(), t.to_owned());
        let want = [
            pair("# ", "Summary"),
            pair("", ""),
            pair("## ", "Key points"),
            pair("- ", "Ship it **Friday**."),
            pair("* ", "Login is fixed."),
            pair("+ ", "Docs next."),
            pair("", ""),
            pair("### ", "Decisions"),
            pair("", "None"),
            pair("", ""),
            pair("## ", "Action items"),
            pair("1. ", "Maria reviews invoices."),
            pair("12) ", "Ana books the room."),
            pair("   - ", "nested under Ana"),
            pair("\t* ", "tab nested"),
            pair("- [ ] ", "open task"),
            pair("- [x] ", "done task"),
            pair("> ", "Quoted note"),
            pair("---", ""),
            pair("", "**Owners**"),
            pair("", ""),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn each_heading_level_and_a_lone_hash_run_is_markup() {
        for level in 1..=6 {
            let hashes = "#".repeat(level);
            assert_eq!(
                parts(&format!("{hashes} Points")),
                [(format!("{hashes} "), "Points".to_owned())]
            );
        }
        assert_eq!(
            parts("####### seven"),
            [(String::new(), "####### seven".to_owned())]
        );
        assert_eq!(parts("#hashtag"), [(String::new(), "#hashtag".to_owned())]);
        assert_eq!(parts("##"), [("##".to_owned(), String::new())]);
    }

    #[test]
    fn a_line_that_only_looks_like_a_marker_keeps_its_text() {
        for line in [
            "**bold start** then more",
            "-5 degrees",
            "2024 was long",
            "*emphasis* word",
        ] {
            assert_eq!(parts(line), [(String::new(), line.to_owned())], "{line}");
        }
    }

    #[test]
    fn a_line_with_only_bold_text_is_text_and_stays_bold_when_the_reply_drops_the_marks() {
        let lines = summary_lines("**Decisions**");
        assert_eq!(lines[0].markup, "");
        assert_eq!(lines[0].text, "**Decisions**");
        assert_eq!(
            join_lines(&lines, &["Mga desisyon".into()]),
            "**Mga desisyon**"
        );
        assert_eq!(
            join_lines(&lines, &["**Mga desisyon**".into()]),
            "**Mga desisyon**"
        );
        let mixed = summary_lines("**Owner:** Ana");
        assert_eq!(
            join_lines(&mixed, &["**May-ari:** Ana".into()]),
            "**May-ari:** Ana"
        );
    }

    #[test]
    fn a_blank_line_a_rule_and_a_code_fence_have_no_text() {
        let lines = summary_lines("a\n\n   \n***\n```\n# not a heading\n```\nb");
        let with_text: Vec<&str> = lines
            .iter()
            .filter(|l| !l.text.is_empty())
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(with_text, ["a", "b"]);
        assert_eq!(lines.len(), 8);
    }

    #[test]
    fn joining_the_untranslated_lines_gives_back_the_input_byte_for_byte() {
        let inputs = [
            SUMMARY,
            "",
            "\n",
            "no newline at the end",
            "trailing spaces  \nand\ttabs\t\n\n\n",
            "win\r\nlines\r\n- b\r\n",
            "None",
            "# only a heading",
            "- é ü ñ 日本語\n\n   \n",
        ];
        for input in inputs {
            let lines = summary_lines(input);
            assert_eq!(
                join_lines(&lines, &untranslated(&lines)),
                input,
                "{input:?}"
            );
        }
    }

    #[test]
    fn a_translated_line_takes_back_its_own_markup_and_none_of_the_models() {
        let lines = summary_lines("## Key points\n- Ship it.\n\n1. Do it.\n   - sub\n");
        let got = join_lines(
            &lines,
            &[
                "Mga punto".into(),
                "Ipadala.".into(),
                "Gawin.".into(),
                "ilalim".into(),
            ],
        );
        assert_eq!(got, "## Mga punto\n- Ipadala.\n\n1. Gawin.\n   - ilalim\n");
        assert_eq!(got.lines().count(), 5);
    }

    #[test]
    fn a_reply_that_wraps_or_comes_back_blank_cannot_change_the_line_count() {
        let lines = summary_lines("- one\n- two\n- three");
        let got = join_lines(&lines, &["uno\ndos extra\n\n- tres".into(), "  ".into()]);
        assert_eq!(got, "- uno dos extra - tres\n- two\n- three");
        assert_eq!(got.split('\n').count(), 3);
        let carriage = summary_lines("- a\r\n- b\r\n");
        assert_eq!(
            join_lines(&carriage, &["x".into(), "y".into()]),
            "- x\r\n- y\r\n"
        );
    }

    /// A model that translates each numbered line by upper-casing it, and counts its calls.
    fn run_summary(
        markdown: &str,
        budget: u32,
    ) -> (Option<String>, Vec<String>, Vec<(usize, usize)>) {
        let mut users = Vec::new();
        let mut steps = Vec::new();
        let out = translate_summary(
            markdown,
            SummaryRun::<()> {
                system: "system",
                budget,
                cost: &|text| text.split_whitespace().count() as u32,
                output: &|_| 100,
                before_batch: &mut |done, all| {
                    steps.push((done, all));
                    Ok(())
                },
                call: &mut |call| {
                    users.push(call.user.to_owned());
                    Ok(call.user.to_uppercase())
                },
            },
        )
        .unwrap();
        (out, users, steps)
    }

    #[test]
    fn a_summary_goes_to_the_model_as_numbered_text_with_no_markup() {
        let (out, users, steps) = run_summary("## Key points\n- Ship it.\n\n1. Do it.", 1_000);
        assert_eq!(users, ["[1] Key points\n[2] Ship it.\n[3] Do it.\n"]);
        assert_eq!(steps, [(0, 1)]);
        assert_eq!(
            out.as_deref(),
            Some("## KEY POINTS\n- SHIP IT.\n\n1. DO IT.")
        );
    }

    #[test]
    fn a_long_summary_goes_in_batches_and_each_batch_is_announced_first() {
        let md: String = (1..=7).map(|n| format!("- item number {n}\n")).collect();
        let (out, users, steps) = run_summary(&md, 8);
        assert!(users.len() > 2, "{} batches", users.len());
        assert_eq!(steps.len(), users.len());
        assert!(
            steps
                .iter()
                .enumerate()
                .all(|(i, s)| *s == (i, users.len()))
        );
        let want: String = (1..=7).map(|n| format!("- ITEM NUMBER {n}\n")).collect();
        assert_eq!(out, Some(want));
    }

    #[test]
    fn a_summary_with_nothing_to_translate_makes_no_call() {
        for md in ["", "\n\n", "---\n***", "```\ncode\n```"] {
            let (out, users, steps) = run_summary(md, 100);
            assert_eq!((out, users, steps), (None, vec![], vec![]), "{md:?}");
        }
    }

    #[test]
    fn a_stop_before_a_batch_ends_the_run_without_a_result() {
        let out = translate_summary(
            "- a\n- b",
            SummaryRun {
                system: "system",
                budget: 100,
                cost: &|_| 1,
                output: &|_| 100,
                before_batch: &mut |_, _| Err("cancelled"),
                call: &mut |_| Ok::<_, &str>(String::new()),
            },
        );
        assert_eq!(out, Err("cancelled"));
    }

    #[test]
    fn the_summary_prompt_is_the_transcript_prompt_for_summary_lines() {
        let transcript = system_prompt(&tag("en"));
        let summary = summary_system_prompt(&tag("en"));
        assert!(transcript.starts_with("You translate lines of a meeting transcript into"));
        assert!(summary.starts_with("You translate lines of a meeting summary into"));
        assert!(summary.contains("same numbers") && summary.contains("Markdown emphasis"));
    }
}
