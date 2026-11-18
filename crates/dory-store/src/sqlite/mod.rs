//! `SqliteStore`: the `Store` adapter over one SQLite file.
//!
//! One writer connection behind a mutex runs every `apply` as a single immediate transaction, so a
//! stage output and its job event commit together or not at all. A second read-only connection
//! serves reads. In WAL mode a reader sees the last committed state and never waits for the
//! writer, so a UI thread can read while a long `apply` runs. The schema is versioned with
//! `PRAGMA user_version` and grown by appending to [`MIGRATIONS`].

pub(crate) mod codec;
mod reader;
mod search;

pub use reader::SqliteReader;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dory_core::domain::{
    AssistantAccess, AssistantChange, AssistantClient, AssistantQuery, ClientKey, Exposure, Fields,
    Job, JobEvent, JobId, JobState, LangTag, Priority, Processing, Recording, RecordingEdit,
    RecordingId, RecordingQuery, RecordingRow, Recovery, Revision, Segment, Settings, SpeakerId,
    SpeakerNames, Summary, SummaryFlag, SummaryTranslation, TakeInfo, Timestamp, Transcript,
    Translation, Writes,
};
use dory_core::models::{Diar, Llm, Stt};
use dory_core::ports::{
    Change, Checkpoint, CheckpointKey, Store, StoreError, StoreRead, TranslationBatch,
    TranslationKey,
};
use dory_core::search::{Found, Query, Vocabulary};
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior,
    params,
};

use codec::{
    blocker_from_db, blocker_to_db, cause_to_db, channel_from_db, channel_to_db, choice_from_db,
    choice_to_db, class_to_db, container_from_db, container_to_db, corrupt, done_from_db,
    done_to_db, error_from_db, goal_from_db, goal_to_db, language_from_db, language_to_db,
    layout_from_db, layout_to_db, ms_from_db, parse_model_ref, pref_from_setting, pref_to_setting,
    range_from_db, stage_from_db, stage_to_db, tags_from_db, tags_to_db, to_i64,
};

/// A migration step SQL cannot do, run after the version's SQL in the same transaction.
type Step = fn(&Connection) -> Result<(), StoreError>;

/// One schema version: its SQL, then an optional [`Step`].
struct Migration {
    sql: &'static str,
    then: Option<Step>,
}

impl Migration {
    const fn sql(sql: &'static str) -> Self {
        Self { sql, then: None }
    }
}

/// Version `n` of the schema is `MIGRATIONS[n - 1]`. Never edit an entry that has shipped.
const MIGRATIONS: &[Migration] = &[
    Migration::sql(include_str!("schema_v1.sql")),
    Migration::sql(include_str!("schema_v2.sql")),
    Migration::sql(include_str!("schema_v3.sql")),
    Migration::sql(include_str!("schema_v4.sql")),
    Migration::sql(include_str!("schema_v5.sql")),
    Migration::sql(include_str!("schema_v6.sql")),
    Migration::sql(include_str!("schema_v7.sql")),
    // v8 created the index and filled it here until v11, whose rebuild reads v9's translations
    // and now fills it once for every older file.
    Migration::sql(include_str!("schema_v8.sql")),
    Migration::sql(include_str!("schema_v9.sql")),
    Migration::sql(include_str!("schema_v10.sql")),
    Migration {
        sql: include_str!("schema_v11.sql"),
        then: Some(search::rebuild),
    },
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn db(e: rusqlite::Error) -> StoreError {
    match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) =>
        {
            StoreError::Corrupt(e.to_string())
        }
        _ => StoreError::Unavailable(Box::new(e)),
    }
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Unavailable(Box::new(e))
}

pub struct SqliteStore {
    /// Declared before `writer`, so it closes first. The last connection to close checkpoints the
    /// WAL and deletes it, which a read-only connection cannot do, and this one has read the
    /// schema for its vocabulary view.
    reader: Mutex<Connection>,
    writer: Mutex<Connection>,
    /// The index's terms as of the generation they carry. Rebuilt by [`StoreRead::vocabulary`]
    /// when a write has moved the index's generation on.
    vocabulary: Mutex<Option<Arc<Vocabulary>>>,
    path: PathBuf,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteStore {
    /// Opens the database at `path`, creating it and its folder if needed, and migrates it to the
    /// schema this build knows. A database written by a newer build is refused, not touched.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut writer = Connection::open(path).map_err(db)?;
        writer.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
        writer
            .query_row("PRAGMA journal_mode = WAL", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(db)?;
        writer
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(db)?;
        writer
            .pragma_update(None, "foreign_keys", true)
            .map_err(db)?;
        if migrate(&mut writer)? | reindex_if_stale(&mut writer)? {
            // A migration can leave a WAL file of hundreds of megabytes, which the next process
            // to open the database scans before its first read or write.
            writer
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .map_err(db)?;
        }

        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db)?;
        reader.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
        search::open_vocabulary_view(&reader)?;
        Ok(Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            vocabulary: Mutex::new(None),
            path: path.to_owned(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The schema version of the open database.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        self.read(user_version)
    }

    pub(crate) fn write<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = lock(&self.writer);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let out = f(&tx)?;
        tx.commit().map_err(db)?;
        Ok(out)
    }

    fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = lock(&self.reader);
        let tx = conn.transaction().map_err(db)?;
        f(&tx)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in the middle of a transaction drops it, which rolls it back, so the connection is
    // still sound.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn user_version(conn: &Connection) -> Result<u32, StoreError> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db)
}

/// Returns whether it changed the schema.
fn migrate(conn: &mut Connection) -> Result<bool, StoreError> {
    let supported = u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX);
    let found = user_version(conn)?;
    if found > supported {
        return Err(StoreError::SchemaTooNew { found, supported });
    }
    let mut migrated = false;
    for (index, migration) in MIGRATIONS.iter().enumerate() {
        let target = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        // Read again under the write lock, because another process may have migrated meanwhile.
        if user_version(&tx)? >= target {
            continue;
        }
        tx.execute_batch(migration.sql).map_err(db)?;
        if let Some(step) = migration.then {
            step(&tx)?;
        }
        tx.execute_batch(&format!("PRAGMA user_version = {target}"))
            .map_err(db)?;
        tx.commit().map_err(db)?;
        migrated = true;
    }
    Ok(migrated)
}

/// Rebuilds the search index when it was built by another version of the core's tokens. Returns
/// whether it did.
fn reindex_if_stale(conn: &mut Connection) -> Result<bool, StoreError> {
    if search::is_current(conn)? {
        return Ok(false);
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db)?;
    let stale = !search::is_current(&tx)?;
    if stale {
        search::rebuild(&tx)?;
    }
    tx.commit().map_err(db)?;
    Ok(stale)
}

// ---- rows ---------------------------------------------------------------------------------

