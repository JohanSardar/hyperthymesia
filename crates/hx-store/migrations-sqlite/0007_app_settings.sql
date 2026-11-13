-- Owner-managed app preferences. Provider keys live in secrets/, never JSON.
CREATE TABLE app_settings (
 id INTEGER PRIMARY KEY CHECK(id=1),
 value TEXT NOT NULL CHECK(json_valid(value) AND json_type(value)='object')
) STRICT;
CREATE TABLE pull_history (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 recipe_id TEXT NOT NULL, source_id TEXT NOT NULL,
 started_at INTEGER NOT NULL, finished_at INTEGER,
 status TEXT NOT NULL, accepted INTEGER NOT NULL DEFAULT 0,
 duplicates INTEGER NOT NULL DEFAULT 0, rejected INTEGER NOT NULL DEFAULT 0
) STRICT;
