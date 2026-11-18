use dory_core::domain::{
    Channel, ChannelLayout, Goal, JobEvent, JobState, LangTag, LanguagePref, Ms, Priority,
    Recording, RecordingId, RecordingQuery, Settings, Stage, SummaryTranslation, TimeRange,
    Timestamp, Translation,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Checkpoint, CheckpointKey, Store, StoreError, StoreRead};
use dory_core::search::{Query, TOKENS_VERSION};
use dory_core::testing::store::{CASES, MemStore, run_store_contract};
use dory_store::SqliteStore;

fn open(dir: &tempfile::TempDir) -> SqliteStore {
    SqliteStore::open(dir.path().join("dory.db")).unwrap()
}

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

fn recording(id: &str, created: i64) -> Recording {
    Recording {
        id: rid(id),
        title: None,
        created_at: Timestamp(created),
        language: LanguagePref::Auto,
        layout: ChannelLayout::MicSystem,
        take: None,
    }
}

#[test]
fn mem_store_passes_the_store_contract() {
    run_store_contract(|| Box::new(MemStore::new()));
}

#[test]
fn sqlite_store_passes_the_store_contract() {
    let dirs = std::sync::Mutex::new(Vec::new());
    run_store_contract(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir);
        dirs.lock().unwrap().push(dir);
        Box::new(store)
    });
}

#[test]
fn every_contract_case_runs_on_sqlite_by_name() {
    for (name, case) in CASES {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(&store)));
        assert!(result.is_ok(), "contract case {name} failed on SqliteStore");
    }
}

/// The schema version this build migrates to.
const LATEST: u32 = 10;

#[test]
fn state_survives_closing_and_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = open(&dir);
        store
            .apply(vec![
                Change::RecordingCreated(recording("r", 10)),
                Change::JobEnqueued {
                    id: rid("r"),
                    goal: Goal::ALL,
                    priority: Priority::Front,
                    at: Timestamp(5),
                },
            ])
            .unwrap();
        let job = store.jobs_for(&rid("r")).unwrap().remove(0);
        store
            .apply(vec![
                Change::Job {
                    job: job.id,
                    event: JobEvent::Started {
                        stage: Stage::Transcribe,
                    },
                },
                Change::Job {
                    job: job.id,
                    event: JobEvent::RunFast,
                },
            ])
            .unwrap();
    }
    let store = open(&dir);
    assert_eq!(
        store.recording(&rid("r")).unwrap(),
        Some(recording("r", 10))
    );
    let job = store.jobs_for(&rid("r")).unwrap().remove(0);
    assert_eq!(
        (job.state, job.priority, job.attempts, job.run_fast),
        (
            JobState::Running {
                stage: Stage::Transcribe
            },
            Priority::Front,
            1,
            true
        )
    );
}

#[test]
fn a_new_database_is_wal_and_at_the_latest_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    assert_eq!(store.schema_version().unwrap(), LATEST);
    let conn = rusqlite::Connection::open(dir.path().join("dory.db")).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn reopening_does_not_rerun_migrations_or_lose_rows() {
    let dir = tempfile::tempdir().unwrap();
    open(&dir)
        .apply(vec![Change::RecordingCreated(recording("keep", 1))])
        .unwrap();
    for _ in 0..3 {
        let store = open(&dir);
        assert_eq!(store.schema_version().unwrap(), LATEST);
        assert_eq!(
            store.recordings(&RecordingQuery::default()).unwrap().len(),
            1
        );
    }
}

#[test]
fn a_database_from_a_newer_build_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    open(&dir)
        .apply(vec![Change::RecordingCreated(recording("keep", 1))])
        .unwrap();
    let path = dir.path().join("dory.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 99")
        .unwrap();
    match SqliteStore::open(&path) {
        Err(StoreError::SchemaTooNew {
            found: 99,
            supported: LATEST,
        }) => {}
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
    let version: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 99, "the refused database was not downgraded");
}