const JOB_COLUMNS: &str = "id, recording_id, goal, done, state, stage, blocker, error_class, \
                           error_message, priority, attempts, not_before, created_at, run_fast, \
                           error_cause, place";

struct RawState {
    state: String,
    stage: Option<String>,
    blocker: Option<String>,
    error_class: Option<String>,
    error_message: Option<String>,
    error_cause: Option<String>,
}

struct RawJob {
    id: i64,
    recording: String,
    goal: i64,
    done: String,
    state: RawState,
    priority: i64,
    attempts: i64,
    not_before: Option<i64>,
    created_at: i64,
    run_fast: bool,
    place: i64,
}

fn raw_job(row: &Row<'_>) -> rusqlite::Result<RawJob> {
    Ok(RawJob {
        id: row.get(0)?,
        recording: row.get(1)?,
        goal: row.get(2)?,
        done: row.get(3)?,
        state: RawState {
            state: row.get(4)?,
            stage: row.get(5)?,
            blocker: row.get(6)?,
            error_class: row.get(7)?,
            error_message: row.get(8)?,
            error_cause: row.get(14)?,
        },
        priority: row.get(9)?,
        attempts: row.get(10)?,
        not_before: row.get(11)?,
        created_at: row.get(12)?,
        run_fast: row.get(13)?,
        place: row.get(15)?,
    })
}

fn decode_state(raw: RawState) -> Result<JobState, StoreError> {
    let stage = || -> Result<_, StoreError> {
        let s = raw
            .stage
            .as_deref()
            .ok_or_else(|| corrupt("job state without a stage"))?;
        stage_from_db(s)
    };
    Ok(match raw.state.as_str() {
        "queued" => JobState::Queued,
        "running" => JobState::Running { stage: stage()? },
        "blocked" => JobState::Blocked {
            stage: stage()?,
            blocker: blocker_from_db(
                raw.blocker
                    .as_deref()
                    .ok_or_else(|| corrupt("blocked job without a blocker"))?,
            )?,
        },
        "succeeded" => JobState::Succeeded,
        "failed" => JobState::Failed {
            stage: stage()?,
            error: error_from_db(
                raw.error_class.clone(),
                raw.error_message.clone(),
                raw.error_cause.as_deref(),
            )?,
        },
        "cancelled" => JobState::Cancelled,
        other => return Err(corrupt(format!("unknown job state {other:?}"))),
    })
}

fn decode_job(raw: RawJob) -> Result<Job, StoreError> {
    Ok(Job {
        id: JobId(u64::try_from(raw.id).map_err(|_| corrupt("negative job id"))?),
        recording: RecordingId::parse(&raw.recording).map_err(|e| corrupt(e.to_string()))?,
        goal: goal_from_db(raw.goal)?,
        done: done_from_db(&raw.done)?,
        state: decode_state(raw.state)?,
        priority: match raw.priority {
            0 => Priority::Normal,
            1 => Priority::Front,
            other => return Err(corrupt(format!("unknown priority {other}"))),
        },
        place: raw.place,
        run_fast: raw.run_fast,
        attempts: u8::try_from(raw.attempts).map_err(|_| corrupt("job attempts out of range"))?,
        not_before: raw.not_before.map(Timestamp),
        created_at: Timestamp(raw.created_at),
    })
}

struct StateColumns {
    state: &'static str,
    stage: Option<&'static str>,
    blocker: Option<String>,
    error_class: Option<&'static str>,
    error_message: Option<String>,
    error_cause: Option<&'static str>,
}

fn encode_state(state: &JobState) -> StateColumns {
    let plain = |state| StateColumns {
        state,
        stage: None,
        blocker: None,
        error_class: None,
        error_message: None,
        error_cause: None,
    };
    match state {
        JobState::Queued => plain("queued"),
        JobState::Running { stage } => StateColumns {
            stage: Some(stage_to_db(*stage)),
            ..plain("running")
        },
        JobState::Blocked { stage, blocker } => StateColumns {
            stage: Some(stage_to_db(*stage)),
            blocker: Some(blocker_to_db(blocker)),
            ..plain("blocked")
        },
        JobState::Succeeded => plain("succeeded"),
        JobState::Failed { stage, error } => StateColumns {
            stage: Some(stage_to_db(*stage)),
            error_class: Some(class_to_db(error.class)),
            error_message: Some(error.message.clone()),
            error_cause: Some(cause_to_db(error.cause)),
            ..plain("failed")
        },
        JobState::Cancelled => plain("cancelled"),
    }
}

fn priority_to_db(priority: Priority) -> i64 {
    match priority {
        Priority::Normal => 0,
        Priority::Front => 1,
    }
}

