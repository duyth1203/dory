-- The output language: the language each segment was heard in, and the transcript translated into
-- the language the user wants transcripts written in.
ALTER TABLE segments ADD COLUMN language TEXT;

-- One translation per recording, beside the spoken segments, never over them. It names the
-- transcript revision it was made from. `texts` is a JSON array with one entry per segment of that
-- revision, a string or null when the segment keeps its spoken text.
CREATE TABLE translations (
    recording_id        TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    target              TEXT NOT NULL,
    transcript_revision INTEGER NOT NULL,
    produced_by         TEXT NOT NULL,
    texts               TEXT NOT NULL
);

-- The batches a translation run has finished, so a quit resumes where it stopped. `texts` is a
-- JSON array of [segment index, text] pairs. Cleared with the translation they built.
CREATE TABLE translation_batches (
    recording_id        TEXT NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    transcript_revision INTEGER NOT NULL,
    target              TEXT NOT NULL,
    model               TEXT NOT NULL,
    seq                 INTEGER NOT NULL,
    texts               TEXT NOT NULL,
    PRIMARY KEY (recording_id, transcript_revision, target, model, seq)
) WITHOUT ROWID;
