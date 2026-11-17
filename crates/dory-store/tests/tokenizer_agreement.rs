//! The index stores the core's folded tokens and FTS5 splits and folds them again, so the core's
//! words must be exactly what the bundled SQLite's unicode61 tokenizer reads back.

use std::collections::HashMap;

use dory_core::search::tokens;
use rusqlite::Connection;

/// The tokenizer options of `schema_v8.sql`.
const TOKENIZER: &str = "unicode61 remove_diacritics 0";

/// What SQLite reads from `core` joined by spaces, one list per code point, which must be `core`
/// itself.
fn sqlite_terms(conn: &Connection, docs: &[(u32, Vec<String>)]) -> Vec<(u32, Vec<String>)> {
    conn.execute_batch("INSERT INTO t (t) VALUES ('delete-all')")
        .unwrap();
    {
        let mut insert = conn
            .prepare("INSERT INTO t (rowid, x) VALUES (?1, ?2)")
            .unwrap();
        for (cp, words) in docs {
            insert.execute((cp, words.join(" "))).unwrap();
        }
    }
    let mut stmt = conn
        .prepare("SELECT doc, term, offset FROM t_instance")
        .unwrap();
    let mut read: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(2)?,
                row.get::<_, String>(1)?,
            ))
        })
        .unwrap();
    for row in rows {
        let (doc, offset, term) = row.unwrap();
        read.entry(doc).or_default().push((offset, term));
    }
    docs.iter()
        .map(|(cp, _)| {
            let mut terms = read.remove(cp).unwrap_or_default();
            terms.sort();
            (*cp, terms.into_iter().map(|(_, term)| term).collect())
        })
        .collect()
}

#[test]
fn every_word_the_core_makes_is_a_word_unicode61_keeps_whole() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE t USING fts5(x, content = '', tokenize = '{TOKENIZER}');
         CREATE VIRTUAL TABLE t_instance USING fts5vocab(t, instance);"
    ))
    .unwrap();
    let contexts: [fn(char) -> String; 2] = [|c| c.to_string(), |c| format!("a{c}b")];
    let mut bad = Vec::new();
    for context in contexts {
        let mut docs = Vec::new();
        for cp in 0..=0x10FFFFu32 {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let words: Vec<String> = tokens(&context(c)).map(|t| t.folded).collect();
            if !words.is_empty() {
                docs.push((cp, words));
            }
            if docs.len() == 50_000 {
                bad.extend(disagreements(&conn, std::mem::take(&mut docs)));
            }
        }
        bad.extend(disagreements(&conn, docs));
    }
    assert!(
        bad.is_empty(),
        "{} code points the core and unicode61 read differently: {:?}",
        bad.len(),
        bad.iter()
            .map(|(cp, _)| cp.as_str())
            .collect::<std::collections::BTreeSet<_>>()
    );
}

fn disagreements(conn: &Connection, docs: Vec<(u32, Vec<String>)>) -> Vec<(String, Vec<String>)> {
    sqlite_terms(conn, &docs)
        .into_iter()
        .zip(docs)
        .filter(|(read, wrote)| read.1 != wrote.1)
        .map(|(read, wrote)| (format!("U+{:04X}", wrote.0), read.1))
        .collect()
}
