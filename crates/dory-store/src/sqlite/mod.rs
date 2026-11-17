//! `SqliteStore`: the `Store` adapter over one SQLite file.
//!
//! One writer connection behind a mutex runs every `apply` as a single immediate transaction, so a
//! stage output and its job event commit together or not at all. A second read-only connection
//! serves reads. In WAL mode a reader sees the last committed state and never waits for the
//! writer, so a UI thread can read while a long `apply` runs. The schema is versioned with
//! `PRAGMA user_version` and grown by appending to [`MIGRATIONS`].

pub(crate) mod codec;
mod reader;

pub use reader::SqliteReader;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dory_core::domain::{
    Job, JobEvent, JobId, JobState, Priority, Processing, Recording, RecordingEdit, RecordingId,
    RecordingQuery, RecordingRow, Revision, Segment, Settings, SpeakerId, SpeakerNames, Summary,
    SummaryFlag, TakeInfo, Timestamp, Transcript,
};
use dory_core::models::{Diar, Llm, Stt};
use dory_core::ports::{Change, Checkpoint, CheckpointKey, Store, StoreError, StoreRead};
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

/// Version `n` of the schema is `MIGRATIONS[n - 1]`. Never edit an entry that has shipped.
const MIGRATIONS: &[&str] = &[
    include_str!("schema_v1.sql"),
    include_str!("schema_v2.sql"),
    include_str!("schema_v3.sql"),
    include_str!("schema_v4.sql"),
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
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
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
        migrate(&mut writer)?;

        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db)?;
        reader.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
        Ok(Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
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

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let supported = u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX);
    let found = user_version(conn)?;
    if found > supported {
        return Err(StoreError::SchemaTooNew { found, supported });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let target = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        // Read again under the write lock, because another process may have migrated meanwhile.
        if user_version(&tx)? >= target {
            continue;
        }
        tx.execute_batch(sql).map_err(db)?;
        tx.execute_batch(&format!("PRAGMA user_version = {target}"))
            .map_err(db)?;
        tx.commit().map_err(db)?;
    }
    Ok(())
}

// ---- rows ---------------------------------------------------------------------------------

const JOB_COLUMNS: &str = "id, recording_id, goal, done, state, stage, blocker, error_class, \
                           error_message, priority, attempts, not_before, created_at, run_fast, \
                           error_cause";

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
        let needle = query
            .text
            .as_deref()
            .map(|t| t.trim().to_lowercase())
            .unwrap_or_default();
        let limit = query.limit.map_or(-1, i64::from);
        self.read(|conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {RECORDING_COLUMNS}, t.revision, s.transcript_revision,
                            j.state, j.stage, j.blocker, j.error_class, j.error_message,
                            j.error_cause
                     FROM recordings r
                     LEFT JOIN transcripts t ON t.recording_id = r.id
                     LEFT JOIN summaries s ON s.recording_id = r.id
                     LEFT JOIN jobs j ON j.id = (SELECT MAX(id) FROM jobs WHERE recording_id = r.id)
                     WHERE ?1 = '' OR instr(r.title_lc || char(10) || r.transcript_lc || char(10) || r.summary_lc, ?1) > 0
                     ORDER BY r.created_at DESC, r.id ASC
                     LIMIT ?2"
                ))
                .map_err(db)?;
            let mapped = stmt
                .query_map(params![needle, limit], |row| {
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
                    Ok((
                        raw_recording(row)?,
                        row.get::<_, Option<i64>>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                        job,
                    ))
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            mapped
                .into_iter()
                .map(|(raw, transcript_rev, summary_rev, job)| {
                    let revision = |n: Option<i64>| -> Result<Option<Revision>, StoreError> {
                        n.map(|n| {
                            u32::try_from(n)
                                .map(Revision)
                                .map_err(|_| corrupt("revision out of range"))
                        })
                        .transpose()
                    };
                    Ok(RecordingRow {
                        recording: decode_recording(raw)?,
                        job: job.map(decode_state).transpose()?,
                        has_transcript: transcript_rev.is_some(),
                        summary: SummaryFlag::derive(revision(summary_rev)?, revision(transcript_rev)?),
                    })
                })
                .collect()
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
                 ORDER BY priority DESC, created_at ASC, id ASC LIMIT 1",
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
}
    };
}