pub(crate) fn load_job(conn: &Connection, id: JobId) -> Result<Option<Job>, StoreError> {
    let raw = conn
        .prepare_cached(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"))
        .map_err(db)?
        .query_row([to_i64(id.0)], raw_job)
        .optional()
        .map_err(db)?;
    raw.map(decode_job).transpose()
}

fn load_jobs_where(
    conn: &Connection,
    clause: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Job>, StoreError> {
    let mut stmt = conn
        .prepare_cached(&format!("SELECT {JOB_COLUMNS} FROM jobs {clause}"))
        .map_err(db)?;
    let raws = stmt
        .query_map(params, raw_job)
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    raws.into_iter().map(decode_job).collect()
}

struct RawRecording {
    id: String,
    title: Option<String>,
    created_at: i64,
    language: Option<String>,
    layout: String,
    container: Option<String>,
    rate_hz: Option<i64>,
    duration_ms: Option<i64>,
}

const RECORDING_COLUMNS: &str = "r.id, r.title, r.created_at, r.language, r.layout, \
                                 r.take_container, r.take_rate_hz, r.take_duration_ms";

fn raw_recording(row: &Row<'_>) -> rusqlite::Result<RawRecording> {
    Ok(RawRecording {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        language: row.get(3)?,
        layout: row.get(4)?,
        container: row.get(5)?,
        rate_hz: row.get(6)?,
        duration_ms: row.get(7)?,
    })
}

fn decode_recording(raw: RawRecording) -> Result<Recording, StoreError> {
    let take = match (raw.container, raw.rate_hz, raw.duration_ms) {
        (None, _, _) => None,
        (Some(container), Some(rate), Some(duration)) => Some(TakeInfo {
            container: container_from_db(&container)?,
            rate_hz: u32::try_from(rate).map_err(|_| corrupt("take rate out of range"))?,
            duration: ms_from_db(duration)?,
        }),
        (Some(_), _, _) => return Err(corrupt("take row is missing its rate or duration")),
    };
    Ok(Recording {
        id: RecordingId::parse(&raw.id).map_err(|e| corrupt(e.to_string()))?,
        title: raw.title,
        created_at: Timestamp(raw.created_at),
        language: language_from_db(raw.language.as_deref())?,
        layout: layout_from_db(&raw.layout)?,
        take,
    })
}

/// A list row: the recording, its transcript and summary revisions, and its newest job's state.
/// Selected from `recordings r` with [`ROW_JOINS`].
const ROW_COLUMNS: &str = "r.id, r.title, r.created_at, r.language, r.layout, \
                           r.take_container, r.take_rate_hz, r.take_duration_ms, \
                           t.revision, s.transcript_revision, \
                           j.state, j.stage, j.blocker, j.error_class, j.error_message, \
                           j.error_cause";
const ROW_WIDTH: usize = 16;
const ROW_JOINS: &str = "LEFT JOIN transcripts t ON t.recording_id = r.id
     LEFT JOIN summaries s ON s.recording_id = r.id
     LEFT JOIN jobs j ON j.id = (SELECT MAX(id) FROM jobs WHERE recording_id = r.id)";

struct RawRow {
    recording: RawRecording,
    transcript_revision: Option<i64>,
    summary_revision: Option<i64>,
    job: Option<RawState>,
}

fn raw_row(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    let job = match row.get::<_, Option<String>>(10)? {
        None => None,
        Some(state) => Some(RawState {
            state,
            stage: row.get(11)?,
            blocker: row.get(12)?,
            error_class: row.get(13)?,
            error_message: row.get(14)?,
            error_cause: row.get(15)?,
        }),
    };
    Ok(RawRow {
        recording: raw_recording(row)?,
        transcript_revision: row.get(8)?,
        summary_revision: row.get(9)?,
        job,
    })
}

fn decode_row(raw: RawRow) -> Result<RecordingRow, StoreError> {
    let revision = |n: Option<i64>| -> Result<Option<Revision>, StoreError> {
        n.map(|n| {
            u32::try_from(n)
                .map(Revision)
                .map_err(|_| corrupt("revision out of range"))
        })
        .transpose()
    };
    Ok(RecordingRow {
        recording: decode_recording(raw.recording)?,
        job: raw.job.map(decode_state).transpose()?,
        has_transcript: raw.transcript_revision.is_some(),
        summary: SummaryFlag::derive(
            revision(raw.summary_revision)?,
            revision(raw.transcript_revision)?,
        ),
    })
}

// ---- reads --------------------------------------------------------------------------------

/// One implementation of every read, shared by the store and the read-only reader. Each type
/// supplies `read`, which runs a closure in one read transaction.
macro_rules! store_reads {
    ($t:ty) => {
impl StoreRead for $t {
    fn recording(&self, id: &RecordingId) -> Result<Option<Recording>, StoreError> {
        self.read(|conn| {
            let raw = conn
                .prepare_cached(&format!(
                    "SELECT {RECORDING_COLUMNS} FROM recordings r WHERE r.id = ?1"
                ))
                .map_err(db)?
                .query_row([id.as_str()], raw_recording)
                .optional()
                .map_err(db)?;
            raw.map(decode_recording).transpose()
        })
    }

    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError> {
        let limit = query.limit.map_or(-1, i64::from);
        self.read(|conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {ROW_COLUMNS} FROM recordings r {ROW_JOINS}
                     ORDER BY r.created_at DESC, r.id ASC
                     LIMIT ?1"
                ))
                .map_err(db)?;
            let raws = stmt
                .query_map([limit], raw_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            raws.into_iter().map(decode_row).collect()
        })
    }

    fn search(&self, query: &Query) -> Result<Vec<Found>, StoreError> {
        let all = search::match_text(query);
        let title = search::title_match_text(query);
        self.read(|conn| {
            search::ready(conn)?;
            found_where(conn, true, "1", params![all, -1, title])
        })
    }

    fn vocabulary(&self) -> Result<Arc<Vocabulary>, StoreError> {
        // Held through the rebuild, so callers that arrive meanwhile wait for it, not repeat it.
        let mut cached = lock(&self.vocabulary);
        self.read(|conn| {
            let generation = search::generation(conn)?;
            if let Some(vocabulary) = cached.as_ref().filter(|v| v.generation() == generation) {
                return Ok(Arc::clone(vocabulary));
            }
            let vocabulary = Arc::new(search::read_vocabulary(conn, generation)?);
            *cached = Some(Arc::clone(&vocabulary));
            Ok(vocabulary)
        })
    }

    fn transcript(&self, id: &RecordingId) -> Result<Option<Transcript>, StoreError> {
        self.read(|conn| load_transcript(conn, id))
    }

    fn summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.read(|conn| load_summary(conn, id))
    }

    fn speaker_names(&self, id: &RecordingId) -> Result<SpeakerNames, StoreError> {
        self.read(|conn| {
            let revision = conn
                .prepare_cached("SELECT names_revision FROM recordings WHERE id = ?1")
                .map_err(db)?
                .query_row([id.as_str()], |row| row.get::<_, i64>(0))
                .optional()
                .map_err(db)?;
            let Some(revision) = revision else {
                return Ok(SpeakerNames::default());
            };
            let mut stmt = conn
                .prepare_cached("SELECT speaker, name FROM speaker_names WHERE recording_id = ?1")
                .map_err(db)?;
            let rows = stmt
                .query_map([id.as_str()], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            let mut names = BTreeMap::new();
            for (speaker, name) in rows {
                let speaker =
                    u16::try_from(speaker).map_err(|_| corrupt("speaker id out of range"))?;
                names.insert(SpeakerId(speaker), name);
            }
            Ok(SpeakerNames {
                for_revision: Revision(
                    u32::try_from(revision).map_err(|_| corrupt("revision out of range"))?,
                ),
                names,
            })
        })
    }

    fn job(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.read(|conn| load_job(conn, id))
    }

    fn jobs_for(&self, id: &RecordingId) -> Result<Vec<Job>, StoreError> {
        self.read(|conn| {
            load_jobs_where(conn, "WHERE recording_id = ?1 ORDER BY id", [id.as_str()])
        })
    }

    fn next_runnable_job(&self, now: Timestamp) -> Result<Option<Job>, StoreError> {
        self.read(|conn| {
            let mut jobs = load_jobs_where(
                conn,
                "WHERE state = 'queued' AND (not_before IS NULL OR not_before <= ?1)
                 ORDER BY priority DESC, place ASC, id ASC LIMIT 1",
                [now.0],
            )?;
            Ok(jobs.pop())
        })
    }

    fn next_wakeup(&self, now: Timestamp) -> Result<Option<Timestamp>, StoreError> {
        self.read(|conn| {
            let at: Option<i64> = conn
                .prepare_cached(
                    "SELECT MIN(not_before) FROM jobs WHERE state = 'queued' AND not_before > ?1",
                )
                .map_err(db)?
                .query_row([now.0], |row| row.get(0))
                .map_err(db)?;
            Ok(at.map(Timestamp))
        })
    }

    fn settings(&self) -> Result<Settings, StoreError> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT key, value FROM settings")
                .map_err(db)?;
            let rows: HashMap<String, String> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(db)?
                .collect::<Result<_, _>>()
                .map_err(db)?;
            let mut settings = Settings::default();
            if let Some(v) = rows.get("stt") {
                settings.stt = choice_from_db::<Stt>(v)?;
            }
            if let Some(v) = rows.get("llm") {
                settings.llm = choice_from_db::<Llm>(v)?;
            }
            if let Some(v) = rows.get("diar") {
                settings.diar = choice_from_db::<Diar>(v)?;
            }
            if let Some(v) = rows.get("language") {
                settings.language = pref_from_setting(v)?;
            }
            if let Some(v) = rows.get("meeting_languages") {
                settings.meeting_languages = v
                    .split(',')
                    .filter(|t| !t.is_empty())
                    .map(codec::parse_tag)
                    .collect::<Result<_, _>>()?;
            }
            if let Some(v) = rows.get("output_language") {
                settings.output_language = Some(v)
                    .filter(|v| !v.is_empty())
                    .map(|v| codec::parse_tag(v))
                    .transpose()?;
            }
            if let Some(v) = rows.get("auto_process") {
                settings.auto_process = match v.as_str() {
                    "1" => true,
                    "0" => false,
                    other => return Err(corrupt(format!("auto_process is {other:?}"))),
                };
            }
            if let Some(v) = rows.get("processing") {
                settings.processing = match v.as_str() {
                    "auto" => Processing::Auto,
                    "fast" => Processing::Fast,
                    "light" => Processing::Light,
                    other => return Err(corrupt(format!("processing is {other:?}"))),
                };
            }
            if let Some(v) = rows.get("switch_recovery") {
                settings.switch_recovery = match v.as_str() {
                    "off" => Recovery::Off,
                    "on" => Recovery::On,
                    "auto" => Recovery::Auto,
                    other => return Err(corrupt(format!("switch_recovery is {other:?}"))),
                };
            }
            Ok(settings)
        })
    }

    fn checkpoints(&self, key: &CheckpointKey) -> Result<Vec<Checkpoint>, StoreError> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT start_ms, end_ms, detected, segments FROM checkpoints
                     WHERE recording_id = ?1 AND model = ?2 AND channel = ?3 AND language = ?4
                     ORDER BY start_ms, end_ms",
                )
                .map_err(db)?;
            let raws = stmt
                .query_map(
                    params![
                        key.recording.as_str(),
                        key.model.as_str(),
                        channel_to_db(key.channel),
                        pref_to_setting(&key.language)
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            raws.into_iter()
                .map(|(start, end, detected, segments)| {
                    Ok(Checkpoint {
                        span: range_from_db(Some(start), Some(end))?
                            .ok_or_else(|| corrupt("checkpoint without a span"))?,
                        segments: codec::raw_segments_from_db(&segments)?,
                        detected_language: detected.as_deref().map(codec::parse_tag).transpose()?,
                    })
                })
                .collect()
        })
    }

    fn assistant_access(&self) -> Result<AssistantAccess, StoreError> {
        self.read(|conn| {
            let enabled = conn
                .prepare_cached("SELECT enabled FROM assistant_access WHERE id = 1")
                .map_err(db)?
                .query_row([], |row| row.get::<_, bool>(0))
                .optional()
                .map_err(db)?
                .unwrap_or(false);
            let clients = conn
                .prepare_cached(
                    "SELECT label, exposure, writes, added_at FROM assistant_clients
                     ORDER BY added_at, label",
                )
                .map_err(db)?
                .query_map([], raw_client)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?
                .into_iter()
                .map(decode_client)
                .collect::<Result<_, _>>()?;
            Ok(AssistantAccess { enabled, clients })
        })
    }

    fn assistant_client(&self, key: &ClientKey) -> Result<Option<AssistantClient>, StoreError> {
        let digest = key_digest(key);
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT label, exposure, writes, added_at FROM assistant_clients
                 WHERE key_digest = ?1",
            )
            .map_err(db)?
            .query_row([digest], raw_client)
            .optional()
            .map_err(db)?
            .map(decode_client)
            .transpose()
        })
    }

    fn hidden_from_assistants(&self, id: &RecordingId) -> Result<bool, StoreError> {
        self.read(|conn| {
            Ok(conn
                .prepare_cached("SELECT hidden FROM recordings WHERE id = ?1")
                .map_err(db)?
                .query_row([id.as_str()], |row| row.get::<_, bool>(0))
                .optional()
                .map_err(db)?
                .unwrap_or(false))
        })
    }

    fn previous_summary(&self, id: &RecordingId) -> Result<Option<Summary>, StoreError> {
        self.read(|conn| load_summary_from(conn, "previous_summaries", id))
    }

    fn summary_instruction(&self, id: &RecordingId) -> Result<Option<String>, StoreError> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT instruction FROM summary_instructions WHERE recording_id = ?1",
            )
            .map_err(db)?
            .query_row([id.as_str()], |row| row.get(0))
            .optional()
            .map_err(db)
        })
    }

    fn translation(&self, id: &RecordingId) -> Result<Option<Translation>, StoreError> {
        self.read(|conn| load_translation(conn, id))
    }

    fn summary_translation(
        &self,
        id: &RecordingId,
    ) -> Result<Option<SummaryTranslation>, StoreError> {
        self.read(|conn| load_summary_translation(conn, id))
    }

    fn summary_translation_wanted(&self, id: &RecordingId) -> Result<Option<LangTag>, StoreError> {
        self.read(|conn| load_summary_translation_wanted(conn, id))
    }

    fn translation_batches(
        &self,
        key: &TranslationKey,
    ) -> Result<Vec<TranslationBatch>, StoreError> {
        self.read(|conn| {
            let rows = conn
                .prepare_cached(
                    "SELECT texts FROM translation_batches
                     WHERE recording_id = ?1 AND transcript_revision = ?2 AND target = ?3
                       AND model = ?4
                     ORDER BY seq",
                )
                .map_err(db)?
                .query_map(
                    params![
                        key.recording.as_str(),
                        i64::from(key.revision.0),
                        key.target.as_str(),
                        key.model.as_str()
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            rows.iter()
                .map(|texts| {
                    Ok(TranslationBatch {
                        texts: codec::batch_texts_from_db(texts)?,
                    })
                })
                .collect()
        })
    }

    fn assistant_recordings(&self, query: &AssistantQuery) -> Result<Vec<Found>, StoreError> {
        let window = "r.hidden = 0
             AND (?4 IS NULL OR r.created_at >= ?4)
             AND (?5 IS NULL OR r.created_at < ?5)
             AND (?6 IS NULL OR r.created_at < ?6 OR (r.created_at = ?6 AND r.id > ?7))";
        let (after_at, after_id) = match &query.after {
            Some((at, id)) => (Some(at.0), Some(id.as_str().to_owned())),
            None => (None, None),
        };
        let limit = i64::from(query.limit);
        let (since, until) = (query.since.map(|t| t.0), query.until.map(|t| t.0));
        let Some(q) = &query.query else {
            return self.read(|conn| {
                let rows = rows_where(
                    conn,
                    window,
                    params![
                        Option::<i64>::None,
                        limit,
                        Option::<i64>::None,
                        since,
                        until,
                        after_at,
                        after_id
                    ],
                )?;
                Ok(rows
                    .into_iter()
                    .map(|row| Found {
                        row,
                        in_title: false,
                    })
                    .collect())
            });
        };
        let Some(all) = search::scoped_match_text(q, query.fields) else {
            return Ok(Vec::new());
        };
        let title = search::title_match_text(q);
        self.read(|conn| {
            search::ready(conn)?;
            found_where(
                conn,
                query.fields.title,
                window,
                params![all, limit, title, since, until, after_at, after_id],
            )
        })
    }

    fn assistant_vocabulary(&self, fields: Fields) -> Result<Vocabulary, StoreError> {
        self.read(|conn| {
            search::ready(conn)?;
            search::read_scoped_vocabulary(conn, fields)
        })
    }
}
    };
}

