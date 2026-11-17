ALTER TABLE recordings ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;

CREATE TABLE assistant_access (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL
);

CREATE TABLE assistant_clients (
    label      TEXT PRIMARY KEY NOT NULL,
    key_digest TEXT NOT NULL UNIQUE,
    exposure   TEXT NOT NULL,
    writes     TEXT NOT NULL,
    added_at   INTEGER NOT NULL
) WITHOUT ROWID;