#[test]
fn a_reader_is_not_blocked_by_a_writer_holding_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(open(&dir));
    store
        .apply(vec![Change::RecordingCreated(recording("a", 1))])
        .unwrap();
    // A second connection holds a write transaction open, as another process would.
    let mut other = rusqlite::Connection::open(dir.path().join("dory.db")).unwrap();
    let tx = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "INSERT INTO recordings (id, created_at, layout) VALUES ('uncommitted', 2, 'mixed')",
        [],
    )
    .unwrap();
    let rows = store.recordings(&RecordingQuery::default()).unwrap();
    assert_eq!(rows.len(), 1, "readers see committed state only");
    drop(tx);
}

#[test]
fn a_version_1_database_migrates_to_checkpoints_and_keeps_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../src/sqlite/schema_v1.sql"))
            .unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 1;
             INSERT INTO recordings (id, created_at, layout) VALUES ('old', 7, 'mic_system');",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), LATEST);
    assert_eq!(
        store.recording(&rid("old")).unwrap(),
        Some(recording("old", 7))
    );
    let key = CheckpointKey {
        recording: rid("old"),
        model: ModelRef::parse("sona/whisper-small").unwrap(),
        channel: Channel::Mic,
        language: LanguagePref::Auto,
    };
    let checkpoint = Checkpoint {
        span: TimeRange::new(Ms(0), Ms(30_000)).unwrap(),
        segments: vec![],
        detected_language: None,
    };
    store
        .apply(vec![Change::CheckpointSaved {
            key: key.clone(),
            checkpoint: checkpoint.clone(),
        }])
        .unwrap();
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(reopened.checkpoints(&key).unwrap(), vec![checkpoint]);
}

#[test]
fn a_version_4_database_gives_each_job_its_enqueue_time_as_its_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../src/sqlite/schema_v1.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../src/sqlite/schema_v2.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../src/sqlite/schema_v3.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../src/sqlite/schema_v4.sql"))
            .unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 4;
             INSERT INTO recordings (id, created_at, layout) VALUES ('a', 1, 'mic_system');
             INSERT INTO recordings (id, created_at, layout) VALUES ('b', 1, 'mic_system');
             INSERT INTO jobs (recording_id, goal, done, state, priority, attempts, created_at)
                 VALUES ('a', 3, '', 'queued', 0, 0, 900);
             INSERT INTO jobs (recording_id, goal, done, state, priority, attempts, created_at)
                 VALUES ('b', 3, '', 'queued', 0, 0, 400);",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), LATEST);
    for (id, created) in [("a", 900), ("b", 400)] {
        let job = store.jobs_for(&rid(id)).unwrap().remove(0);
        assert_eq!(
            (job.place, job.created_at),
            (created, Timestamp(created)),
            "{id}"
        );
    }
    let next = store.next_runnable_job(Timestamp(1_000)).unwrap().unwrap();
    assert_eq!(next.recording, rid("b"), "the older job still runs first");
    let index: String = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'jobs_runnable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        index.contains("place"),
        "the worker index follows place: {index}"
    );
}

fn found(store: &SqliteStore, text: &str) -> Vec<String> {
    store
        .search(&Query::parse(text).unwrap())
        .unwrap()
        .into_iter()
        .map(|f| f.row.recording.id.to_string())
        .collect()
}

