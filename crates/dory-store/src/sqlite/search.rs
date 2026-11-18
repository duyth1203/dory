//! The search index: one row per recording in a contentless FTS5 table, keyed by
//! `search_keys.key`, because VACUUM may renumber the implicit rowid of `recordings`, whose key is
//! text.
//!
//! A row holds the core's folded tokens joined by spaces, never the raw text, so FTS5 only splits
//! on spaces and the index agrees with `dory_core::search` on every word. Every write that changes
//! searchable text, a translation included, calls [`reindex`] inside its own transaction, so the
//! index commits with the text. Queries quote every token, so a typed "or" or "near" is a word, never FTS syntax.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use dory_core::domain::{Fields, RecordingId};
use dory_core::ports::StoreError;
use dory_core::search::{Clause, Query, TOKENS_VERSION, Vocabulary, tokens};
use rusqlite::{Connection, OptionalExtension, params};

use super::{codec, db};

/// Appends the folded words of `text` to `out`, each followed by a space.
fn push_words(out: &mut String, text: &str) {
    for token in tokens(text) {
        out.push_str(&token.folded);
        out.push(' ');
    }
}

/// The recording's folded words per index column: title, speakers, summary, transcript. Speaker
/// names count only when they were given for the current transcript revision. The summary column
/// holds what was written, then its translation, which always describes the stored summary. The
/// transcript column holds what was spoken, then the translated segments of a translation that
/// applies. So a grant that reads a text reads its translation too.
fn row_words(conn: &Connection, id: &str) -> Result<[String; 4], StoreError> {
    let (title, names_revision) = conn
        .prepare_cached("SELECT title, names_revision FROM recordings WHERE id = ?1")
        .map_err(db)?
        .query_row([id], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(db)?;
    let transcript_revision: Option<i64> = conn
        .prepare_cached("SELECT revision FROM transcripts WHERE recording_id = ?1")
        .map_err(db)?
        .query_row([id], |row| row.get(0))
        .optional()
        .map_err(db)?;

    let mut title_words = String::new();
    push_words(&mut title_words, title.as_deref().unwrap_or_default());

    let mut speakers = String::new();
    if transcript_revision == Some(names_revision) {
        let mut stmt = conn
            .prepare_cached(
                "SELECT name FROM speaker_names WHERE recording_id = ?1 ORDER BY speaker",
            )
            .map_err(db)?;
        let mut rows = stmt.query([id]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            push_words(&mut speakers, &row.get::<_, String>(0).map_err(db)?);
        }
    }

    let mut summary = String::new();
    for table in ["summaries", "summary_translations"] {
        if let Some(markdown) = conn
            .prepare_cached(&format!(
                "SELECT markdown FROM {table} WHERE recording_id = ?1"
            ))
            .map_err(db)?
            .query_row([id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(db)?
        {
            push_words(&mut summary, &markdown);
        }
    }

    let mut transcript = String::new();
    let mut segments = 0;
    {
        let mut stmt = conn
            .prepare_cached("SELECT text FROM segments WHERE recording_id = ?1 ORDER BY idx")
            .map_err(db)?;
        let mut rows = stmt.query([id]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            push_words(&mut transcript, &row.get::<_, String>(0).map_err(db)?);
            segments += 1;
        }
    }
    if let Some(texts) = translated_texts(conn, id, transcript_revision, segments)? {
        for text in texts.iter().flatten() {
            push_words(&mut transcript, text);
        }
    }
    Ok([title_words, speakers, summary, transcript])
}

/// The texts of the recording's translation when it applies to the stored transcript, as
/// `Translation::applies_to` decides: made from this revision, one text per segment.
fn translated_texts(
    conn: &Connection,
    id: &str,
    revision: Option<i64>,
    segments: usize,
) -> Result<Option<Vec<Option<String>>>, StoreError> {
    let Some(revision) = revision else {
        return Ok(None);
    };
    let Some(texts) = conn
        .prepare_cached(
            "SELECT texts FROM translations WHERE recording_id = ?1 AND transcript_revision = ?2",
        )
        .map_err(db)?
        .query_row(params![id, revision], |row| row.get::<_, String>(0))
        .optional()
        .map_err(db)?
    else {
        return Ok(None);
    };
    let texts = codec::translation_texts_from_db(&texts)?;
    Ok((texts.len() == segments).then_some(texts))
}

/// Writes the index row of one recording, whose key exists and whose old row is gone.
fn write_row(tx: &Connection, key: i64, id: &str) -> Result<(), StoreError> {
    let [title, speakers, summary, transcript] = row_words(tx, id)?;
    tx.prepare_cached(
        "INSERT INTO search (rowid, title, speakers, summary, transcript)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .map_err(db)?
    .execute(params![key, title, speakers, summary, transcript])
    .map_err(db)?;
    Ok(())
}

fn bump_generation(tx: &Connection) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE search_meta SET generation = generation + 1")
        .map_err(db)?
        .execute([])
        .map_err(db)?;
    Ok(())
}

/// Indexes the recording's current title, speaker names, summary, and transcript again, in the
/// caller's transaction. The recording must exist.
pub(super) fn reindex(tx: &Connection, id: &RecordingId) -> Result<(), StoreError> {
    tx.prepare_cached("INSERT OR IGNORE INTO search_keys (recording_id) VALUES (?1)")
        .map_err(db)?
        .execute([id.as_str()])
        .map_err(db)?;
    let key: i64 = tx
        .prepare_cached("SELECT key FROM search_keys WHERE recording_id = ?1")
        .map_err(db)?
        .query_row([id.as_str()], |row| row.get(0))
        .map_err(db)?;
    tx.prepare_cached("DELETE FROM search WHERE rowid = ?1")
        .map_err(db)?
        .execute([key])
        .map_err(db)?;
    write_row(tx, key, id.as_str())?;
    bump_generation(tx)
}

/// Drops the recording's index row. Runs before the recording row goes, which takes its key with
/// it.
pub(super) fn forget(tx: &Connection, id: &RecordingId) -> Result<(), StoreError> {
    tx.prepare_cached(
        "DELETE FROM search WHERE rowid = (SELECT key FROM search_keys WHERE recording_id = ?1)",
    )
    .map_err(db)?
    .execute([id.as_str()])
    .map_err(db)?;
    bump_generation(tx)
}

/// Indexes every recording from scratch with this build's tokens, in the caller's transaction.
/// The v12 migration runs it, and so does an open that finds an index built by another
/// [`TOKENS_VERSION`]. A crash before the commit leaves the old state, so the next open runs it
/// again.
pub(super) fn rebuild(tx: &Connection) -> Result<(), StoreError> {
    let started = Instant::now();
    tx.execute_batch(
        "INSERT INTO search (search) VALUES ('delete-all');
         INSERT OR IGNORE INTO search_keys (recording_id) SELECT id FROM recordings;",
    )
    .map_err(db)?;
    let keys = tx
        .prepare("SELECT key, recording_id FROM search_keys ORDER BY key")
        .map_err(db)?
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    for (key, id) in &keys {
        write_row(tx, *key, id)?;
    }
    tx.prepare_cached("UPDATE search_meta SET tokens_version = ?1, generation = generation + 1")
        .map_err(db)?
        .execute([TOKENS_VERSION])
        .map_err(db)?;
    eprintln!(
        "search-backfill ms={:.0} recordings={}",
        started.elapsed().as_secs_f64() * 1000.0,
        keys.len()
    );
    Ok(())
}

/// Gives the reader connection two temporary views of the index's terms: one row per term, and
/// one per term and column with the number of recordings that hold it there. A temp table lives
/// in the connection, so the reader stays read-only and the schema gains nothing.
pub(super) fn open_vocabulary_view(reader: &Connection) -> Result<(), StoreError> {
    reader
        .execute_batch(
            "CREATE VIRTUAL TABLE temp.search_terms USING fts5vocab('main', 'search', 'row');
             CREATE VIRTUAL TABLE temp.search_columns USING fts5vocab('main', 'search', 'col');",
        )
        .map_err(db)
}

/// The index columns in [`row_words`] order, each with whether `fields` reads it.
fn columns(fields: Fields) -> [(&'static str, bool); 4] {
    [
        ("title", fields.title),
        ("speakers", fields.speakers),
        ("summary", fields.summary),
        ("transcript", fields.transcript),
    ]
}

/// The terms an assistant with `fields` could find: those some recording not hidden from
/// assistants holds in one of those columns. The index counts, per term and column, the
/// recordings that hold it, so a term is kept unless every one of them is hidden. Hidden
/// recordings are few, so their words are read from their text rather than from the index.
pub(super) fn read_scoped_vocabulary(
    conn: &Connection,
    fields: Fields,
) -> Result<Vocabulary, StoreError> {
    let started = Instant::now();
    let granted: Vec<&str> = columns(fields)
        .into_iter()
        .filter_map(|(name, on)| on.then_some(name))
        .collect();
    let mut hidden: HashMap<(String, &str), i64> = HashMap::new();
    let ids = conn
        .prepare_cached(
            "SELECT r.id FROM recordings r JOIN search_keys k ON k.recording_id = r.id
             WHERE r.hidden = 1",
        )
        .map_err(db)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    for id in &ids {
        for ((name, on), words) in columns(fields).into_iter().zip(row_words(conn, id)?) {
            if !on {
                continue;
            }
            let words: HashSet<&str> = words.split_whitespace().collect();
            for word in words {
                *hidden.entry((word.to_owned(), name)).or_default() += 1;
            }
        }
    }
    let mut terms = Vec::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT term, col, doc FROM temp.search_columns")
            .map_err(db)?;
        let mut rows = stmt.query([]).map_err(db)?;
        while let Some(row) = rows.next().map_err(db)? {
            let col: String = row.get(1).map_err(db)?;
            let Some(&name) = granted.iter().find(|name| **name == col) else {
                continue;
            };
            let term: String = row.get(0).map_err(db)?;
            let doc: i64 = row.get(2).map_err(db)?;
            if doc > hidden.get(&(term.clone(), name)).copied().unwrap_or(0) {
                terms.push(term);
            }
        }
    }
    let vocabulary = Vocabulary::new(generation(conn)?, terms);
    eprintln!(
        "assistant-vocabulary ms={:.2} terms={} hidden={}",
        started.elapsed().as_secs_f64() * 1000.0,
        vocabulary.terms().len(),
        ids.len()
    );
    Ok(vocabulary)
}

/// The index's write counter. It goes up with every index write, so a cache built from the index
/// is stale when this differs.
pub(super) fn generation(conn: &Connection) -> Result<u64, StoreError> {
    let generation: i64 = conn
        .query_row("SELECT generation FROM search_meta", [], |row| row.get(0))
        .map_err(db)?;
    Ok(u64::try_from(generation).unwrap_or_default())
}

/// Every term of the index, read in the caller's snapshot together with `generation`.
pub(super) fn read_vocabulary(
    conn: &Connection,
    generation: u64,
) -> Result<Vocabulary, StoreError> {
    let started = Instant::now();
    let terms = conn
        .prepare_cached("SELECT term FROM temp.search_terms")
        .map_err(db)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    let vocabulary = Vocabulary::new(generation, terms);
    eprintln!(
        "vocabulary ms={:.2} terms={}",
        started.elapsed().as_secs_f64() * 1000.0,
        vocabulary.terms().len()
    );
    Ok(vocabulary)
}

/// Whether the index was built by this build's tokens.
pub(super) fn is_current(conn: &Connection) -> Result<bool, StoreError> {
    let version: u32 = conn
        .query_row("SELECT tokens_version FROM search_meta", [], |row| {
            row.get(0)
        })
        .map_err(db)?;
    Ok(version == TOKENS_VERSION)
}

fn quoted(words: &[String]) -> String {
    format!("\"{}\"", words.join(" "))
}

/// The FTS5 `MATCH` text for `query`: a word is `("tok"*)`, a phrase is `"tok1 tok2"`, and a
/// phrase with a prefix last word is `"tok1 tok2" *`, joined with AND. Tokens hold only letters and digits, so no token can close its quotes.
pub(super) fn match_text(query: &Query) -> String {
    query
        .clauses()
        .iter()
        .map(|clause| match clause {
            Clause::Word { alts, prefix } => {
                let star = if *prefix { "*" } else { "" };
                let alts: Vec<String> = alts
                    .iter()
                    .map(|alt| format!("{}{star}", quoted(std::slice::from_ref(alt))))
                    .collect();
                format!("({})", alts.join(" OR "))
            }
            Clause::Phrase { words, prefix } => match prefix {
                true => format!("{} *", quoted(words)),
                false => quoted(words),
            },
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The same lookup restricted to the title.
pub(super) fn title_match_text(query: &Query) -> String {
    format!("{{title}} : ({})", match_text(query))
}

/// The same lookup restricted to the columns `fields` reads, or `None` when it reads none. Every
/// clause may match in any of them, as in the unrestricted lookup.
pub(super) fn scoped_match_text(query: &Query, fields: Fields) -> Option<String> {
    let granted: Vec<&str> = columns(fields)
        .into_iter()
        .filter_map(|(name, on)| on.then_some(name))
        .collect();
    (!granted.is_empty()).then(|| format!("{{{}}} : ({})", granted.join(" "), match_text(query)))
}

/// Refuses a lookup on an index another build's tokens wrote. The owner rebuilds it when it opens
/// the library; a read-only reader cannot, and its words would disagree with the index's.
pub(super) fn ready(conn: &Connection) -> Result<(), StoreError> {
    match is_current(conn)? {
        true => Ok(()),
        false => Err(StoreError::SearchIndexStale),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_is_quoted_and_words_are_prefixes() {
        let cases = [
            ("budget", "(\"budget\"*)"),
            ("Budget REVIEW", "(\"budget\"*) AND (\"review\"*)"),
            ("\"budget review\" or", "\"budget review\" AND (\"or\"*)"),
            ("near(x", "\"near x\" *"),
            ("follow-u \"follow u\"", "\"follow u\" * AND \"follow u\""),
            ("{title}: *", "(\"title\"*)"),
        ];
        for (text, want) in cases {
            assert_eq!(match_text(&Query::parse(text).unwrap()), want, "{text}");
        }
        assert_eq!(
            title_match_text(&Query::parse("a b").unwrap()),
            "{title} : ((\"a\"*) AND (\"b\"*))"
        );
    }
}
