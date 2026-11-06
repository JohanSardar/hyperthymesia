-- Configuration and live evidence are core state, never agent-created tables.
CREATE TABLE sources (
 id TEXT PRIMARY KEY, connector_instance TEXT NOT NULL UNIQUE,
 name TEXT NOT NULL, description TEXT NOT NULL,
 enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
 status TEXT NOT NULL DEFAULT 'setting_up'
   CHECK(status IN ('setting_up','ready','needs_attention','needs_reconnect')),
 history_days INTEGER NOT NULL DEFAULT 1 CHECK(history_days BETWEEN 0 AND 30),
 connected_at INTEGER NOT NULL, setup_generation INTEGER NOT NULL DEFAULT 1 CHECK(setup_generation>0),
 last_pull_at INTEGER, last_error TEXT,
 CHECK(connector_instance='source:' || id)
) STRICT;

CREATE TABLE recipes (
 id TEXT PRIMARY KEY, source_id TEXT NOT NULL REFERENCES sources(id),
 name TEXT NOT NULL, request_hash TEXT NOT NULL,
 recipe TEXT NOT NULL CHECK(json_valid(recipe)),
 enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
 owner_off INTEGER NOT NULL DEFAULT 0 CHECK(owner_off IN (0,1)),
 CHECK(NOT owner_off OR NOT enabled), UNIQUE(source_id,request_hash)
) STRICT;
CREATE TABLE recipe_state (
 recipe_id TEXT PRIMARY KEY REFERENCES recipes(id), cursor TEXT CHECK(cursor IS NULL OR json_valid(cursor)),
 next_run_at INTEGER NOT NULL, last_ok_at INTEGER, last_error TEXT
) STRICT;
CREATE INDEX recipe_state_due ON recipe_state(next_run_at);
CREATE TABLE addons (
 id TEXT PRIMARY KEY, source_id TEXT NOT NULL REFERENCES sources(id),
 text_hash TEXT NOT NULL, text TEXT NOT NULL CHECK(length(CAST(text AS BLOB)) BETWEEN 1 AND 1000),
 enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
 owner_off INTEGER NOT NULL DEFAULT 0 CHECK(owner_off IN (0,1)),
 CHECK(NOT owner_off OR NOT enabled), UNIQUE(source_id,text_hash)
) STRICT;
CREATE TABLE switch_log (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 target_kind TEXT NOT NULL CHECK(target_kind IN ('source','recipe','addon')),
 target_id TEXT NOT NULL, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), changed_at INTEGER NOT NULL
) STRICT;
CREATE TABLE setup_runs (
 source_id TEXT NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL,
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','succeeded','failed')),
 result TEXT CHECK(result IS NULL OR json_valid(result)), last_error TEXT,
 created_at INTEGER NOT NULL, finished_at INTEGER,
 PRIMARY KEY(source_id,generation)
) STRICT;

CREATE TABLE live_snapshots (
 id INTEGER PRIMARY KEY, run_token TEXT NOT NULL,
 connector_instance TEXT NOT NULL REFERENCES sources(connector_instance),
 revision TEXT NOT NULL, envelope TEXT NOT NULL CHECK(json_valid(envelope)),
 sensitivity TEXT NOT NULL DEFAULT 'restricted' CHECK(sensitivity='restricted'),
 created_at INTEGER NOT NULL,
 UNIQUE(run_token,connector_instance,revision)
) STRICT;

-- Keep the existing event/OCR guards and extend both directions to live IDs.
CREATE TRIGGER live_snapshots_id_collision BEFORE INSERT ON live_snapshots BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in events') WHERE EXISTS(SELECT 1 FROM events WHERE id=NEW.id);
 SELECT RAISE(ABORT,'evidence ID already exists in ocr_evidence') WHERE EXISTS(SELECT 1 FROM ocr_evidence WHERE id=NEW.id);
END;
CREATE TRIGGER events_live_id_collision BEFORE INSERT ON events
WHEN EXISTS(SELECT 1 FROM live_snapshots WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in live_snapshots');
END;
CREATE TRIGGER ocr_live_id_collision BEFORE INSERT ON ocr_evidence
WHEN EXISTS(SELECT 1 FROM live_snapshots WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in live_snapshots');
END;
CREATE TRIGGER immutable_live_snapshots_update BEFORE UPDATE ON live_snapshots BEGIN
 SELECT RAISE(ABORT,'live snapshots are immutable');
END;
CREATE TRIGGER immutable_live_snapshots_delete BEFORE DELETE ON live_snapshots BEGIN
 SELECT RAISE(ABORT,'live snapshots are immutable');
END;
CREATE TRIGGER immutable_switch_log_update BEFORE UPDATE ON switch_log BEGIN
 SELECT RAISE(ABORT,'switch log is immutable');
END;
CREATE TRIGGER immutable_switch_log_delete BEFORE DELETE ON switch_log BEGIN
 SELECT RAISE(ABORT,'switch log is immutable');
END;
-- OCR rows predate live snapshots and allow updates; guard that direction too.
CREATE TRIGGER ocr_live_id_update_collision BEFORE UPDATE OF id ON ocr_evidence
WHEN EXISTS(SELECT 1 FROM live_snapshots WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in live_snapshots');
END;
