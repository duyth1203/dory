use dory_core::domain::{
    Channel, ChannelLayout, Goal, JobEvent, JobState, LanguagePref, Ms, Priority, Recording,
    RecordingId, RecordingQuery, Stage, TimeRange, Timestamp,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Checkpoint, CheckpointKey, Store, StoreError, StoreRead};
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
const LATEST: u32 = 3;

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