#[test]
fn v8_migrates_a_v7_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
            include_str!("../src/sqlite/schema_v1.sql"),
            include_str!("../src/sqlite/schema_v2.sql"),
            include_str!("../src/sqlite/schema_v3.sql"),
            include_str!("../src/sqlite/schema_v4.sql"),
            include_str!("../src/sqlite/schema_v5.sql"),
            include_str!("../src/sqlite/schema_v6.sql"),
            include_str!("../src/sqlite/schema_v7.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 7;
             INSERT INTO recordings (id, title, created_at, layout, names_revision, title_lc)
                 VALUES ('a', 'Atlas kickoff', 3, 'mic_system', 1, 'atlas kickoff'),
                        ('b', NULL, 2, 'mixed', 0, ''),
                        ('c', 'Weekly', 1, 'mixed', 0, 'weekly');
             INSERT INTO transcripts (recording_id, revision, languages, produced_by)
                 VALUES ('a', 1, 'en', 'sona/whisper-small'),
                        ('b', 1, '', 'sona/whisper-small');
             INSERT INTO segments (recording_id, idx, start_ms, end_ms, speaker, channel, text)
                 VALUES ('a', 0, 0, 900, 1, 'system', 'We climb Kilimanjaro'),
                        ('a', 1, 1000, 1900, 0, 'mic', 'after the review'),
                        ('b', 0, NULL, NULL, 0, 'mixed', 'Herr Müller kommt');
             INSERT INTO speaker_names (recording_id, speaker, name) VALUES ('a', 1, 'Priyanka');
             INSERT INTO summaries (recording_id, markdown, language, produced_by, transcript_revision)
                 VALUES ('c', '## Okavango delta', 'en', 'ollama/gemma4:e4b', 0);",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), LATEST);
    assert_eq!(
        std::fs::metadata(dir.path().join("dory.db-wal"))
            .unwrap()
            .len(),
        0,
        "the migration's write-ahead log is checkpointed and truncated"
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('recordings')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        columns.iter().all(|c| !c.ends_with("_lc")),
        "the _lc columns are gone: {columns:?}"
    );
    assert_eq!(found(&store, "atlas"), ["a"], "a title");
    assert_eq!(found(&store, "kilimanjaro review"), ["a"], "a transcript");
    assert_eq!(found(&store, "priyanka"), ["a"], "a speaker name");
    assert_eq!(found(&store, "muller"), ["b"], "folded accents");
    assert_eq!(found(&store, "okavango"), ["c"], "a summary");
    let keys: i64 = conn
        .query_row("SELECT count(*) FROM search_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(keys, 3, "one key per recording");
    let version: u32 = conn
        .query_row("SELECT tokens_version FROM search_meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, TOKENS_VERSION);
}

#[test]
fn tokens_version_bump_reindexes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    {
        let store = SqliteStore::open(&path).unwrap();
        store
            .apply(vec![
                Change::RecordingCreated(Recording {
                    title: Some("Roadmap".into()),
                    ..recording("r", 1)
                }),
                Change::RecordingCreated(recording("s", 2)),
            ])
            .unwrap();
        assert_eq!(found(&store, "roadmap"), ["r"]);
    }
    // What an index built by an older tokenizer looks like to this build: another version, and
    // rows this build would not have written.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "UPDATE search_meta SET tokens_version = 0;
             INSERT INTO search (search) VALUES ('delete-all');
             INSERT INTO search (rowid, title, speakers, summary, transcript)
                 SELECT key, 'stale', '', '', '' FROM search_keys;",
        )
        .unwrap();
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(found(&store, "roadmap"), ["r"], "rebuilt on open");
    assert_eq!(
        found(&store, "stale"),
        Vec::<String>::new(),
        "old rows are gone"
    );
    let version: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT tokens_version FROM search_meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, TOKENS_VERSION);
}

#[test]
fn renumbered_rowids_keep_each_recording_its_own_index_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    let store = SqliteStore::open(&path).unwrap();
    for (id, at, title) in [("a", 1, "alpha"), ("b", 2, "beta"), ("c", 3, "gamma")] {
        store
            .apply(vec![Change::RecordingCreated(Recording {
                title: Some(title.into()),
                ..recording(id, at)
            })])
            .unwrap();
    }
    store
        .apply(vec![Change::RecordingDeleted(rid("a"))])
        .unwrap();
    drop(store);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("VACUUM")
        .unwrap();
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(found(&store, "beta"), ["b"]);
    assert_eq!(found(&store, "gamma"), ["c"]);
    assert_eq!(found(&store, "alpha"), Vec::<String>::new());
}

fn titled(id: &str, at: i64, title: &str) -> Change {
    Change::RecordingCreated(Recording {
        title: Some(title.into()),
        ..recording(id, at)
    })
}

#[test]
fn vocabulary_is_cached_until_a_write_moves_the_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    let store = SqliteStore::open(&path).unwrap();
    store.apply(vec![titled("a", 1, "Roadmap")]).unwrap();
    let first = store.vocabulary().unwrap();
    assert_eq!(first.terms(), ["roadmap"]);
    assert!(
        std::sync::Arc::ptr_eq(&first, &store.vocabulary().unwrap()),
        "no write, so the cached copy is served"
    );

    store
        .apply(vec![titled("b", 2, "Zanzibar offsite")])
        .unwrap();
    let second = store.vocabulary().unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &second), "a write rebuilds");
    assert!(second.generation() > first.generation());
    assert_eq!(second.terms(), ["offsite", "roadmap", "zanzibar"]);

    let other = SqliteStore::open(&path).unwrap();
    other
        .apply(vec![Change::RecordingDeleted(rid("b"))])
        .unwrap();
    assert_eq!(
        store.vocabulary().unwrap().terms(),
        ["roadmap"],
        "a write through another connection invalidates this one's cache"
    );
}

