CREATE TABLE checkpoints (
    recording_id TEXT NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    model        TEXT NOT NULL,
    channel      TEXT NOT NULL,
    language     TEXT NOT NULL,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER NOT NULL,
    detected     TEXT,
    segments     TEXT NOT NULL,
    PRIMARY KEY (recording_id, model, channel, language, start_ms, end_ms)
) WITHOUT ROWID;
