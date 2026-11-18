//! The line under a conversation hit: where in the recording the query matches, and a snippet of
//! it. The caller reads the text, and these rules choose the place, so one function decides what
//! a row says for every store.

use super::matcher::{Snippet, locate, snippet};
use super::query::Query;

/// Characters a snippet aims for. The sidebar shows one line at 125% text and below and two
/// above, and a snippet starts at most a third of this before its match, so the match is on the
/// first line at every size.
pub const SNIPPET_WIDTH: usize = 60;

/// Where an [`Excerpt`] was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Transcript,
    Summary,
    /// A name the user gave a speaker.
    Speakers,
}

/// What a conversation hit shows: a snippet, where it came from, and which of that source's items
/// it was cut from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub source: Source,
    pub item: usize,
    pub snippet: Snippet,
}

/// The snippet of `text` for the item [`locate`] chose. A phrase that no single item holds was
/// located by its words, so the snippet follows them too.
fn snippet_of(query: &Query, text: &str, width: usize) -> Option<Snippet> {
    snippet(query, text, width).or_else(|| {
        let words = query.phrase_words()?;
        snippet(&words, text, width)
    })
}

fn in_first_match(query: &Query, items: &[&str], source: Source, width: usize) -> Option<Excerpt> {
    let item = locate(query, items)?.item;
    Some(Excerpt {
        source,
        item,
        snippet: snippet_of(query, items[item], width)?,
    })
}

/// The first place `query` matches, looking in the transcript's `translated` turns, then in its
/// `spoken` turns, then in the summary translation's `translated_blocks`, then in the summary's
/// `blocks`, then in `speakers`, the names the user gave. `translated` is empty without a
/// translation, and turn `n` is the same turn in both, so a hit in a translation is quoted as the
/// transcript shows it. `translated_blocks` is empty without a summary translation, and `item`
/// counts blocks of the text the excerpt came from. `None` when none of them holds a word of the
/// query.
pub fn excerpt(
    query: &Query,
    translated: &[&str],
    spoken: &[&str],
    translated_blocks: &[&str],
    blocks: &[&str],
    speakers: &[&str],
) -> Option<Excerpt> {
    excerpt_within(
        query,
        translated,
        spoken,
        translated_blocks,
        blocks,
        speakers,
        SNIPPET_WIDTH,
    )
}

/// [`excerpt`] with snippets of about `width` characters.
pub fn excerpt_within(
    query: &Query,
    translated: &[&str],
    spoken: &[&str],
    translated_blocks: &[&str],
    blocks: &[&str],
    speakers: &[&str],
    width: usize,
) -> Option<Excerpt> {
    in_first_match(query, translated, Source::Transcript, width)
        .or_else(|| in_first_match(query, spoken, Source::Transcript, width))
        .or_else(|| in_first_match(query, translated_blocks, Source::Summary, width))
        .or_else(|| in_first_match(query, blocks, Source::Summary, width))
        .or_else(|| in_first_match(query, speakers, Source::Speakers, width))
}