#[test]
fn a_job_change_leaves_the_cached_vocabulary_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    store.apply(vec![titled("a", 1, "Roadmap")]).unwrap();
    let first = store.vocabulary().unwrap();
    store
        .apply(vec![Change::JobEnqueued {
            id: rid("a"),
            goal: Goal::ALL,
            priority: Priority::Normal,
            at: Timestamp(5),
        }])
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &store.vocabulary().unwrap()));
}

#[test]
fn a_version_7_database_gains_segment_languages_and_translations_and_keeps_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
            include_str!("../src/sqlite/schema_v1.sql"),
            include_str!("../src/sqlite/schema_v2.sql"),
            include_str!("../src/sqlite/schema_v3.sql"),
            include_str!("../src/sqlite/schema_v4.sql"),
            include_str!("../src/sqlite/schema_v5.sql"),
            include_str!("../src/sqlite/schema_v6.sql"),
            include_str!("../src/sqlite/schema_v7.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 7;
             INSERT INTO recordings (id, title, created_at, layout) VALUES ('old', 'Old', 1, 'mic_system');
             INSERT INTO transcripts (recording_id, revision, languages, produced_by)
                 VALUES ('old', 1, 'tl', 'sona/whisper-small');
             INSERT INTO segments (recording_id, idx, start_ms, end_ms, speaker, channel, text)
                 VALUES ('old', 0, 0, 900, 1, 'system', 'Kumusta');
             INSERT INTO jobs (recording_id, goal, done, state, priority, attempts, created_at)
                 VALUES ('old', 3, 'transcribe', 'queued', 0, 0, 5);",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), LATEST);
    assert_eq!(
        found(&store, "kumusta"),
        ["old"],
        "the upgrade went through search's v8 and indexed the old transcript"
    );

    let transcript = store.transcript(&rid("old")).unwrap().unwrap();
    assert_eq!(transcript.segments[0].text, "Kumusta");
    assert_eq!(
        transcript.segments[0].language, None,
        "unknown, so it gets translated"
    );
    let job = store.jobs_for(&rid("old")).unwrap().remove(0);
    assert_eq!(job.goal, Goal::ALL, "an old job has no translate stage");
    assert_eq!(store.translation(&rid("old")).unwrap(), None);
    assert_eq!(store.settings().unwrap().output_language, None);

    let english = LangTag::parse("en").unwrap();
    let translation = Translation {
        target: english.clone(),
        transcript_revision: transcript.revision,
        produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
        texts: vec![Some("Hello".into())],
    };
    store
        .apply(vec![
            Change::TranslationReplaced {
                id: rid("old"),
                translation: translation.clone(),
            },
            Change::SettingsReplaced(Settings {
                output_language: Some(english),
                ..Settings::default()
            }),
        ])
        .unwrap();
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(
        reopened.translation(&rid("old")).unwrap(),
        Some(translation)
    );
    assert!(reopened.settings().unwrap().output_language.is_some());
    assert_eq!(
        reopened.transcript(&rid("old")).unwrap().unwrap(),
        transcript,
        "the spoken transcript is untouched"
    );
}

#[test]
fn a_translate_job_survives_a_restart_at_the_translate_stage() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    store
        .apply(vec![Change::RecordingCreated(recording("r", 1))])
        .unwrap();
    let goal = Goal::ALL.translating(true);
    store
        .apply(vec![Change::JobEnqueued {
            id: rid("r"),
            goal,
            priority: Priority::Normal,
            at: Timestamp(5),
        }])
        .unwrap();
    let job = store.jobs_for(&rid("r")).unwrap().remove(0);
    for event in [
        JobEvent::Started {
            stage: Stage::Transcribe,
        },
        JobEvent::StageDone {
            stage: Stage::Transcribe,
        },
        JobEvent::Started {
            stage: Stage::Translate,
        },
    ] {
        store
            .apply(vec![Change::Job { job: job.id, event }])
            .unwrap();
    }
    drop(store);
    let job = open(&dir).jobs_for(&rid("r")).unwrap().remove(0);
    assert_eq!(job.goal, goal);
    assert!(job.goal.wants(Stage::Translate));
    assert_eq!(job.done, [Stage::Transcribe]);
    assert_eq!(
        job.state,
        JobState::Running {
            stage: Stage::Translate
        }
    );
}

