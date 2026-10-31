-- Immutable exact broker responses, model settings and fx completion log, committed
-- atomically with the revision. Legacy providers leave this NULL.
ALTER TABLE revisions ADD COLUMN agent_run jsonb;
