CREATE TABLE previous_summaries (
    recording_id        TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    markdown            TEXT NOT NULL,
    language            TEXT,
    produced_by         TEXT NOT NULL,
    transcript_revision INTEGER NOT NULL
);

CREATE TABLE summary_instructions (
    recording_id TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    instruction  TEXT NOT NULL
);
