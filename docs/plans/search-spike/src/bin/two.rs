use rusqlite::{params, Connection};
use std::time::Instant;
use unicode_normalization::UnicodeNormalization;

fn fold(s: &str) -> String {
    s.nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).flat_map(char::to_lowercase).collect()
}

fn osa_within(a: &[char], b: &[char], k: usize) -> bool {
    if a.len().abs_diff(b.len()) > k { return false; }
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n { d[i][0] = i; }
    for j in 0..=m { d[0][j] = j; }
    for i in 1..=n {
        let mut best = usize::MAX;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] { v = v.min(d[i - 2][j - 2] + 1); }
            d[i][j] = v;
            best = best.min(v);
        }
        if best > k { return false; }
    }
    d[n][m] <= k
}

fn time<T>(label: &str, runs: usize, mut f: impl FnMut() -> T) -> T {
    let mut v = Vec::new();
    let mut out = None;
    for _ in 0..runs { let t = Instant::now(); out = Some(f()); v.push(t.elapsed().as_secs_f64() * 1000.0); }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{label:<62} median {:>8.2} ms  min {:>8.2}  max {:>8.2}", v[v.len() / 2], v[0], v[v.len() - 1]);
    out.unwrap()
}

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut c = Connection::open(format!("{dir}/spike.sqlite")).unwrap();
    println!("-- contentless index (content='', contentless_delete=1) --");
    time("contentless build (backfill all)", 1, || c.execute_batch("DROP TABLE IF EXISTS fts_vocab; DROP TABLE IF EXISTS cl; CREATE VIRTUAL TABLE cl USING fts5(title, transcript, summary, speakers, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2', prefix='2 3'); INSERT INTO cl(rowid, title, transcript, summary, speakers) SELECT rowid, title, transcript, summary, '' FROM recordings;").unwrap());
    let sz: f64 = c.query_row("SELECT sum(pgsize)/1e6 FROM dbstat WHERE name LIKE 'cl%'", [], |r| r.get(0)).unwrap();
    println!("contentless size: {sz:.1} MB");
    for q in ["budget*", "muller*", "roadmap* budget*", "a*", "a", "{title}: budget*", "{transcript summary speakers}: budget*", "\"stubbornly muller\""] {
        let hits = time(&format!("cl ids+bm25 '{q}'"), 7, || {
            let mut s = c.prepare_cached("SELECT r.id FROM cl f JOIN recordings r ON r.rowid = f.rowid WHERE cl MATCH ?1 ORDER BY bm25(cl, 10.0, 1.0, 2.0, 1.0), r.created_at DESC").unwrap();
            s.query_map([q], |r| r.get::<_, String>(0)).unwrap().count()
        });
        println!("    hits {hits}");
    }
    time("cl replace one 2h transcript (delete+insert)", 7, || {
        let tx = c.transaction().unwrap();
        tx.execute_batch("DELETE FROM cl WHERE rowid = 7; INSERT INTO cl(rowid, title, transcript, summary, speakers) SELECT rowid, title, transcript, summary, '' FROM recordings WHERE rowid = 7;").unwrap();
        tx.commit().unwrap();
    });
    let after: i64 = c.query_row("SELECT count(*) FROM cl WHERE cl MATCH 'muller*'", [], |r| r.get(0)).unwrap();
    println!("    muller* hits after 7 replaces: {after} (500 expected)");

    println!("\n-- fuzzy with OSA distance over vocabulary --");
    c.execute_batch("CREATE VIRTUAL TABLE cl_vocab USING fts5vocab(cl, 'row');").unwrap();
    let terms: Vec<String> = time("read fts5vocab row terms (contentless)", 5, || {
        let mut s = c.prepare_cached("SELECT term FROM cl_vocab").unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    });
    println!("    distinct terms: {}", terms.len());
    time("prefix probe: does any term start with 'roadmpa'", 7, || c.query_row("SELECT count(*) FROM (SELECT 1 FROM cl WHERE cl MATCH 'roadmpa*' LIMIT 1)", [], |r| r.get::<_, i64>(0)).unwrap());
    let chars: Vec<Vec<char>> = terms.iter().map(|t| t.chars().collect()).collect();
    for (q, k) in [("roadmpa", 1usize), ("budgte", 1), ("mueller", 1), ("ångstrom", 1), ("recieveing", 2), ("stubornly", 1), ("antinatoinal", 2)] {
        let qc: Vec<char> = fold(q).chars().collect();
        let near = time(&format!("OSA <= {k} over vocab '{q}'"), 7, || {
            chars.iter().zip(&terms).filter(|(t, _)| osa_within(&qc, t, k)).map(|(_, s)| s.clone()).collect::<Vec<_>>()
        });
        println!("    near: {:?}", &near[..near.len().min(8)]);
    }

    println!("\n-- core snippet and jump target from stored segments --");
    c.execute_batch("DROP TABLE IF EXISTS segments; CREATE TABLE segments (recording_id TEXT, idx INTEGER, text TEXT, PRIMARY KEY (recording_id, idx));").unwrap();
    {
        let tx = c.transaction().unwrap();
        let mut rows = tx.prepare("SELECT id, transcript FROM recordings").unwrap();
        let all: Vec<(String, String)> = rows.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        drop(rows);
        for (id, tr) in &all {
            for (i, line) in tr.lines().enumerate() {
                tx.execute("INSERT INTO segments VALUES (?1, ?2, ?3)", params![id, i as i64, line]).unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let ids: Vec<String> = { let mut s = c.prepare("SELECT r.id FROM cl f JOIN recordings r ON r.rowid = f.rowid WHERE cl MATCH 'muller*' ORDER BY bm25(cl) LIMIT 15").unwrap(); s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() };
    for (label, needle) in [("rare term 'muller'", "muller"), ("common prefix 'a'", "a"), ("absent 'zzzz'", "zzzz")] {
        time(&format!("15 visible rows: load segments + first match, {label}"), 7, || {
            let mut found = 0;
            for id in &ids {
                let mut s = c.prepare_cached("SELECT idx, text FROM segments WHERE recording_id = ?1 ORDER BY idx").unwrap();
                let segs: Vec<(i64, String)> = s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
                if segs.iter().any(|(_, t)| t.split(|ch: char| !ch.is_alphanumeric()).any(|w| fold(w).starts_with(needle))) { found += 1; }
            }
            found
        });
    }
    let segs: i64 = c.query_row("SELECT count(*) FROM segments WHERE recording_id = 'r0007'", [], |r| r.get(0)).unwrap();
    println!("    segments per recording: {segs}");
}
