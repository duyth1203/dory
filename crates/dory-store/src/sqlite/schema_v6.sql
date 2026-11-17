CREATE TABLE search_keys (
    key          INTEGER PRIMARY KEY,
    recording_id TEXT NOT NULL UNIQUE REFERENCES recordings (id) ON DELETE CASCADE
);
CREATE VIRTUAL TABLE search USING fts5(
    title, speakers, summary, transcript,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 0',
    prefix = '1 2 3'
);
CREATE TABLE search_meta (
    tokens_version INTEGER NOT NULL,
    generation     INTEGER NOT NULL
);
INSERT INTO search_meta (tokens_version, generation) VALUES (0, 0);
ALTER TABLE recordings DROP COLUMN title_lc;
ALTER TABLE recordings DROP COLUMN transcript_lc;
ALTER TABLE recordings DROP COLUMN summary_lc;
