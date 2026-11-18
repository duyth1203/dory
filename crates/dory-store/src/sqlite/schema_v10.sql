-- A summary translated on request, beside the summary and never over it. At most one per
-- recording. It describes the summary now stored: the store deletes it in the same commit that
-- writes or restores a summary, so no translation outlives the text it was made from.
CREATE TABLE summary_translations (
    recording_id TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    target       TEXT NOT NULL,
    produced_by  TEXT NOT NULL,
    markdown     TEXT NOT NULL
);

-- The language the user last asked the summary to be translated into, until a translation into
-- it is stored or the summary changes. The translate-summary stage reads its target here.
CREATE TABLE summary_translation_wishes (
    recording_id TEXT PRIMARY KEY NOT NULL REFERENCES recordings (id) ON DELETE CASCADE,
    target       TEXT NOT NULL
);