#[test]
fn a_version_9_database_gains_summary_translations_and_keeps_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
            include_str!("../src/sqlite/schema_v1.sql"),
            include_str!("../src/sqlite/schema_v2.sql"),
            include_str!("../src/sqlite/schema_v3.sql"),
            include_str!("../src/sqlite/schema_v4.sql"),
            include_str!("../src/sqlite/schema_v5.sql"),
            include_str!("../src/sqlite/schema_v6.sql"),
            include_str!("../src/sqlite/schema_v7.sql"),
            include_str!("../src/sqlite/schema_v8.sql"),
            include_str!("../src/sqlite/schema_v9.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 9;
             INSERT INTO recordings (id, title, created_at, layout) VALUES ('old', 'Old', 1, 'mic_system');
             INSERT INTO summaries (recording_id, markdown, language, produced_by, transcript_revision)
                 VALUES ('old', '## Mga punto', 'tl', 'ollama/gemma4:e4b', 1);
             INSERT INTO jobs (recording_id, goal, done, state, priority, attempts, created_at)
                 VALUES ('old', 7, 'transcribe', 'queued', 0, 0, 5);",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), LATEST);
    assert_eq!(
        store.summary(&rid("old")).unwrap().unwrap().markdown,
        "## Mga punto"
    );
    assert_eq!(store.summary_translation(&rid("old")).unwrap(), None);
    assert_eq!(store.summary_translation_wanted(&rid("old")).unwrap(), None);
    assert_eq!(
        store.jobs_for(&rid("old")).unwrap()[0].goal,
        Goal::ALL.translating(true),
        "an old job keeps its goal bits"
    );

    let english = LangTag::parse("en").unwrap();
    let translation = SummaryTranslation {
        target: english.clone(),
        produced_by: ModelRef::parse("ollama/gemma4:e4b").unwrap(),
        markdown: "## Points".into(),
    };
    store
        .apply(vec![
            Change::SummaryTranslationWanted {
                id: rid("old"),
                target: english,
            },
            Change::SummaryTranslated {
                id: rid("old"),
                translation: translation.clone(),
            },
        ])
        .unwrap();
    drop(store);
    assert_eq!(
        open_at(&path).summary_translation(&rid("old")).unwrap(),
        Some(translation)
    );
}

fn open_at(path: &std::path::Path) -> SqliteStore {
    SqliteStore::open(path).unwrap()
}

#[test]
fn a_translate_summary_job_survives_a_restart_and_keeps_its_wish() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir);
    store
        .apply(vec![Change::RecordingCreated(recording("r", 1))])
        .unwrap();
    store
        .apply(vec![
            Change::SummaryTranslationWanted {
                id: rid("r"),
                target: LangTag::parse("fr").unwrap(),
            },
            Change::JobEnqueued {
                id: rid("r"),
                goal: Goal::TRANSLATE_SUMMARY,
                priority: Priority::Front,
                at: Timestamp(5),
            },
        ])
        .unwrap();
    let job = store.jobs_for(&rid("r")).unwrap().remove(0);
    store
        .apply(vec![Change::Job {
            job: job.id,
            event: JobEvent::Started {
                stage: Stage::TranslateSummary,
            },
        }])
        .unwrap();
    drop(store);
    let reopened = open(&dir);
    let job = reopened.jobs_for(&rid("r")).unwrap().remove(0);
    assert_eq!(job.goal, Goal::TRANSLATE_SUMMARY);
    assert_eq!(
        job.state,
        JobState::Running {
            stage: Stage::TranslateSummary
        }
    );
    assert_eq!(job.priority, Priority::Front);
    assert_eq!(
        reopened.summary_translation_wanted(&rid("r")).unwrap(),
        Some(LangTag::parse("fr").unwrap())
    );
}
