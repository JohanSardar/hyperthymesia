-- Pause and never-record rules, shared by the web app and the Mac menu bar.
CREATE TABLE recording_controls (
 id INTEGER PRIMARY KEY CHECK(id=1),
 value TEXT NOT NULL CHECK(json_valid(value) AND json_type(value)='object')
) STRICT;