store_reads!(SqliteStore);
store_reads!(reader::SqliteReader);

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
            "SELECT start_ms, end_ms, speaker, channel, text FROM segments
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
            ))
        })
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    let segments = raws
        .into_iter()
        .map(|(start, end, speaker, channel, text)| {
            Ok(Segment {
                range: range_from_db(start, end)?,
                speaker: SpeakerId(
                    u16::try_from(speaker).map_err(|_| corrupt("speaker id out of range"))?,
                ),
                channel: channel_from_db(&channel)?,
                text,
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

pub(crate) fn load_summary(
    conn: &Connection,
    id: &RecordingId,
) -> Result<Option<Summary>, StoreError> {
    let raw = conn
        .prepare_cached(
            "SELECT markdown, language, produced_by, transcript_revision
                 FROM summaries WHERE recording_id = ?1",
        )
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
            tx.prepare_cached(
                "INSERT OR IGNORE INTO recordings
                   (id, title, created_at, language, layout,
                    take_container, take_rate_hz, take_duration_ms, title_lc)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
                rec.title.as_deref().unwrap_or_default().to_lowercase(),
            ])
            .map_err(db)?;
            Ok(None)
        }
        Change::RecordingEdited { id, edit } => {
            let changed = match edit {
                RecordingEdit::Title(title) => tx
                    .prepare_cached("UPDATE recordings SET title = ?2, title_lc = ?3 WHERE id = ?1")
                    .map_err(db)?
                    .execute(params![
                        id.as_str(),
                        title,
                        title.as_deref().unwrap_or_default().to_lowercase()
                    ]),
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
            Ok(None)
        }
        Change::RecordingDeleted(id) => {
            tx.prepare_cached("DELETE FROM recordings WHERE id = ?1")
                .map_err(db)?
                .execute([id.as_str()])
                .map_err(db)?;
            Ok(None)
        }
        Change::TranscriptReplaced { id, transcript } => {
            require_recording(tx, id)?;
            replace_transcript(tx, id, transcript)?;
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
            Ok(None)
        }
        Change::SummaryReplaced { id, summary } => {
            require_recording(tx, id)?;
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
            tx.prepare_cached("UPDATE recordings SET summary_lc = ?2 WHERE id = ?1")
                .map_err(db)?
                .execute(params![id.as_str(), summary.markdown.to_lowercase()])
                .map_err(db)?;
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
                                           not_before, created_at)
                         VALUES (?1, ?2, '', ?3, NULL, NULL, NULL, NULL, ?4, 0, NULL, ?5)",
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
            "INSERT INTO segments (recording_id, idx, start_ms, end_ms, speaker, channel, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .map_err(db)?;
    let mut search = String::new();
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
            ])
            .map_err(db)?;
        search.push_str(&segment.text);
        search.push('\n');
    }
    tx.prepare_cached("UPDATE recordings SET transcript_lc = ?2 WHERE id = ?1")
        .map_err(db)?
        .execute(params![id.as_str(), search.to_lowercase()])
        .map_err(db)?;
    Ok(())
}

fn write_job(tx: &Connection, job: &Job) -> Result<(), StoreError> {
    let columns = encode_state(&job.state);
    let changed = tx
        .prepare_cached(
            "UPDATE jobs SET goal = ?2, done = ?3, state = ?4, stage = ?5, blocker = ?6,
                             error_class = ?7, error_message = ?8, priority = ?9, attempts = ?10,
                             not_before = ?11, created_at = ?12, run_fast = ?13,
                             error_cause = ?14
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
        ])
        .map_err(db)?;
    if changed == 0 {
        return Err(not_found("job", job.id.0));
    }
    Ok(())
}
