-- One row per AI run (a pass, a connection setup or a Test), with a
-- content-free log of its steps. Provider output and errors are never stored;
-- `problem` is a short code the app turns into plain words and a fix.
CREATE TABLE agent_runs (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 kind TEXT NOT NULL CHECK(kind IN ('pass','setup','check')),
 job_id INTEGER, window_start INTEGER, label TEXT,
 provider TEXT, model TEXT, effort TEXT,
 started_at INTEGER NOT NULL, finished_at INTEGER,
 state TEXT NOT NULL CHECK(state IN ('running','done','failed','waiting','stopped','skipped')),
 problem TEXT, cost_usd REAL, input_tokens INTEGER, output_tokens INTEGER,
 revision_id INTEGER,
 log TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(log) AND json_type(log)='array')
) STRICT;
CREATE INDEX agent_runs_started ON agent_runs(started_at);
CREATE INDEX agent_runs_job ON agent_runs(job_id);
