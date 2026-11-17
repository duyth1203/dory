ALTER TABLE jobs ADD COLUMN place INTEGER NOT NULL DEFAULT 0;
UPDATE jobs SET place = created_at;
DROP INDEX jobs_runnable;
CREATE INDEX jobs_runnable ON jobs (state, priority DESC, place, id);
