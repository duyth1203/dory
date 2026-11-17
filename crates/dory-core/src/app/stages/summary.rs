//! The summary policy: the prompts, and one pass or a map-reduce sized from the model's context.
//!
//! One pass when the whole transcript fits. Otherwise the transcript is cut into chunks that each
//! fit, each chunk becomes notes, and the notes become the summary. Notes that still do not fit are
//! merged in groups first, as many rounds as it takes. Every pass writes in the meeting's main
//! language, and the last pass writes the same three sections a one-pass summary has.

use std::fmt::Write as _;

use crate::domain::{Fraction, LangTag, SpeakerNames, Transcript};

/// Room left for the chat template the engine wraps around the prompt, and for `count_tokens`
/// being an estimate.
pub(crate) const PROMPT_OVERHEAD: u32 = 64;

/// What a completion is for. Only the summary pass streams to the user as a draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    Notes,
    Summary,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Call<'a> {
    pub pass: Pass,
    pub system: &'a str,
    pub user: &'a str,
    pub max_output_tokens: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SummaryError<E> {
    Engine(E),
    /// The system prompt and the output leave no room for any input.
    ContextTooSmall,
}

/// What each kind of pass is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prompt {
    SummaryOfTranscript,
    SummaryOfNotes,
    NotesOfTranscript,
    NotesOfNotes,
}

fn system_prompt(prompt: Prompt, language: Option<&LangTag>) -> String {
    let language = match language {
        Some(tag) if crate::domain::languages::find(tag.primary()).is_some() => {
            format!("{} (BCP 47 tag {tag})", crate::domain::languages::name(tag))
        }
        Some(tag) => format!("the language with BCP 47 tag {tag}"),
        None => "the language the meeting was held in".to_owned(),
    };
    let sections = format!(
        "Write the whole summary, headings included, in {language}. Use Markdown with three \
         sections in this order: key points, decisions, and action items. Keep each key point to \
         one short line and merge points that repeat. List every decision and every action item, \
         each once, with its owner and due date when the input names them. Write \"None\" under \
         a section with nothing to report."
    );
    let notes = format!(
        "Write in {language}. Keep every key point, decision, and action item with its owner, \
         and every name, number, and date. Use short bullet points and no headings."
    );
    match prompt {
        Prompt::SummaryOfTranscript => format!(
            "You summarize meeting transcripts. {sections} Use only what the transcript says."
        ),
        Prompt::SummaryOfNotes => format!(
            "You summarize a meeting from notes taken on its consecutive parts, in order. \
             {sections} Use only what the notes say."
        ),
        Prompt::NotesOfTranscript => format!(
            "You take notes on one part of a meeting transcript. {notes} Use only what the \
             transcript says."
        ),
        Prompt::NotesOfNotes => format!(
            "You combine notes taken on consecutive parts of one meeting into one set of notes, \
             dropping repeats. {notes} Use only what the notes say."
        ),
    }
}

/// One line per segment, `[mm:ss] Speaker: text`, each ending in a newline.
pub(crate) fn transcript_lines(transcript: &Transcript, names: &SpeakerNames) -> Vec<String> {
    transcript
        .segments
        .iter()
        .map(|segment| {
            let mut line = String::new();
            if let Some(range) = segment.range {
                let secs = range.start().0 / 1000;
                let _ = write!(line, "[{:02}:{:02}] ", secs / 60, secs % 60);
            }
            let _ = writeln!(
                line,
                "{}: {}",
                names.label(segment, transcript.revision),
                segment.text
            );
            line
        })
        .collect()
}

/// The summary may take a quarter of the context, up to what a long meeting's summary needs: a
/// 2-hour meeting with 40 action items runs past 2,000 tokens.
pub(crate) fn summary_output(context: u32) -> u32 {
    (context / 4).min(4096)
}

/// Notes stay small enough that any two of them fit one merge pass, so each round of merging at
/// least halves the notes.
pub(crate) fn notes_output(context: u32) -> u32 {
    (context / 8).min(1024)
}

