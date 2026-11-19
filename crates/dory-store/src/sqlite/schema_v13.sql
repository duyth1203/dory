-- The models the user picked for one job, over their Settings choice. NULL follows Settings.
ALTER TABLE jobs ADD COLUMN stt_model TEXT;
ALTER TABLE jobs ADD COLUMN llm_model TEXT;
