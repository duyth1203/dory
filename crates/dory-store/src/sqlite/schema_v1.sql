CREATE TABLE recordings (
    id               TEXT PRIMARY KEY NOT NULL,
    title            TEXT,
    created_at       INTEGER NOT NULL,
    language         TEXT,
    layout           TEXT NOT NULL,
    take_container   TEXT,
    take_rate_hz     INTEGER,
    take_duration_ms INTEGER,
    names_revision   INTEGER NOT NULL DEFAULT 0,
    title_lc         TEXT NOT NULL DEFAULT '',
    transcript_lc    TEXT NOT NULL DEFAULT '',
    summary_lc       TEXT NOT NULL DEFAULT ''
);
CREATE INDEX recordings_newest ON recordings (created_at DESC, id);

CREATE TABLE transcripts (
    recording_id TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    revision     INTEGER NOT NULL,
    languages    TEXT NOT NULL,
    produced_by  TEXT NOT NULL
);

CREATE TABLE segments (
    recording_id TEXT NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    start_ms     INTEGER,
    end_ms       INTEGER,
    speaker      INTEGER NOT NULL,
    channel      TEXT NOT NULL,
    text         TEXT NOT NULL,
    PRIMARY KEY (recording_id, idx)
) WITHOUT ROWID;

CREATE TABLE summaries (
    recording_id        TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    markdown            TEXT NOT NULL,
    language            TEXT,
    produced_by         TEXT NOT NULL,
    transcript_revision INTEGER NOT NULL
);

CREATE TABLE speaker_names (
    recording_id TEXT NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    speaker      INTEGER NOT NULL,
    name         TEXT NOT NULL,
    PRIMARY KEY (recording_id, speaker)
) WITHOUT ROWID;

CREATE TABLE jobs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id  TEXT NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    goal          INTEGER NOT NULL,
    done          TEXT NOT NULL,
    state         TEXT NOT NULL,
    stage         TEXT,
    blocker       TEXT,
    error_class   TEXT,
    error_message TEXT,
    priority      INTEGER NOT NULL,
    attempts      INTEGER NOT NULL,
    not_before    INTEGER,
    created_at    INTEGER NOT NULL
);
CREATE INDEX jobs_runnable ON jobs (state, priority DESC, created_at, id);
CREATE INDEX jobs_by_recording ON jobs (recording_id, id);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE legacy_imports (
    recording_id TEXT PRIMARY KEY NOT NULL,
    content_hash TEXT NOT NULL,
    job_id       INTEGER
);