/// The most input one pass gets, however large the context. Recall falls on long inputs: on a
/// synthetic 2-hour meeting with 40 action items, Gemma 4 E4B kept 34 in its summary from
/// 4096-token passes and 17 from 27k-token passes.
pub(crate) const MAX_PASS_INPUT: u32 = 3_000;

/// Tokens left for the input once the system prompt, the output, and the overhead are set aside,
/// up to [`MAX_PASS_INPUT`].
pub(crate) fn input_budget(context: u32, system: u32, output: u32) -> u32 {
    context
        .saturating_sub(
            system
                .saturating_add(output)
                .saturating_add(PROMPT_OVERHEAD),
        )
        .min(MAX_PASS_INPUT)
}

/// Packs `items` in order into chunks of at most `budget` tokens. An item over the budget on its
/// own is split at spaces. A single word over the budget stays whole, since it cannot be split.
pub(crate) fn plan_chunks(
    items: &[String],
    budget: u32,
    count: &dyn Fn(&str) -> u32,
) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut used = 0u32;
    for piece in items
        .iter()
        .flat_map(|item| split_to_fit(item, budget, count))
    {
        let tokens = count(&piece);
        if !chunk.is_empty() && used.saturating_add(tokens) > budget {
            chunks.push(std::mem::take(&mut chunk));
            used = 0;
        }
        chunk.push_str(&piece);
        used = used.saturating_add(tokens);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

fn split_to_fit(item: &str, budget: u32, count: &dyn Fn(&str) -> u32) -> Vec<String> {
    if count(item) <= budget {
        return vec![item.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for word in item.split_inclusive(' ') {
        let longer = format!("{piece}{word}");
        if !piece.is_empty() && count(&longer) > budget {
            pieces.push(std::mem::replace(&mut piece, word.to_owned()));
        } else {
            piece = longer;
        }
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// Runs the summary passes through `call` and returns the summary text. `progress` hears the
/// share of planned passes done, which never goes back.
pub(crate) fn summarize<E>(
    lines: &[String],
    language: Option<&LangTag>,
    instruction: Option<&str>,
    context: u32,
    count: &dyn Fn(&str) -> u32,
    call: &mut dyn FnMut(Call<'_>) -> Result<String, E>,
    progress: &mut dyn FnMut(Fraction),
) -> Result<String, SummaryError<E>> {
    let mut passes = Passes {
        language,
        instruction,
        context,
        count,
        call,
        progress,
        done: 0,
        planned: 1,
        shown: 0.0,
    };
    let transcript = lines.concat();
    if passes.fits(Prompt::SummaryOfTranscript, &transcript) {
        return passes.run(Prompt::SummaryOfTranscript, &transcript);
    }
    let budget = passes.budget(Prompt::NotesOfTranscript);
    if budget == 0 {
        return Err(SummaryError::ContextTooSmall);
    }
    let chunks = plan_chunks(lines, budget, count);
    passes.planned = chunks.len() + 1;
    let mut notes = chunks
        .iter()
        .map(|chunk| passes.run(Prompt::NotesOfTranscript, chunk))
        .collect::<Result<Vec<_>, _>>()?;
    loop {
        let items: Vec<String> = notes.iter().map(|n| format!("{}\n\n", n.trim())).collect();
        let all = items.concat();
        if passes.fits(Prompt::SummaryOfNotes, &all) {
            return passes.run(Prompt::SummaryOfNotes, &all);
        }
        let budget = passes.budget(Prompt::NotesOfNotes);
        let groups = plan_chunks(&items, budget, count);
        if budget == 0 || groups.len() >= notes.len() {
            return Err(SummaryError::ContextTooSmall);
        }
        passes.planned += groups.len();
        notes = groups
            .iter()
            .map(|group| passes.run(Prompt::NotesOfNotes, group))
            .collect::<Result<Vec<_>, _>>()?;
    }
}

struct Passes<'a, E> {
    language: Option<&'a LangTag>,
    /// What the user asked the summary to focus on. Only the final summary pass sees it; notes
    /// keep everything.
    instruction: Option<&'a str>,
    context: u32,
    count: &'a dyn Fn(&str) -> u32,
    call: &'a mut dyn FnMut(Call<'_>) -> Result<String, E>,
    progress: &'a mut dyn FnMut(Fraction),
    done: usize,
    planned: usize,
    shown: f32,
}

impl<E> Passes<'_, E> {
    fn output(&self, prompt: Prompt) -> u32 {
        match prompt {
            Prompt::SummaryOfTranscript | Prompt::SummaryOfNotes => summary_output(self.context),
            Prompt::NotesOfTranscript | Prompt::NotesOfNotes => notes_output(self.context),
        }
    }

    fn system(&self, prompt: Prompt) -> String {
        let system = system_prompt(prompt, self.language);
        match (prompt, self.instruction) {
            (Prompt::SummaryOfTranscript | Prompt::SummaryOfNotes, Some(instruction)) => format!(
                "{system} The user also asks for this, within the rules above: \"{instruction}\""
            ),
            _ => system,
        }
    }

    fn budget(&self, prompt: Prompt) -> u32 {
        let system = (self.count)(&self.system(prompt));
        input_budget(self.context, system, self.output(prompt))
    }

    fn fits(&self, prompt: Prompt, input: &str) -> bool {
        (self.count)(input) <= self.budget(prompt)
    }

    fn run(&mut self, prompt: Prompt, user: &str) -> Result<String, SummaryError<E>> {
        let pass = match prompt {
            Prompt::SummaryOfTranscript | Prompt::SummaryOfNotes => Pass::Summary,
            Prompt::NotesOfTranscript | Prompt::NotesOfNotes => Pass::Notes,
        };
        let system = self.system(prompt);
        let max_output_tokens = self.output(prompt);
        let text = (self.call)(Call {
            pass,
            system: &system,
            user,
            max_output_tokens,
        })
        .map_err(SummaryError::Engine)?;
        self.done += 1;
        self.shown = self
            .shown
            .max(self.done as f32 / self.planned.max(1) as f32);
        (self.progress)(Fraction::new(self.shown));
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Channel, Revision, Segment, SpeakerId};
    use crate::domain::{Ms, TimeRange};
    use crate::models::ModelRef;

    fn words(text: &str) -> u32 {
        text.split_whitespace().count() as u32
    }

    fn line(n: usize, words: usize) -> String {
        let body: Vec<String> = (0..words).map(|w| format!("w{n}.{w}")).collect();
        format!("{}\n", body.join(" "))
    }

    #[test]
    fn the_prompt_names_the_language_and_labels_speakers() {
        let segment = |speaker: u16, channel, start: u64, text: &str| Segment {
            range: Some(TimeRange::new(Ms(start), Ms(start + 1_000)).unwrap()),
            speaker: SpeakerId(speaker),
            channel,
            text: text.into(),
        };
        let transcript = Transcript {
            revision: Revision(2),
            segments: vec![
                segment(0, Channel::Mic, 0, "Ship it Friday."),
                segment(1, Channel::System, 65_000, "Agreed."),
                segment(2, Channel::System, 70_000, "I'll write the notes."),
            ],
            detected_languages: vec![LangTag::parse("vi").unwrap()],
            produced_by: ModelRef::parse("fake/stt").unwrap(),
        };
        let mut names = SpeakerNames::default();
        names.rename(Revision(2), SpeakerId(2), Some("Ana".into()));
        assert_eq!(
            transcript_lines(&transcript, &names).concat(),
            "[00:00] Me: Ship it Friday.\n[01:05] Speaker 1: Agreed.\n[01:10] Ana: I'll write the notes.\n"
        );
        let vi = LangTag::parse("vi").unwrap();
        for prompt in [
            Prompt::SummaryOfTranscript,
            Prompt::SummaryOfNotes,
            Prompt::NotesOfTranscript,
            Prompt::NotesOfNotes,
        ] {
            let system = system_prompt(prompt, Some(&vi));
            assert!(system.contains("BCP 47 tag vi"), "{prompt:?}: {system}");
        }
        let summary = system_prompt(Prompt::SummaryOfNotes, Some(&vi));
        assert!(summary.contains("key points, decisions, and action items"));
    }

    /// A small model does not know what a bare tag means: told "the language with BCP 47 tag
    /// tl", Gemma wrote an English summary under a stray "tl" line. The prompt names the language
    /// in words, and keeps the tag only as a disambiguator.
    #[test]
    fn the_prompt_names_the_language_in_words() {
        let tag = |t: &str| LangTag::parse(t).unwrap();
        for (lang, name) in [
            ("tl", "Tagalog (BCP 47 tag tl)"),
            ("vi", "Vietnamese (BCP 47 tag vi)"),
            ("en-US", "English (US) (BCP 47 tag en-US)"),
        ] {
            for prompt in [
                Prompt::SummaryOfTranscript,
                Prompt::SummaryOfNotes,
                Prompt::NotesOfTranscript,
                Prompt::NotesOfNotes,
            ] {
                let system = system_prompt(prompt, Some(&tag(lang)));
                assert!(
                    system.contains(&format!("in {name}")),
                    "{prompt:?}: {system}"
                );
            }
        }
        let unknown = system_prompt(Prompt::SummaryOfTranscript, Some(&tag("xq")));
        assert!(
            unknown.contains("in the language with BCP 47 tag xq"),
            "{unknown}"
        );
    }

    #[test]
    fn chunks_pack_lines_in_order_without_passing_the_budget() {
        let lines: Vec<String> = [3, 3, 3, 5, 2]
            .iter()
            .enumerate()
            .map(|(n, w)| line(n, *w))
            .collect();
        let chunks = plan_chunks(&lines, 6, &words);
        assert_eq!(
            chunks,
            vec![
                format!("{}{}", lines[0], lines[1]),
                lines[2].clone(),
                lines[3].clone(),
                lines[4].clone()
            ]
        );
        assert_eq!(plan_chunks(&lines, 100, &words), vec![lines.concat()]);
    }

    #[test]
    fn a_line_over_the_budget_is_split_at_spaces() {
        let lines = vec![line(0, 2), line(1, 10), line(2, 1)];
        let chunks = plan_chunks(&lines, 4, &words);
        assert_eq!(
            chunks.iter().map(|c| words(c)).collect::<Vec<_>>(),
            vec![2, 4, 4, 3],
            "{chunks:?}"
        );
        assert_eq!(
            chunks.concat().split_whitespace().collect::<Vec<_>>(),
            lines.concat().split_whitespace().collect::<Vec<_>>(),
            "every word once, in order"
        );
    }

    /// Calls a fake model that answers notes passes with `note` and the summary with `summary`.
    struct Run {
        calls: Vec<(Pass, String, String, u32)>,
        progress: Vec<f32>,
        result: Result<String, SummaryError<()>>,
    }

    fn run(lines: &[String], context: u32, note: &str) -> Run {
        let mut calls = Vec::new();
        let mut progress = Vec::new();
        let vi = LangTag::parse("vi").unwrap();
        let result = summarize(
            lines,
            Some(&vi),
            None,
            context,
            &words,
            &mut |c: Call<'_>| {
                calls.push((
                    c.pass,
                    c.system.to_owned(),
                    c.user.to_owned(),
                    c.max_output_tokens,
                ));
                Ok(match c.pass {
                    Pass::Notes => note.to_owned(),
                    Pass::Summary => "the summary".to_owned(),
                })
            },
            &mut |f| progress.push(f.get()),
        );
        Run {
            calls,
            progress,
            result,
        }
    }

    fn assert_every_call_fits(run: &Run, context: u32) {
        for (pass, system, user, max) in &run.calls {
            assert!(
                words(system) + words(user) + max + PROMPT_OVERHEAD <= context,
                "{pass:?} does not fit {context}: {} + {} + {max}",
                words(system),
                words(user)
            );
        }
    }

    fn assert_progress_climbs_to_done(run: &Run) {
        assert!(
            run.progress.windows(2).all(|w| w[0] <= w[1]),
            "{:?}",
            run.progress
        );
        assert_eq!(run.progress.last(), Some(&1.0));
    }

    #[test]
    fn a_long_transcript_is_split_even_when_the_context_could_hold_it() {
        let lines: Vec<String> = (0..400).map(|n| line(n, 10)).collect();
        let run = run(&lines, 131_072, "notes");
        assert!(run.calls.len() > 2, "{} passes", run.calls.len());
        assert!(
            run.calls
                .iter()
                .all(|(_, _, user, _)| words(user) <= MAX_PASS_INPUT),
            "no pass gets more than MAX_PASS_INPUT"
        );
    }

    #[test]
    fn a_transcript_that_fits_is_summarized_in_one_pass() {
        let lines: Vec<String> = (0..20).map(|n| line(n, 10)).collect();
        let run = run(&lines, 8_192, "notes");
        assert_eq!(run.result, Ok("the summary".to_owned()));
        assert_eq!(run.calls.len(), 1);
        let (pass, system, user, max) = &run.calls[0];
        assert_eq!(
            (*pass, user.as_str(), *max),
            (Pass::Summary, lines.concat().as_str(), 2048)
        );
        assert_eq!(
            system,
            &system_prompt(
                Prompt::SummaryOfTranscript,
                Some(&LangTag::parse("vi").unwrap())
            )
        );
        assert_progress_climbs_to_done(&run);
    }

    #[test]
    fn a_transcript_that_does_not_fit_is_mapped_to_notes_then_reduced() {
        let lines: Vec<String> = (0..60).map(|n| line(n, 10)).collect();
        let context = 400;
        let run = run(&lines, context, "a short note");
        assert_eq!(run.result, Ok("the summary".to_owned()));
        assert_every_call_fits(&run, context);
        let (last, maps) = run.calls.split_last().unwrap();
        assert_eq!(last.0, Pass::Summary);
        assert!(maps.len() > 1 && maps.iter().all(|c| c.0 == Pass::Notes));
        assert_eq!(
            maps.iter().map(|c| c.2.as_str()).collect::<String>(),
            lines.concat(),
            "every line goes to exactly one notes pass, in order"
        );
        assert_eq!(last.2.matches("a short note").count(), maps.len());
        assert_progress_climbs_to_done(&run);
    }

    #[test]
    fn notes_that_still_do_not_fit_are_merged_until_they_do() {
        let lines: Vec<String> = (0..120).map(|n| line(n, 10)).collect();
        let context = 400;
        let long_note = vec!["note"; 45].join(" ");
        let run = run(&lines, context, &long_note);
        assert_eq!(run.result, Ok("the summary".to_owned()));
        assert_every_call_fits(&run, context);
        let merges = run
            .calls
            .iter()
            .filter(|c| c.0 == Pass::Notes && c.2.contains("note note"))
            .count();
        assert!(merges > 0, "some notes passes read notes, not transcript");
        assert_eq!(run.calls.last().unwrap().0, Pass::Summary);
        assert_progress_climbs_to_done(&run);
    }

    #[test]
    fn a_context_with_no_room_for_input_is_refused() {
        let lines = vec![line(0, 10)];
        let run = run(&lines, 100, "note");
        assert_eq!(run.result, Err(SummaryError::ContextTooSmall));
    }
}

#[cfg(test)]
mod instruction_tests {
    use super::*;

    #[test]
    fn the_instruction_follows_the_fixed_rules_in_the_summary_pass_only() {
        let lines: Vec<String> = (0..60)
            .map(|n| format!("[00:{n:02}] Speaker 1: {}\n", "word ".repeat(10)))
            .collect();
        let mut systems = Vec::new();
        let words = |t: &str| t.split_whitespace().count() as u32;
        summarize::<()>(
            &lines,
            None,
            Some("focus on the budget"),
            400,
            &words,
            &mut |c: Call<'_>| {
                systems.push((c.pass, c.system.to_owned()));
                Ok("ok".into())
            },
            &mut |_| {},
        )
        .unwrap();
        assert!(systems.len() > 1, "a long transcript takes notes first");
        for (pass, system) in &systems {
            let has = system.ends_with("within the rules above: \"focus on the budget\"");
            assert_eq!(has, *pass == Pass::Summary, "{pass:?}: {system}");
        }
        let mut plain = Vec::new();
        summarize::<()>(
            &lines[..2],
            None,
            None,
            4000,
            &words,
            &mut |c: Call<'_>| {
                plain.push(c.system.to_owned());
                Ok("ok".into())
            },
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(plain, [system_prompt(Prompt::SummaryOfTranscript, None)]);
    }
}
