//! A bilingual transcript's turns carry the language they were spoken in (B4): the label a screen
//! reader speaks, the export text, and the rule that tags only the minority language.

use dory_core::domain::{
    Channel, ChannelLayout, LangTag, LanguagePref, Ms, Recording, RecordingId, Revision, Segment,
    SpeakerId, SpeakerNames, TimeRange, Timestamp, Transcript, Translation,
};
use dory_core::models::ModelRef;
use dory_gpui::a11y::text_runs;
use dory_gpui::app::{Detail, TranscriptText, Turn, translated_turns, turns};
use dory_gpui::export::{ExportFormat, render};
use dory_gpui::transcript_view::{rename_hint, spoken_turn};
use gpui_kit::Role;

fn tag(code: &str) -> LangTag {
    LangTag::parse(code).unwrap()
}

fn at(start: u64, end: u64) -> Option<TimeRange> {
    TimeRange::new(Ms(start), Ms(end))
}

/// Sam speaks Tagalog for a minute, English for 30 seconds, then Tagalog again.
fn meeting() -> Transcript {
    let line = |speaker, range, text: &str, language| Segment {
        range,
        speaker: SpeakerId(speaker),
        channel: Channel::System,
        text: text.to_owned(),
        language: Some(tag(language)),
    };
    Transcript {
        revision: Revision(1),
        segments: vec![
            line(1, at(750_000, 810_000), "Sige, ituloy natin.", "tl"),
            line(1, at(810_000, 840_000), "That works for me.", "en"),
            line(1, at(840_000, 900_000), "Ako na ang bahala.", "tl"),
            line(2, at(900_000, 930_000), "Salamat po.", "tl"),
        ],
        detected_languages: vec![tag("tl"), tag("en")],
        produced_by: ModelRef::parse("sona/whisper-small").unwrap(),
    }
}

fn detail(transcript: Transcript, translation: Option<Translation>) -> Detail {
    let mut names = SpeakerNames::default();
    names.rename(transcript.revision, SpeakerId(1), Some("Sam".into()));
    let turns_spoken = turns(&transcript, &names);
    let translated = translation
        .as_ref()
        .map(|t| translated_turns(&transcript, &names, t));
    Detail {
        recording: Recording {
            id: RecordingId::parse("2026-11-17T150131-000e").unwrap(),
            title: Some("Weekly sync".into()),
            created_at: Timestamp(1_763_391_691_000),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: None,
        },
        transcript: Some(transcript),
        summary: None,
        names,
        turns: turns_spoken,
        translation,
        translated_turns: translated,
        summary_translation: None,
        translating_into: None,
    }
}

fn tags(turns: &[Turn]) -> Vec<Option<&str>> {
    turns.iter().map(|t| t.tag.as_deref()).collect()
}

fn text_languages(turns: &[Turn]) -> Vec<Option<&str>> {
    turns
        .iter()
        .map(|t| t.text_language.as_ref().map(LangTag::as_str))
        .collect()
}

fn english_translation() -> Translation {
    Translation {
        target: tag("en"),
        transcript_revision: Revision(1),
        produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
        texts: vec![
            Some("Okay, let's go on.".into()),
            None,
            Some("I'll take care of it.".into()),
            Some("Thank you.".into()),
        ],
    }
}

#[test]
fn a_tagged_turn_reads_its_language_after_the_time_and_an_untagged_one_is_unchanged() {
    let detail = detail(meeting(), None);
    assert_eq!(
        tags(&detail.turns),
        [None, Some("English"), None, None],
        "the main language is never tagged"
    );
    assert_eq!(
        spoken_turn(&detail.turns[1]),
        "Sam, 13 minutes 30 seconds, English, That works for me."
    );
    assert_eq!(
        spoken_turn(&detail.turns[0]),
        "Sam, 12 minutes 30 seconds, Sige, ituloy natin."
    );
}

#[test]
fn a_translated_turn_says_what_it_was_translated_from_instead_of_the_tag() {
    let detail = detail(meeting(), Some(english_translation()));
    let translated = detail.translated_turns.as_ref().unwrap();
    assert_eq!(
        tags(translated),
        tags(&detail.turns),
        "both views tag the spoken language"
    );
    assert_eq!(
        spoken_turn(&translated[0]),
        "Sam, 12 minutes 30 seconds, translated from Tagalog, Okay, let's go on."
    );
    assert_eq!(
        spoken_turn(&translated[1]),
        "Sam, 13 minutes 30 seconds, English, That works for me.",
        "the English turn is not translated"
    );
}