store_reads!(SqliteStore);
store_reads!(reader::SqliteReader);

/// Recording rows matching `filter`, newest first, at most `?2` of them. `filter` is SQL over the
/// `recordings r` row.
fn rows_where(
    conn: &Connection,
    filter: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<RecordingRow>, StoreError> {
    let raws = conn
        .prepare_cached(&format!(
            "SELECT {ROW_COLUMNS} FROM recordings r {ROW_JOINS}
             WHERE {filter}
             ORDER BY r.created_at DESC, r.id ASC
             LIMIT ?2"
        ))
        .map_err(db)?
        .query_map(params, raw_row)
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    raws.into_iter().map(decode_row).collect()
}

/// The recordings the index finds for the MATCH text `?1` that also pass `filter`, newest first,
/// at most `?2` of them. `filter` is SQL over the `recordings r` row whose parameters start at
/// `?4`. With `titles`, each says whether the MATCH text `?3` finds it too.
fn found_where(
    conn: &Connection,
    titles: bool,
    filter: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Found>, StoreError> {
    let in_title = match titles {
        true => "k.key IN (SELECT rowid FROM search WHERE search MATCH ?3)",
        false => "?3 IS NULL",
    };
    let raws = conn
        .prepare_cached(&format!(
            "SELECT {ROW_COLUMNS}, {in_title}
             FROM search
             JOIN search_keys k ON k.key = search.rowid
             JOIN recordings r ON r.id = k.recording_id
             {ROW_JOINS}
             WHERE search MATCH ?1 AND {filter}
             ORDER BY r.created_at DESC, r.id ASC
             LIMIT ?2"
        ))
        .map_err(db)?
        .query_map(params, |row| {
            Ok((raw_row(row)?, row.get::<_, bool>(ROW_WIDTH)?))
        })
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    raws.into_iter()
        .map(|(raw, in_title)| {
            Ok(Found {
                row: decode_row(raw)?,
                in_title,
            })
        })
        .collect()
}

pub(crate) fn load_transcript(
    conn: &Connection,
    id: &RecordingId,
) -> Result<Option<Transcript>, StoreError> {
    let head = conn
        .prepare_cached(
            "SELECT revision, languages, produced_by FROM transcripts WHERE recording_id = ?1",
        )
        .map_err(db)?
        .query_row([id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .optional()
        .map_err(db)?;
    let Some((revision, languages, produced_by)) = head else {
        return Ok(None);
    };
    let mut stmt = conn
        .prepare_cached(
            "SELECT start_ms, end_ms, speaker, channel, text, language FROM segments
                 WHERE recording_id = ?1 ORDER BY idx",
        )
        .map_err(db)?;
    let raws = stmt
        .query_map([id.as_str()], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    let segments = raws
        .into_iter()
        .map(|(start, end, speaker, channel, text, language)| {
            Ok(Segment {
                range: range_from_db(start, end)?,
                speaker: SpeakerId(
                    u16::try_from(speaker).map_err(|_| corrupt("speaker id out of range"))?,
                ),
                channel: channel_from_db(&channel)?,
                text,
                language: language.as_deref().map(codec::parse_tag).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(Some(Transcript {
        revision: Revision(u32::try_from(revision).map_err(|_| corrupt("revision out of range"))?),
        segments,
        detected_languages: tags_from_db(&languages)?,
        produced_by: parse_model_ref(&produced_by)?,
    }))
}

pub(crate) fn load_translation(
    conn: &Connection,
    id: &RecordingId,
) -> Result<Option<Translation>, StoreError> {
    let raw = conn
        .prepare_cached(
            "SELECT target, transcript_revision, produced_by, texts
             FROM translations WHERE recording_id = ?1",
        )
        .map_err(db)?
        .query_row([id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .optional()
        .map_err(db)?;
    raw.map(|(target, revision, produced_by, texts)| {
        Ok(Translation {
            target: codec::parse_tag(&target)?,
            transcript_revision: Revision(
                u32::try_from(revision).map_err(|_| corrupt("revision out of range"))?,
            ),
            produced_by: parse_model_ref(&produced_by)?,
            texts: codec::translation_texts_from_db(&texts)?,
        })
    })
    .transpose()
}

pub(crate) fn load_summary_translation(
    conn: &Connection,
    id: &RecordingId,
) -> Result<Option<SummaryTranslation>, StoreError> {
    let raw = conn
        .prepare_cached(
            "SELECT target, produced_by, markdown FROM summary_translations
             WHERE recording_id = ?1",
        )
        .map_err(db)?
        .query_row([id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .optional()
        .map_err(db)?;
    raw.map(|(target, produced_by, markdown)| {
        Ok(SummaryTranslation {
            target: codec::parse_tag(&target)?,
            produced_by: parse_model_ref(&produced_by)?,
            markdown,
        })
    })
    .transpose()
}

pub(crate) fn load_summary_translation_wanted(
    conn: &Connection,
    id: &RecordingId,
) -> Result<Option<LangTag>, StoreError> {
    conn.prepare_cached("SELECT target FROM summary_translation_wishes WHERE recording_id = ?1")
        .map_err(db)?
        .query_row([id.as_str()], |row| row.get::<_, String>(0))
        .optional()
        .map_err(db)?
        .map(|target| codec::parse_tag(&target))
        .transpose()
}

/// A summary written or restored takes its translation and any wish for one with it.
fn drop_summary_translation(tx: &Connection, id: &RecordingId) -> Result<(), StoreError> {
    for table in ["summary_translations", "summary_translation_wishes"] {
        tx.prepare_cached(&format!("DELETE FROM {table} WHERE recording_id = ?1"))
            .map_err(db)?
            .execute([id.as_str()])
            .map_err(db)?;
    }
    Ok(())
}

pub(crate) fn load_summary(
    conn: &Connection,
    id: &RecordingId,
) -> Result<Option<Summary>, StoreError> {
    load_summary_from(conn, "summaries", id)
}

fn load_summary_from(
    conn: &Connection,
    table: &'static str,
    id: &RecordingId,
) -> Result<Option<Summary>, StoreError> {
    let raw = conn
        .prepare_cached(&format!(
            "SELECT markdown, language, produced_by, transcript_revision
                 FROM {table} WHERE recording_id = ?1"
        ))
        .map_err(db)?
        .query_row([id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .optional()
        .map_err(db)?;
    raw.map(|(markdown, language, produced_by, revision)| {
        Ok(Summary {
            markdown,
            language: language.as_deref().map(codec::parse_tag).transpose()?,
            produced_by: parse_model_ref(&produced_by)?,
            transcript_revision: Revision(
                u32::try_from(revision).map_err(|_| corrupt("revision out of range"))?,
            ),
        })
    })
    .transpose()
}

// ---- writes -------------------------------------------------------------------------------

impl Store for SqliteStore {
    fn apply(&self, changes: Vec<Change>) -> Result<(), StoreError> {
        if changes.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            for change in &changes {
                apply_change(tx, change)?;
            }
            Ok(())
        })
    }
}

fn write_summary(
    tx: &Connection,
    table: &'static str,
    id: &RecordingId,
    summary: &Summary,
) -> Result<(), StoreError> {
    tx.prepare_cached(&format!(
        "INSERT OR REPLACE INTO {table}
           (recording_id, markdown, language, produced_by, transcript_revision)
         VALUES (?1, ?2, ?3, ?4, ?5)"
    ))
    .map_err(db)?
    .execute(params![
        id.as_str(),
        summary.markdown,
        summary.language.as_ref().map(|l| l.to_string()),
        summary.produced_by.as_str(),
        i64::from(summary.transcript_revision.0),
    ])
    .map_err(db)?;
    Ok(())
}

fn key_digest(key: &ClientKey) -> String {
    crate::sha256::sha256_hex(key.as_str().as_bytes())
}

type RawClient = (String, String, String, i64);

fn raw_client(row: &Row<'_>) -> rusqlite::Result<RawClient> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode_client(
    (label, exposure, writes, added_at): RawClient,
) -> Result<AssistantClient, StoreError> {
    Ok(AssistantClient {
        exposure: Exposure::parse(&exposure)
            .ok_or_else(|| corrupt("unknown assistant exposure"))?,
        writes: Writes::parse(&writes).ok_or_else(|| corrupt("unknown assistant writes"))?,
        added_at: Timestamp(added_at),
        label,
    })
}

fn apply_assistant(tx: &Connection, change: &AssistantChange) -> Result<(), StoreError> {
    match change {
        AssistantChange::Enabled(on) => {
            tx.prepare_cached(
                "INSERT INTO assistant_access (id, enabled) VALUES (1, ?1)
                 ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled",
            )
            .map_err(db)?
            .execute([on])
            .map_err(db)?;
        }
        AssistantChange::ClientAdded { client, key } => {
            let digest = key_digest(key);
            let taken: bool = tx
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM assistant_clients
                     WHERE label = ?1 OR key_digest = ?2)",
                )
                .map_err(db)?
                .query_row(params![client.label, digest], |row| row.get(0))
                .map_err(db)?;
            if taken {
                return Err(corrupt(format!(
                    "an assistant named {} or with that key exists",
                    client.label
                )));
            }
            tx.prepare_cached(
                "INSERT INTO assistant_clients (label, key_digest, exposure, writes, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(db)?
            .execute(params![
                client.label,
                digest,
                client.exposure.as_str(),
                client.writes.as_str(),
                client.added_at.0
            ])
            .map_err(db)?;
        }
        AssistantChange::ClientChanged(client) => {
            let changed = tx
                .prepare_cached(
                    "UPDATE assistant_clients SET exposure = ?2, writes = ?3 WHERE label = ?1",
                )
                .map_err(db)?
                .execute(params![
                    client.label,
                    client.exposure.as_str(),
                    client.writes.as_str()
                ])
                .map_err(db)?;
            if changed == 0 {
                return Err(not_found("assistant", &client.label));
            }
        }
        AssistantChange::ClientRemoved { label } => {
            let removed = tx
                .prepare_cached("DELETE FROM assistant_clients WHERE label = ?1")
                .map_err(db)?
                .execute([label])
                .map_err(db)?;
            if removed == 0 {
                return Err(not_found("assistant", label));
            }
        }
        AssistantChange::RecordingHidden { id, hidden } => {
            require_recording(tx, id)?;
            tx.prepare_cached("UPDATE recordings SET hidden = ?2 WHERE id = ?1")
                .map_err(db)?
                .execute(params![id.as_str(), hidden])
                .map_err(db)?;
        }
    }
    Ok(())
}

fn not_found(what: &str, id: impl std::fmt::Display) -> StoreError {
    StoreError::NotFound(format!("{what} {id}"))
}

pub(crate) fn recording_exists(tx: &Connection, id: &RecordingId) -> Result<bool, StoreError> {
    tx.prepare_cached("SELECT 1 FROM recordings WHERE id = ?1")
        .map_err(db)?
        .exists([id.as_str()])
        .map_err(db)
}

fn require_recording(tx: &Connection, id: &RecordingId) -> Result<(), StoreError> {
    let found = tx
        .prepare_cached("SELECT 1 FROM recordings WHERE id = ?1")
        .map_err(db)?
        .exists([id.as_str()])
        .map_err(db)?;
    if found {
        Ok(())
    } else {
        Err(not_found("recording", id))
    }
}

/// Applies one change inside the caller's transaction. Returns the id of a job the change created.
pub(crate) fn apply_change(tx: &Connection, change: &Change) -> Result<Option<JobId>, StoreError> {
    match change {
        Change::RecordingCreated(rec) => {
            let take = rec.take.as_ref();
            let created = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO recordings
                       (id, title, created_at, language, layout,
                        take_container, take_rate_hz, take_duration_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(db)?
                .execute(params![
                    rec.id.as_str(),
                    rec.title,
                    rec.created_at.0,
                    language_to_db(&rec.language),
                    layout_to_db(rec.layout),
                    take.map(|t| container_to_db(t.container)),
                    take.map(|t| i64::from(t.rate_hz)),
                    take.map(|t| to_i64(t.duration.0)),
                ])
                .map_err(db)?;
            if created > 0 {
                search::reindex(tx, &rec.id)?;
            }
            Ok(None)
        }
        Change::RecordingEdited { id, edit } => {
            let changed = match edit {
                RecordingEdit::Title(title) => tx
                    .prepare_cached("UPDATE recordings SET title = ?2 WHERE id = ?1")
                    .map_err(db)?
                    .execute(params![id.as_str(), title]),
                RecordingEdit::Language(language) => tx
                    .prepare_cached("UPDATE recordings SET language = ?2 WHERE id = ?1")
                    .map_err(db)?
                    .execute(params![id.as_str(), language_to_db(language)]),
                RecordingEdit::Take(take) => tx
                    .prepare_cached(
                        "UPDATE recordings
                         SET take_container = ?2, take_rate_hz = ?3, take_duration_ms = ?4
                         WHERE id = ?1",
                    )
                    .map_err(db)?
                    .execute(params![
                        id.as_str(),
                        container_to_db(take.container),
                        i64::from(take.rate_hz),
                        to_i64(take.duration.0)
                    ]),
            }
            .map_err(db)?;
            if changed == 0 {
                return Err(not_found("recording", id));
            }
            if matches!(edit, RecordingEdit::Title(_)) {
                search::reindex(tx, id)?;
            }
            Ok(None)
        }
        Change::RecordingDeleted(id) => {
            search::forget(tx, id)?;
            tx.prepare_cached("DELETE FROM recordings WHERE id = ?1")
                .map_err(db)?
                .execute([id.as_str()])
                .map_err(db)?;
            Ok(None)
        }
        Change::TranscriptReplaced { id, transcript } => {
            require_recording(tx, id)?;
            replace_transcript(tx, id, transcript)?;
            search::reindex(tx, id)?;
            Ok(None)
        }
        Change::SpeakerNamesReplaced { id, names } => {
            require_recording(tx, id)?;
            tx.prepare_cached("UPDATE recordings SET names_revision = ?2 WHERE id = ?1")
                .map_err(db)?
                .execute(params![id.as_str(), i64::from(names.for_revision.0)])
                .map_err(db)?;
            tx.prepare_cached("DELETE FROM speaker_names WHERE recording_id = ?1")
                .map_err(db)?
                .execute([id.as_str()])
                .map_err(db)?;
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO speaker_names (recording_id, speaker, name) VALUES (?1, ?2, ?3)",
                )
                .map_err(db)?;
            for (speaker, name) in &names.names {
                insert
                    .execute(params![id.as_str(), i64::from(speaker.0), name])
                    .map_err(db)?;
            }
            search::reindex(tx, id)?;
            Ok(None)
        }
        Change::SummaryReplaced { id, summary } => {
            require_recording(tx, id)?;
            tx.prepare_cached(
                "INSERT OR REPLACE INTO previous_summaries
                   (recording_id, markdown, language, produced_by, transcript_revision)
                 SELECT recording_id, markdown, language, produced_by, transcript_revision
                 FROM summaries WHERE recording_id = ?1",
            )
            .map_err(db)?
            .execute([id.as_str()])
            .map_err(db)?;
            tx.prepare_cached(
                "INSERT OR REPLACE INTO summaries
                   (recording_id, markdown, language, produced_by, transcript_revision)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(db)?
            .execute(params![
                id.as_str(),
                summary.markdown,
                summary.language.as_ref().map(|l| l.to_string()),
                summary.produced_by.as_str(),
                i64::from(summary.transcript_revision.0),
            ])
            .map_err(db)?;
            drop_summary_translation(tx, id)?;
            search::reindex(tx, id)?;
            Ok(None)
        }
        Change::SummaryTranslationWanted { id, target } => {
            require_recording(tx, id)?;
            tx.prepare_cached(
                "INSERT OR REPLACE INTO summary_translation_wishes (recording_id, target)
                 VALUES (?1, ?2)",
            )
            .map_err(db)?
            .execute(params![id.as_str(), target.as_str()])
            .map_err(db)?;
            Ok(None)
        }
        Change::SummaryTranslated { id, translation } => {
            require_recording(tx, id)?;
            if load_summary_translation_wanted(tx, id)?.as_ref() == Some(&translation.target) {
                tx.prepare_cached(
                    "INSERT OR REPLACE INTO summary_translations
                       (recording_id, target, produced_by, markdown)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(db)?
                .execute(params![
                    id.as_str(),
                    translation.target.as_str(),
                    translation.produced_by.as_str(),
                    translation.markdown,
                ])
                .map_err(db)?;
                tx.prepare_cached("DELETE FROM summary_translation_wishes WHERE recording_id = ?1")
                    .map_err(db)?
                    .execute([id.as_str()])
                    .map_err(db)?;
            }
            Ok(None)
        }
        Change::JobEnqueued {
            id,
            goal,
            priority,
            at,
        } => {
            require_recording(tx, id)?;
            let existing = load_jobs_where(
                tx,
                "WHERE recording_id = ?1 AND goal = ?2 AND state IN ('queued', 'running', 'blocked')
                 ORDER BY id LIMIT 1",
                params![id.as_str(), goal_to_db(*goal)],
            )?;
            match existing.into_iter().next() {
                Some(job) => {
                    if *priority == Priority::Front {
                        write_job(tx, &job.on(&JobEvent::Promote)?)?;
                    }
                    Ok(Some(job.id))
                }
                None => {
                    let columns = encode_state(&JobState::Queued);
                    tx.prepare_cached(
                        "INSERT INTO jobs (recording_id, goal, done, state, stage, blocker,
                                           error_class, error_message, priority, attempts,
                                           not_before, created_at, place)
                         VALUES (?1, ?2, '', ?3, NULL, NULL, NULL, NULL, ?4, 0, NULL, ?5, ?5)",
                    )
                    .map_err(db)?
                    .execute(params![
                        id.as_str(),
                        goal_to_db(*goal),
                        columns.state,
                        priority_to_db(*priority),
                        at.0
                    ])
                    .map_err(db)?;
                    let rowid = tx.last_insert_rowid();
                    Ok(Some(JobId(
                        u64::try_from(rowid).map_err(|_| corrupt("negative job id"))?,
                    )))
                }
            }
        }
        Change::Job { job, event } => {
            let current = load_job(tx, *job)?.ok_or_else(|| not_found("job", job.0))?;
            write_job(tx, &current.on(event)?)?;
            Ok(None)
        }
        Change::CheckpointSaved { key, checkpoint } => {
            require_recording(tx, &key.recording)?;
            tx.prepare_cached(
                "INSERT OR REPLACE INTO checkpoints
                   (recording_id, model, channel, language, start_ms, end_ms, detected, segments)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(db)?
            .execute(params![
                key.recording.as_str(),
                key.model.as_str(),
                channel_to_db(key.channel),
                pref_to_setting(&key.language),
                to_i64(checkpoint.span.start().0),
                to_i64(checkpoint.span.end().0),
                checkpoint.detected_language.as_ref().map(|l| l.to_string()),
                codec::raw_segments_to_db(&checkpoint.segments),
            ])
            .map_err(db)?;
            Ok(None)
        }
        Change::CheckpointsCleared(id) => {
            tx.prepare_cached("DELETE FROM checkpoints WHERE recording_id = ?1")
                .map_err(db)?
                .execute([id.as_str()])
                .map_err(db)?;
            Ok(None)
        }
        Change::TranslationReplaced { id, translation } => {
            require_recording(tx, id)?;
            tx.prepare_cached(
                "INSERT OR REPLACE INTO translations
                   (recording_id, target, transcript_revision, produced_by, texts)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(db)?
            .execute(params![
                id.as_str(),
                translation.target.as_str(),
                i64::from(translation.transcript_revision.0),
                translation.produced_by.as_str(),
                codec::translation_texts_to_db(&translation.texts),
            ])
            .map_err(db)?;
            search::reindex(tx, id)?;
            Ok(None)
        }
        Change::TranslationBatchSaved { key, batch } => {
            require_recording(tx, &key.recording)?;
            tx.prepare_cached(
                "INSERT INTO translation_batches
                   (recording_id, transcript_revision, target, model, seq, texts)
                 SELECT ?1, ?2, ?3, ?4, COALESCE(MAX(seq) + 1, 0), ?5
                 FROM translation_batches
                 WHERE recording_id = ?1 AND transcript_revision = ?2 AND target = ?3
                   AND model = ?4",
            )
            .map_err(db)?
            .execute(params![
                key.recording.as_str(),
                i64::from(key.revision.0),
                key.target.as_str(),
                key.model.as_str(),
                codec::batch_texts_to_db(&batch.texts),
            ])
            .map_err(db)?;
            Ok(None)
        }
        Change::TranslationBatchesCleared(id) => {
            tx.prepare_cached("DELETE FROM translation_batches WHERE recording_id = ?1")
                .map_err(db)?
                .execute([id.as_str()])
                .map_err(db)?;
            Ok(None)
        }
        Change::Assistant(change) => {
            apply_assistant(tx, change)?;
            Ok(None)
        }
        Change::SummaryRestored(id) => {
            require_recording(tx, id)?;
            let previous = load_summary_from(tx, "previous_summaries", id)?
                .ok_or_else(|| not_found("previous summary of", id))?;
            let current = load_summary(tx, id)?;
            write_summary(tx, "summaries", id, &previous)?;
            match current {
                Some(current) => write_summary(tx, "previous_summaries", id, &current)?,
                None => {
                    tx.prepare_cached("DELETE FROM previous_summaries WHERE recording_id = ?1")
                        .map_err(db)?
                        .execute([id.as_str()])
                        .map_err(db)?;
                }
            }
            drop_summary_translation(tx, id)?;
            search::reindex(tx, id)?;
            Ok(None)
        }
        Change::SummaryInstructionSet { id, instruction } => {
            require_recording(tx, id)?;
            match instruction {
                Some(text) => tx
                    .prepare_cached(
                        "INSERT OR REPLACE INTO summary_instructions (recording_id, instruction)
                         VALUES (?1, ?2)",
                    )
                    .map_err(db)?
                    .execute(params![id.as_str(), text]),
                None => tx
                    .prepare_cached("DELETE FROM summary_instructions WHERE recording_id = ?1")
                    .map_err(db)?
                    .execute([id.as_str()]),
            }
            .map_err(db)?;
            Ok(None)
        }
        Change::SettingsReplaced(settings) => {
            let mut put = tx
                .prepare_cached("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)")
                .map_err(db)?;
            for (key, value) in [
                ("stt", choice_to_db(&settings.stt)),
                ("llm", choice_to_db(&settings.llm)),
                ("diar", choice_to_db(&settings.diar)),
                ("language", pref_to_setting(&settings.language)),
                (
                    "meeting_languages",
                    settings
                        .meeting_languages
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                (
                    "output_language",
                    settings
                        .output_language
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ),
                (
                    "auto_process",
                    if settings.auto_process { "1" } else { "0" }.to_owned(),
                ),
                (
                    "processing",
                    match settings.processing {
                        Processing::Auto => "auto",
                        Processing::Fast => "fast",
                        Processing::Light => "light",
                    }
                    .to_owned(),
                ),
                (
                    "switch_recovery",
                    match settings.switch_recovery {
                        Recovery::Off => "off",
                        Recovery::On => "on",
                        Recovery::Auto => "auto",
                    }
                    .to_owned(),
                ),
            ] {
                put.execute(params![key, value]).map_err(db)?;
            }
            Ok(None)
        }
    }
}

fn replace_transcript(
    tx: &Connection,
    id: &RecordingId,
    transcript: &Transcript,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT OR REPLACE INTO transcripts (recording_id, revision, languages, produced_by)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .map_err(db)?
    .execute(params![
        id.as_str(),
        i64::from(transcript.revision.0),
        tags_to_db(&transcript.detected_languages),
        transcript.produced_by.as_str(),
    ])
    .map_err(db)?;
    tx.prepare_cached("DELETE FROM segments WHERE recording_id = ?1")
        .map_err(db)?
        .execute([id.as_str()])
        .map_err(db)?;
    let mut insert = tx
        .prepare_cached(
            "INSERT INTO segments
               (recording_id, idx, start_ms, end_ms, speaker, channel, text, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(db)?;
    for (idx, segment) in transcript.segments.iter().enumerate() {
        insert
            .execute(params![
                id.as_str(),
                to_i64(idx as u64),
                segment.range.map(|r| to_i64(r.start().0)),
                segment.range.map(|r| to_i64(r.end().0)),
                i64::from(segment.speaker.0),
                channel_to_db(segment.channel),
                segment.text,
                segment.language.as_ref().map(ToString::to_string),
            ])
            .map_err(db)?;
    }
    Ok(())
}

fn write_job(tx: &Connection, job: &Job) -> Result<(), StoreError> {
    let columns = encode_state(&job.state);
    let changed = tx
        .prepare_cached(
            "UPDATE jobs SET goal = ?2, done = ?3, state = ?4, stage = ?5, blocker = ?6,
                             error_class = ?7, error_message = ?8, priority = ?9, attempts = ?10,
                             not_before = ?11, created_at = ?12, run_fast = ?13,
                             error_cause = ?14, place = ?15
             WHERE id = ?1",
        )
        .map_err(db)?
        .execute(params![
            to_i64(job.id.0),
            goal_to_db(job.goal),
            done_to_db(&job.done),
            columns.state,
            columns.stage,
            columns.blocker,
            columns.error_class,
            columns.error_message,
            priority_to_db(job.priority),
            i64::from(job.attempts),
            job.not_before.map(|t| t.0),
            job.created_at.0,
            job.run_fast,
            columns.error_cause,
            job.place,
        ])
        .map_err(db)?;
    if changed == 0 {
        return Err(not_found("job", job.id.0));
    }
    Ok(())
}
