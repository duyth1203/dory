//! Search as product rules: what a word is, how typed text becomes a query, and where a query
//! matches. A store adapter indexes [`tokens`] and looks up [`Query::clauses`]. Views mark
//! [`highlights`] and show a [`snippet`], so the index and the marks agree on every word.

mod excerpt;
mod matcher;
mod query;
mod tokens;
mod vocabulary;

pub use excerpt::{Excerpt, SNIPPET_WIDTH, Source, excerpt, summary_blocks};
pub use matcher::{Located, Snippet, highlights, locate, matches, snippet};
pub use query::{Clause, Correction, Query};
pub use tokens::{TOKENS_VERSION, Token, fold, tokens};
pub use vocabulary::Vocabulary;

use crate::domain::RecordingRow;

/// One recording a search found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub row: RecordingRow,
    /// The title alone matches every clause.
    pub in_title: bool,
}

/// What [`crate::app::Library::search`] returns: the query that found the hits, for marking
/// matches, and the hits, newest first. When nothing matched what the user typed and a correction
/// did, `query` is the corrected one and `corrections` names each word it replaced. Otherwise
/// `corrections` is empty.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResults {
    pub query: Query,
    pub hits: Vec<Found>,
    pub corrections: Vec<Correction>,
}

impl SearchResults {
    /// See [`sections`].
    pub fn sections(&self) -> [(Section, Vec<&Found>); 2] {
        sections(&self.hits)
    }
}

/// The two lists the sidebar shows while the search field has text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// The title alone matches.
    Titles,
    /// Everything else: the match is in speaker names, the summary, or the transcript.
    Conversations,
}

/// Splits `hits` into [`Section::Titles`] and [`Section::Conversations`], each keeping the order
/// it had, so newest first stays newest first. A section may be empty.
pub fn sections(hits: &[Found]) -> [(Section, Vec<&Found>); 2] {
    let (titles, conversations) = hits.iter().partition(|found| found.in_title);
    [
        (Section::Titles, titles),
        (Section::Conversations, conversations),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ChannelLayout, Recording, RecordingId, RecordingRow, SummaryFlag, Timestamp,
    };

    fn found(id: &str, created: i64, in_title: bool) -> Found {
        Found {
            row: RecordingRow {
                recording: Recording {
                    id: RecordingId::parse(id).unwrap(),
                    title: None,
                    created_at: Timestamp(created),
                    language: Default::default(),
                    layout: ChannelLayout::MicSystem,
                    take: None,
                },
                job: None,
                has_transcript: false,
                summary: SummaryFlag::None,
            },
            in_title,
        }
    }

    fn ids(hits: &[&Found]) -> Vec<String> {
        hits.iter()
            .map(|f| f.row.recording.id.to_string())
            .collect()
    }

    #[test]
    fn sections_split_title_hits_from_conversation_hits_and_keep_the_order() {
        let hits = [
            found("a", 500, false),
            found("b", 400, true),
            found("c", 300, false),
            found("d", 200, true),
        ];
        let [(first, titles), (second, conversations)] = sections(&hits);
        assert_eq!((first, second), (Section::Titles, Section::Conversations));
        assert_eq!(ids(&titles), ["b", "d"]);
        assert_eq!(ids(&conversations), ["a", "c"]);
    }

    #[test]
    fn a_section_with_no_hits_is_empty_rather_than_missing() {
        let only_titles = [found("a", 2, true), found("b", 1, true)];
        let [(_, titles), (_, conversations)] = sections(&only_titles);
        assert_eq!(
            (ids(&titles), conversations.len()),
            (vec!["a".into(), "b".into()], 0)
        );
        let only_talk = [found("a", 2, false)];
        let [(_, titles), (_, conversations)] = sections(&only_talk);
        assert_eq!((titles.len(), ids(&conversations)), (0, vec!["a".into()]));
        let [(_, titles), (_, conversations)] = sections(&[]);
        assert_eq!((titles.len(), conversations.len()), (0, 0));
    }
}
