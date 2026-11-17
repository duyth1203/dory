use std::path::Path;

use dory_core::domain::{
    ChannelLayout, LanguagePref, Recording, RecordingId, RecordingQuery, Timestamp,
};
use dory_core::ports::{Change, Store, StoreError, StoreRead};
use dory_store::{SqliteReader, SqliteStore};

const LATEST: u32 = 7;

fn recording(id: &str) -> Recording {
    Recording {
        id: RecordingId::parse(id).unwrap(),
        title: Some(format!("title of {id}")),
        created_at: Timestamp(1),
        language: LanguagePref::Auto,
        layout: ChannelLayout::MicSystem,
        take: None,
    }
}

fn library_with_one(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("dory.sqlite");
    SqliteStore::open(&path)
        .unwrap()
        .apply(vec![Change::RecordingCreated(recording("one"))])
        .unwrap();
    path
}

fn set_version(path: &Path, version: u32) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version = {version}"))
        .unwrap();
}

fn version(path: &Path) -> u32 {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

/// The database file's bytes. A WAL reader may create SQLite's `-wal` and `-shm` sidecars, which
/// is how readers coordinate, but it never writes the database itself.
fn database(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

#[test]
fn a_reader_reads_what_the_store_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    store
        .apply(vec![Change::RecordingCreated(recording("one"))])
        .unwrap();
    let reader = SqliteReader::open(&path).unwrap();
    let rows = reader.recordings(&RecordingQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);
    store
        .apply(vec![Change::RecordingCreated(recording("two"))])
        .unwrap();
    assert_eq!(
        reader.recordings(&RecordingQuery::default()).unwrap().len(),
        2,
        "a later commit is visible to the open reader"
    );
}

#[test]
fn a_reader_never_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let path = library_with_one(dir.path());
    set_version(&path, 2);
    let before = database(&path);
    match SqliteReader::open(&path) {
        Err(StoreError::SchemaTooOld {
            found: 2,
            supported: LATEST,
        }) => {}
        other => panic!("expected SchemaTooOld, got {other:?}"),
    }
    assert_eq!(version(&path), 2, "the reader did not migrate");
    assert_eq!(database(&path), before, "the database did not change");
}

#[test]
fn a_reader_never_creates_a_library() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("absent");
    let path = folder.join("dory.sqlite");
    match SqliteReader::open(&path) {
        Err(StoreError::Missing(p)) => assert_eq!(p, path),
        other => panic!("expected Missing, got {other:?}"),
    }
    assert!(!folder.exists(), "the folder was not created");
}

#[test]
fn an_empty_file_is_not_a_library() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.sqlite");
    std::fs::write(&path, b"").unwrap();
    assert!(matches!(
        SqliteReader::open(&path),
        Err(StoreError::Missing(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"");
}

#[test]
fn a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = library_with_one(dir.path());
    set_version(&path, LATEST + 1);
    let before = database(&path);
    assert!(matches!(
        SqliteReader::open(&path),
        Err(StoreError::SchemaTooNew { found, supported: LATEST }) if found == LATEST + 1
    ));
    assert_eq!(database(&path), before);
}

#[test]
fn a_schema_change_while_open_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = library_with_one(dir.path());
    let reader = SqliteReader::open(&path).unwrap();
    assert_eq!(
        reader.recordings(&RecordingQuery::default()).unwrap().len(),
        1
    );
    set_version(&path, LATEST + 1);
    assert!(matches!(
        reader.recordings(&RecordingQuery::default()),
        Err(StoreError::SchemaChanged { found, supported: LATEST }) if found == LATEST + 1
    ));
}

#[test]
fn a_wal_library_without_shm_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = library_with_one(dir.path());
    let shm = dir.path().join("dory.sqlite-shm");
    let wal = dir.path().join("dory.sqlite-wal");
    assert!(!shm.exists() && !wal.exists(), "a clean close removes them");
    let before = std::fs::read(&path).unwrap();
    let reader = SqliteReader::open(&path).unwrap();
    assert_eq!(
        reader.recordings(&RecordingQuery::default()).unwrap().len(),
        1
    );
    drop(reader);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the database bytes did not change"
    );
}
