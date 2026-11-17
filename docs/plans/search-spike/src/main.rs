use rusqlite::{params, Connection};
use std::time::Instant;
use unicode_normalization::UnicodeNormalization;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn f(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 }
}

fn fold(s: &str) -> String {
    s.nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).flat_map(char::to_lowercase).collect()
}

fn tokens(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(fold)
}

fn lev_within(a: &[char], b: &[char], k: usize) -> bool {
    if a.len().abs_diff(b.len()) > k { return false; }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut best = cur[0];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            best = best.min(cur[j]);
        }
        if best > k { return false; }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()] <= k
}

fn time<T>(label: &str, runs: usize, mut f: impl FnMut() -> T) -> T {
    let mut v = Vec::new();
    let mut out = None;
    for _ in 0..runs { let t = Instant::now(); out = Some(f()); v.push(t.elapsed().as_secs_f64() * 1000.0); }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{label:<58} median {:>8.2} ms  min {:>8.2}  max {:>8.2}", v[v.len() / 2], v[0], v[v.len() - 1]);
    out.unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[1];
    let n: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(500);
    let words_per: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(18_000);
    let dict = std::fs::read_to_string("/usr/share/dict/words").unwrap();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut vocab: Vec<&str> = dict.lines().filter(|w| (2..=12).contains(&w.len())).collect();
    for i in (1..vocab.len()).rev() { let j = (rng.next() % (i as u64 + 1)) as usize; vocab.swap(i, j); }
    vocab.truncate(40_000);
    for (i, w) in ["Müller", "José", "café", "naïve", "Zoë", "roadmap", "budget", "Ångström", "résumé"].iter().enumerate() { vocab[200 + i * 300] = w; }
    let h: Vec<f64> = (1..=vocab.len()).map(|r| 1.0 / r as f64).scan(0.0, |s, x| { *s += x; Some(*s) }).collect();
    let total = *h.last().unwrap();
    let mut pick = |rng: &mut Rng| { let x = rng.f() * total; vocab[h.partition_point(|&c| c < x).min(vocab.len() - 1)] };

    let path = format!("{dir}/spike.sqlite");
    let _ = std::fs::remove_file(&path);
    let mut c = Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
        CREATE TABLE recordings (id TEXT PRIMARY KEY, created_at INTEGER, title TEXT, title_lc TEXT, transcript_lc TEXT, summary_lc TEXT, transcript TEXT, summary TEXT);").unwrap();
    let t = Instant::now();
    {
        let tx = c.transaction().unwrap();
        for i in 0..n {
            let title = format!("{} {} sync {i}", pick(&mut rng), pick(&mut rng));
            let mut tr = String::with_capacity(words_per * 8);
            for w in 0..words_per { tr.push_str(pick(&mut rng)); tr.push(if w % 15 == 14 { '\n' } else { ' ' }); }
            let mut su = String::new();
            for w in 0..300 { su.push_str(pick(&mut rng)); su.push(if w % 20 == 19 { '\n' } else { ' ' }); }
            tx.execute("INSERT INTO recordings VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", params![format!("r{i:04}"), i as i64, title, title.to_lowercase(), tr.to_lowercase(), su.to_lowercase(), tr, su]).unwrap();
        }
        tx.commit().unwrap();
    }
    let text_mb: f64 = c.query_row("SELECT sum(length(transcript))/1e6 FROM recordings", [], |r| r.get(0)).unwrap();
    println!("corpus: {n} recordings x {words_per} words, transcript text {text_mb:.1} MB, generated in {:.1}s", t.elapsed().as_secs_f64());
    c.execute_batch("VACUUM").unwrap();
    let base_size = std::fs::metadata(&path).unwrap().len() as f64 / 1e6;
    println!("db with raw + _lc columns: {base_size:.1} MB");

    println!("\n-- current: instr over _lc concat --");
    for q in ["budget", "zzzz-no-hit", "müller"] {
        time(&format!("instr '{q}'"), 7, || {
            let mut s = c.prepare_cached("SELECT id FROM recordings r WHERE ?1 = '' OR instr(r.title_lc || char(10) || r.transcript_lc || char(10) || r.summary_lc, ?1) > 0 ORDER BY created_at DESC").unwrap();
            s.query_map([q], |r| r.get::<_, String>(0)).unwrap().count()
        });
    }

    for (name, opts) in [("fts", "tokenize='unicode61 remove_diacritics 2', prefix='2 3'"), ("fts_noprefix", "tokenize='unicode61 remove_diacritics 2'")] {
        println!("\n-- {name}: {opts} --");
        time(&format!("{name} build (backfill all)"), 1, || {
            c.execute_batch(&format!("DROP TABLE IF EXISTS {name}; CREATE VIRTUAL TABLE {name} USING fts5(title, transcript, summary, speakers, {opts});
                INSERT INTO {name}(rowid, title, transcript, summary, speakers) SELECT rowid, title, transcript, summary, '' FROM recordings;")).unwrap();
        });
        let sz: f64 = c.query_row(&format!("SELECT sum(pgsize)/1e6 FROM dbstat WHERE name LIKE '{name}%'"), [], |r| r.get(0)).unwrap();
        println!("{name} size incl. its content copy: {sz:.1} MB");
        let idx: f64 = c.query_row(&format!("SELECT sum(pgsize)/1e6 FROM dbstat WHERE name = '{name}_data'"), [], |r| r.get(0)).unwrap();
        println!("{name} inverted index alone: {idx:.1} MB");
        let queries = ["budget*", "müller*", "muller*", "roadmap* budget*", "\"roadmap budget\"", "a*", "ro*", "zzzz*", "{title}: budget*"];
        for q in queries {
            let hits = time(&format!("{name} ids+bm25 '{q}'"), 7, || {
                let mut s = c.prepare_cached(&format!("SELECT r.id FROM {name} f JOIN recordings r ON r.rowid = f.rowid WHERE {name} MATCH ?1 ORDER BY bm25({name}, 10.0, 1.0, 2.0, 1.0)")).unwrap();
                s.query_map([q], |r| r.get::<_, String>(0)).unwrap().count()
            });
            println!("    hits {hits}");
        }
        for q in ["budget*", "a*"] {
            time(&format!("{name} ids+bm25+snippet ALL hits '{q}'"), 3, || {
                let mut s = c.prepare_cached(&format!("SELECT r.id, snippet({name}, 1, char(2), char(3), '…', 10) FROM {name} f JOIN recordings r ON r.rowid = f.rowid WHERE {name} MATCH ?1 ORDER BY bm25({name}, 10.0, 1.0, 2.0, 1.0)")).unwrap();
                s.query_map([q], |r| r.get::<_, String>(1)).unwrap().count()
            });
            time(&format!("{name} ids+bm25+snippet top 40 '{q}'"), 5, || {
                let mut s = c.prepare_cached(&format!("SELECT r.id, snippet({name}, 1, char(2), char(3), '…', 10) FROM {name} f JOIN recordings r ON r.rowid = f.rowid WHERE {name} MATCH ?1 ORDER BY bm25({name}, 10.0, 1.0, 2.0, 1.0) LIMIT 40")).unwrap();
                s.query_map([q], |r| r.get::<_, String>(1)).unwrap().count()
            });
        }
        let snip: String = c.query_row(&format!("SELECT snippet({name}, 1, '[', ']', '…', 10) FROM {name} WHERE {name} MATCH 'muller*' LIMIT 1"), [], |r| r.get(0)).unwrap();
        println!("    snippet sample: {snip}");
        time(&format!("{name} replace one 2h transcript (delete+insert)"), 5, || {
            let tx = c.transaction().unwrap();
            tx.execute_batch(&format!("DELETE FROM {name} WHERE rowid = 7; INSERT INTO {name}(rowid, title, transcript, summary, speakers) SELECT rowid, title, transcript, summary, '' FROM recordings WHERE rowid = 7;")).unwrap();
            tx.commit().unwrap();
        });
    }
    c.execute_batch("DROP TABLE fts_noprefix").unwrap();

    println!("\n-- fuzzy over the vocabulary --");
    c.execute_batch("CREATE VIRTUAL TABLE fts_vocab USING fts5vocab(fts, 'row');").unwrap();
    let terms: Vec<String> = time("read fts5vocab row terms", 5, || {
        let mut s = c.prepare_cached("SELECT term FROM fts_vocab").unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    });
    println!("    distinct terms: {}", terms.len());
    let chars: Vec<Vec<char>> = terms.iter().map(|t| t.chars().collect()).collect();
    for (q, k) in [("roadmpa", 1usize), ("budgte", 1), ("mueller", 1), ("ångstrom", 1), ("recieveing", 2)] {
        let qc: Vec<char> = fold(q).chars().collect();
        let near = time(&format!("edit distance <= {k} over vocab '{q}'"), 7, || {
            chars.iter().zip(&terms).filter(|(t, _)| lev_within(&qc, t, k)).map(|(_, s)| s.clone()).collect::<Vec<_>>()
        });
        println!("    near: {:?}", &near[..near.len().min(8)]);
    }

    println!("\n-- trigram alternative --");
    time("trigram build", 1, || c.execute_batch("CREATE VIRTUAL TABLE tri USING fts5(title, transcript, summary, tokenize='trigram remove_diacritics 1'); INSERT INTO tri(rowid, title, transcript, summary) SELECT rowid, title, transcript, summary FROM recordings;").unwrap());
    let sz: f64 = c.query_row("SELECT sum(pgsize)/1e6 FROM dbstat WHERE name LIKE 'tri%'", [], |r| r.get(0)).unwrap();
    println!("trigram size: {sz:.1} MB");
    for q in ["budget", "muller", "oadma"] {
        let hits = time(&format!("trigram '{q}'"), 7, || {
            let mut s = c.prepare_cached("SELECT rowid FROM tri WHERE tri MATCH ?1").unwrap();
            s.query_map([q], |r| r.get::<_, i64>(0)).unwrap().count()
        });
        println!("    hits {hits}");
    }
    c.execute_batch("DROP TABLE tri").unwrap();

    println!("\n-- in-core matcher alternative --");
    let all: Vec<(String, String, String)> = time("load all raw text from store", 3, || {
        let mut s = c.prepare_cached("SELECT title, transcript, summary FROM recordings").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    });
    time("core tokenize+fold+match 'budget' over all text", 3, || {
        all.iter().filter(|(a, b, c)| tokens(a).chain(tokens(b)).chain(tokens(c)).any(|t| t.starts_with("budget"))).count()
    });
    let one = &all[7].1;
    let hits = time("core tokenize+fold one 2h transcript, find matches", 21, || tokens(one).filter(|t| t.starts_with("budget")).count());
    println!("    matches in one transcript: {hits}");
    c.execute_batch("VACUUM").unwrap();
    println!("\nfinal db size with fts (raw+_lc+fts): {:.1} MB", std::fs::metadata(&path).unwrap().len() as f64 / 1e6);
}
