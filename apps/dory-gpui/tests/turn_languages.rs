//! A bilingual transcript's turns carry the language they were spoken in (B4): the label a screen
//! reader speaks, the export text, and the rule that tags only the minority language.

use dory_core::domain::{
    Channel, ChannelLayout, LangTag, LanguagePref, Ms, Recording, RecordingId, Revision, Segment,
    SpeakerId, SpeakerNames, TimeRange, Timestamp, Transcript, Translation,
};
use dory_core::models::ModelRef;
use dory_gpui::app::{Detail, TranscriptText, Turn, translated_turns, turns};
use dory_gpui::export::{ExportFormat, render};
use dory_gpui::transcript_view::{rename_hint, spoken_turn};

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
    let translation = Translation {
        target: tag("en"),
        transcript_revision: Revision(1),
        produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
        texts: vec![
            Some("Okay, let's go on.".into()),
            None,
            Some("I'll take care of it.".into()),
            Some("Thank you.".into()),
        ],
    };
    let detail = detail(meeting(), Some(translation));
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