/// The summary's Markdown as plain lines, so a snippet never shows a heading hash, a bullet, or
/// emphasis marks. Blank lines are dropped.
pub fn summary_blocks(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .map(|line| {
            let line = line.trim().trim_start_matches('#').trim_start();
            let line = line
                .strip_prefix(['-', '*', '+', '>'])
                .filter(|rest| rest.starts_with(' '))
                .unwrap_or(line);
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            let line = match line[digits..].strip_prefix(". ") {
                Some(rest) if digits > 0 => rest,
                _ => line,
            };
            line.trim()
                .replace("**", "")
                .replace("__", "")
                .replace('`', "")
        })
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query::parse(text).unwrap()
    }

    fn marked(found: &Excerpt) -> Vec<&str> {
        found
            .snippet
            .marks
            .iter()
            .map(|m| &found.snippet.text[m.clone()])
            .collect()
    }

    #[test]
    fn the_transcript_wins_over_the_summary_and_the_names() {
        let found = excerpt(
            &q("roadmap"),
            &[],
            &["hello", "we agreed the roadmap slips"],
            &[],
            &["Roadmap review"],
            &["Roadmap Rita"],
        )
        .unwrap();
        assert_eq!((found.source, found.item), (Source::Transcript, 1));
        assert_eq!(found.snippet.text, "we agreed the roadmap slips");
        assert_eq!(marked(&found), ["roadmap"]);
    }

    #[test]
    fn the_summary_is_used_when_no_turn_matches_and_names_when_neither_does() {
        let from_summary = excerpt(
            &q("budget"),
            &[],
            &["hello"],
            &[],
            &["Key points", "The budget is fine"],
            &["Sam"],
        )
        .unwrap();
        assert_eq!(from_summary.source, Source::Summary);
        assert_eq!(from_summary.snippet.text, "The budget is fine");

        let from_name = excerpt(
            &q("sam"),
            &[],
            &["hello"],
            &[],
            &["Key points"],
            &["Sam", "Lee"],
        )
        .unwrap();
        assert_eq!(from_name.source, Source::Speakers);
        assert_eq!(from_name.snippet.text, "Sam");
        assert_eq!(marked(&from_name), ["Sam"]);

        assert_eq!(
            excerpt(&q("zanzibar"), &[], &["hello"], &[], &["Key"], &["Sam"]),
            None
        );
        assert_eq!(excerpt(&q("zanzibar"), &[], &[], &[], &[], &[]), None);
    }

    #[test]
    fn a_phrase_split_across_turns_shows_the_first_turn_with_one_of_its_words() {
        let found = excerpt(
            &q("\"budget review\""),
            &[],
            &["intro", "about the budget", "review it is"],
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(found.snippet.text, "about the budget");
        assert_eq!(marked(&found), ["budget"]);
    }

    #[test]
    fn a_long_turn_is_cut_to_a_window_around_the_match() {
        let turn = "One two three four five six seven eight nine ten eleven twelve roadmap \
                    thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
        let found = excerpt(&q("roadmap"), &[], &[turn], &[], &[], &[]).unwrap();
        assert!(found.snippet.text.starts_with('…') && found.snippet.text.ends_with('…'));
        assert!(found.snippet.text.chars().count() <= SNIPPET_WIDTH + 2);
        assert_eq!(marked(&found), ["roadmap"]);
        let before = &found.snippet.text[..found.snippet.marks[0].start];
        assert!(
            before.chars().count() <= SNIPPET_WIDTH / 3 + 1,
            "{before:?}"
        );
    }

    #[test]
    fn summary_blocks_drop_markdown_syntax_and_blank_lines() {
        let md = "## Key points\n\n- **Ship** it `Friday`.\n* Second\n1. First step\n12. Twelfth\n> quoted\n-not a bullet\n";
        assert_eq!(
            summary_blocks(md),
            [
                "Key points",
                "Ship it Friday.",
                "Second",
                "First step",
                "Twelfth",
                "quoted",
                "-not a bullet"
            ]
        );
    }

    #[test]
    fn a_translated_turn_is_quoted_before_the_spoken_one() {
        let translated = ["Hello.", "The budget is approved."];
        let spoken = ["Hello.", "Aprubado na ang badyet."];
        let found = excerpt(
            &q("approved"),
            &translated,
            &spoken,
            &[],
            &["Approved"],
            &[],
        )
        .unwrap();
        assert_eq!((found.source, found.item), (Source::Transcript, 1));
        assert_eq!(found.snippet.text, "The budget is approved.");
        assert_eq!(marked(&found), ["approved"]);

        let found = excerpt(&q("aprubado"), &translated, &spoken, &[], &[], &[]).unwrap();
        assert_eq!(
            (found.source, found.item, found.snippet.text.as_str()),
            (Source::Transcript, 1, "Aprubado na ang badyet."),
            "a word only spoken is quoted as spoken"
        );
    }

    #[test]
    fn a_translated_summary_block_is_quoted_before_the_written_one() {
        let translated = ["Points", "The budget is approved"];
        let written = ["Mga punto", "Budget: aprubado na"];
        let found = excerpt(&q("budget"), &[], &["hello"], &translated, &written, &[]).unwrap();
        assert_eq!((found.source, found.item), (Source::Summary, 1));
        assert_eq!(found.snippet.text, "The budget is approved");
        assert_eq!(marked(&found), ["budget"]);

        let found = excerpt(&q("aprubado"), &[], &[], &translated, &written, &[]).unwrap();
        assert_eq!(
            (found.source, found.item, found.snippet.text.as_str()),
            (Source::Summary, 1, "Budget: aprubado na"),
            "a word only written is quoted as written"
        );

        let found = excerpt(&q("budget"), &[], &["the budget"], &translated, &[], &[]).unwrap();
        assert_eq!(
            found.source,
            Source::Transcript,
            "the transcript still wins"
        );
    }
}