#[test]
fn markdown_and_text_put_the_language_after_the_speaker_and_srt_is_unchanged() {
    let detail = detail(meeting(), None);
    let shown = TranscriptText::Spoken;
    let markdown = render(ExportFormat::Markdown, &detail, shown).unwrap();
    assert!(
        markdown.contains("\n**Sam (English)** [13:30]\nThat works for me.\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("\n**Sam** [12:30]\nSige, ituloy natin.\n"),
        "{markdown}"
    );
    let text = render(ExportFormat::Text, &detail, shown).unwrap();
    assert!(
        text.contains("\nSam (English) [13:30]\nThat works for me.\n"),
        "{text}"
    );
    let srt = render(ExportFormat::Srt, &detail, shown).unwrap();
    assert!(
        !srt.contains("English") && !srt.contains("Tagalog"),
        "{srt}"
    );
    assert!(srt.contains("That works for me."), "{srt}");
}

#[test]
fn renaming_a_speaker_counts_their_turns_across_both_languages() {
    let detail = detail(meeting(), None);
    assert_eq!(
        rename_hint(&detail.turns, SpeakerId(1), "Sam"),
        "Renames Sam in all 3 turns. Return to save, Escape to cancel."
    );
}

#[test]
fn a_meeting_in_one_language_shows_no_tags() {
    let mut one = meeting();
    for segment in &mut one.segments {
        segment.language = Some(tag("tl"));
    }
    one.detected_languages = vec![tag("tl")];
    let detail = detail(one, None);
    assert!(detail.turns.iter().all(|t| t.tag.is_none()));
    let markdown = render(ExportFormat::Markdown, &detail, TranscriptText::Spoken).unwrap();
    assert!(!markdown.contains('('), "{markdown}");
}

#[test]
fn a_turns_text_is_in_its_spoken_language_and_a_translated_turns_in_the_target() {
    let detail = detail(meeting(), Some(english_translation()));
    assert_eq!(
        text_languages(&detail.turns),
        [Some("tl"), Some("en"), Some("tl"), Some("tl")],
        "every turn as spoken reads in its own language, the main one too"
    );
    assert_eq!(
        text_languages(detail.translated_turns.as_ref().unwrap()),
        [Some("en"), Some("en"), Some("en"), Some("en")],
        "a translated turn reads in the target, and the untranslated English turn in English"
    );
}

#[test]
fn a_turn_whose_language_is_unknown_carries_none() {
    let mut unknown = meeting();
    for segment in &mut unknown.segments {
        segment.language = None;
    }
    unknown.detected_languages = Vec::new();
    let detail = detail(unknown, None);
    assert!(detail.turns.iter().all(|t| t.text_language.is_none()));
    let runs = text_runs(&detail.turns[0].text, None);
    assert!(runs.iter().all(|run| run.language().is_none()));
}

#[test]
fn a_turns_text_reaches_voiceover_as_runs_tagged_with_its_language() {
    let detail = detail(meeting(), None);
    let turn = &detail.turns[0];
    let runs = text_runs(&turn.text, turn.text_language.as_ref());
    assert!(runs.iter().all(|run| run.role() == Role::TextRun));
    assert!(runs.iter().all(|run| run.language() == Some("tl")));
    let read: String = runs.iter().filter_map(|run| run.value()).collect();
    assert_eq!(read, turn.text);
}

#[test]
fn a_long_turn_splits_into_runs_at_words_that_accesskit_can_index() {
    let text = "Ako na ang bahala sa lahat ng ito, huwag kang mag-alala. ".repeat(12);
    let text = text.trim_end();
    let runs = text_runs(text, Some(&tag("tl")));
    assert!(runs.len() > 1, "{} characters fit in one run", text.len());
    let read: String = runs.iter().filter_map(|run| run.value()).collect();
    assert_eq!(read, text);
    for run in &runs {
        let value = run.value().unwrap();
        let lengths = run.character_lengths();
        assert_eq!(lengths.len(), value.chars().count());
        assert!(lengths.len() <= usize::from(u8::MAX));
        assert_eq!(
            lengths.iter().map(|&l| usize::from(l)).sum::<usize>(),
            value.len()
        );
        let starts = run.word_starts();
        assert_eq!(
            starts.first(),
            Some(&0),
            "each run starts a word: {value:?}"
        );
        assert!(starts.windows(2).all(|w| w[0] < w[1]));
        assert!(starts.iter().all(|&s| usize::from(s) < lengths.len()));
    }
}
