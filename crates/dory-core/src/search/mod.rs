//! Search as product rules: what a word is, how typed text becomes a query, and where a query
//! matches. A store adapter indexes [`tokens`] and looks up [`Query::clauses`]. Views mark
//! [`highlights`] and show a [`snippet`], so the index and the marks agree on every word.

mod matcher;
mod query;
mod tokens;
mod vocabulary;

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
