-- Squashed from Postgres migrations 0001 through 0010. Historical data repairs
-- are intentionally absent. All application clocks are bound as Ts, except the
-- input_changed_at default/trigger, whose millisecond clock cannot delay settle.
CREATE TABLE evidence_seq (
 next_id INTEGER NOT NULL CHECK(next_id > 0)
) STRICT;
CREATE UNIQUE INDEX evidence_seq_singleton ON evidence_seq((1));
INSERT INTO evidence_seq VALUES(1);

CREATE TABLE events (
 id INTEGER PRIMARY KEY, observation_id TEXT NOT NULL, connector_instance TEXT NOT NULL,
 source_key TEXT NOT NULL, revision TEXT NOT NULL, kind TEXT NOT NULL,
 source_start INTEGER NOT NULL, source_end INTEGER, observed_at INTEGER NOT NULL,
 device TEXT, sensitivity TEXT NOT NULL, envelope TEXT NOT NULL CHECK(json_valid(envelope)),
 received_at INTEGER NOT NULL,
 UNIQUE(connector_instance,source_key,revision)
) STRICT;
CREATE INDEX events_time ON events(source_start,source_end);

CREATE TABLE normalized (
 event_id INTEGER PRIMARY KEY REFERENCES events(id), normalizer_version TEXT NOT NULL,
 payload TEXT NOT NULL CHECK(json_valid(payload)), search_text TEXT NOT NULL DEFAULT ''
) STRICT;
CREATE VIRTUAL TABLE normalized_fts USING fts5(
 search_text, content='normalized', content_rowid='event_id',
 tokenize='unicode61 remove_diacritics 0'
);
CREATE TRIGGER normalized_insert AFTER INSERT ON normalized BEGIN
 INSERT INTO normalized_fts(rowid,search_text) VALUES(NEW.event_id,NEW.search_text);
END;
-- normalized is mutable in Postgres. Keep the external-content index consistent
-- for updates/deletes too, even though today's normalizer only inserts.
CREATE TRIGGER normalized_update AFTER UPDATE ON normalized BEGIN
 INSERT INTO normalized_fts(normalized_fts,rowid,search_text) VALUES('delete',OLD.event_id,OLD.search_text);
 INSERT INTO normalized_fts(rowid,search_text) VALUES(NEW.event_id,NEW.search_text);
END;
CREATE TRIGGER normalized_delete AFTER DELETE ON normalized BEGIN
 INSERT INTO normalized_fts(normalized_fts,rowid,search_text) VALUES('delete',OLD.event_id,OLD.search_text);
END;

CREATE TABLE quarantine (
 event_id INTEGER PRIMARY KEY REFERENCES events(id), reason TEXT NOT NULL, created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE blobs (
 sha256 TEXT PRIMARY KEY CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'), bytes INTEGER,
 published INTEGER NOT NULL DEFAULT 0 CHECK(published IN (0,1)),
 evidence_expired INTEGER NOT NULL DEFAULT 0 CHECK(evidence_expired IN (0,1)),
 retention_class TEXT NOT NULL DEFAULT 'chunk', refcount INTEGER NOT NULL DEFAULT 0,
 pins INTEGER NOT NULL DEFAULT 0 CHECK(pins>=0), created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE event_blobs (
 event_id INTEGER REFERENCES events(id), sha256 TEXT REFERENCES blobs(sha256), PRIMARY KEY(event_id,sha256)
) STRICT;

CREATE TABLE windows (
 start_at INTEGER PRIMARY KEY, end_at INTEGER NOT NULL, generation INTEGER NOT NULL DEFAULT 1,
 dirty INTEGER NOT NULL DEFAULT 1 CHECK(dirty IN (0,1)),
 timeline TEXT CHECK(timeline IS NULL OR json_valid(timeline)), compiler_version TEXT, compiled_generation INTEGER,
 canonical_revision INTEGER REFERENCES revisions(id),
 input_changed_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec')*1000000 AS INTEGER)),
 empty_generation INTEGER, timeline_hash TEXT,
 revision24_finished INTEGER NOT NULL DEFAULT 0 CHECK(revision24_finished IN (0,1))
) STRICT;
CREATE TABLE revisions (
 id INTEGER PRIMARY KEY AUTOINCREMENT, start_at INTEGER NOT NULL REFERENCES windows(start_at),
 input_generation INTEGER NOT NULL, interpretation TEXT NOT NULL CHECK(json_valid(interpretation)),
 created_at INTEGER NOT NULL, output_key TEXT UNIQUE,
 input_namespaces TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(input_namespaces) AND json_type(input_namespaces)='array'),
 agent_run TEXT CHECK(agent_run IS NULL OR json_valid(agent_run)),
 has_dependency_record INTEGER NOT NULL DEFAULT 0 CHECK(has_dependency_record IN (0,1))
) STRICT;
CREATE INDEX revisions_input_identity ON revisions(start_at,input_generation);
CREATE TABLE revision_dependencies (
 revision_id INTEGER NOT NULL REFERENCES revisions(id),
 prior_revision_id INTEGER NOT NULL REFERENCES revisions(id),
 PRIMARY KEY(revision_id,prior_revision_id)
) STRICT;
CREATE INDEX revision_dependencies_prior ON revision_dependencies(prior_revision_id);

CREATE TABLE watermarks (
 connector_instance TEXT PRIMARY KEY, observed_through INTEGER NOT NULL,
 sequence TEXT CHECK(sequence IS NULL OR (length(sequence)=20 AND sequence NOT GLOB '*[^0-9]*'))
) STRICT;
CREATE TABLE device_tokens (
 hash TEXT PRIMARY KEY, name TEXT NOT NULL,
 prefixes TEXT NOT NULL CHECK(json_valid(prefixes) AND json_type(prefixes)='array'),
 scopes TEXT NOT NULL CHECK(json_valid(scopes) AND json_type(scopes)='array'),
 revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1))
) STRICT;
CREATE TABLE entity_assertions (
 id INTEGER PRIMARY KEY AUTOINCREMENT, entity TEXT NOT NULL,
 assertion TEXT NOT NULL CHECK(json_valid(assertion)),
 evidence TEXT NOT NULL CHECK(json_valid(evidence) AND json_type(evidence)='array'),
 sensitivity TEXT NOT NULL DEFAULT 'restricted'
) STRICT;
CREATE TABLE jobs (
 id INTEGER PRIMARY KEY AUTOINCREMENT, output_key TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
 payload TEXT NOT NULL CHECK(json_valid(payload)),
 queue TEXT NOT NULL DEFAULT 'interactive' CHECK(queue IN ('interactive','backfill')),
 state TEXT NOT NULL DEFAULT 'ready', available_at INTEGER NOT NULL,
 lease_until INTEGER, lease_owner TEXT, attempts INTEGER NOT NULL DEFAULT 0,
 max_attempts INTEGER NOT NULL DEFAULT 5, last_error TEXT,
 request_version INTEGER NOT NULL DEFAULT 1, leased_version INTEGER NOT NULL DEFAULT 0,
 revivals INTEGER NOT NULL DEFAULT 0,
 -- These replace comparisons of timestamps embedded in legacy JSON payloads.
 -- Non-window jobs may leave them NULL, just as they have no window in JSON.
 window_start INTEGER, generation INTEGER
) STRICT;
CREATE INDEX jobs_ready ON jobs(state,available_at);
CREATE INDEX jobs_window_generation ON jobs(window_start,generation);

CREATE TABLE ocr_jobs (
 sha256 TEXT NOT NULL REFERENCES blobs(sha256), engine_version TEXT NOT NULL,
 source_event_id INTEGER NOT NULL REFERENCES events(id),
 attempts INTEGER NOT NULL DEFAULT 0, available_at INTEGER NOT NULL,
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','complete','sampled','failed')),
 perceptual_hash TEXT, sampled_from TEXT REFERENCES blobs(sha256),
 result TEXT CHECK(result IS NULL OR json_valid(result)),
 PRIMARY KEY(sha256,engine_version)
) STRICT;
CREATE TABLE ocr_evidence (
 id INTEGER PRIMARY KEY, observation_id TEXT NOT NULL UNIQUE,
 source_event_id INTEGER NOT NULL REFERENCES events(id),
 sha256 TEXT NOT NULL, engine_version TEXT NOT NULL,
 kind TEXT NOT NULL DEFAULT 'ocr.text' CHECK(kind='ocr.text'),
 FOREIGN KEY(sha256,engine_version) REFERENCES ocr_jobs(sha256,engine_version),
 UNIQUE(source_event_id,sha256,engine_version)
) STRICT;
CREATE INDEX ocr_pending ON ocr_jobs(available_at) WHERE status='pending';
CREATE INDEX ocr_source ON ocr_evidence(source_event_id);
CREATE VIEW ocr_derived AS
SELECT d.id,d.source_event_id,d.sha256,d.engine_version,e.source_start,
 e.connector_instance,e.device,e.sensitivity,
 json_set(e.envelope,'$.kind','ocr.text','$.sensitivity',e.sensitivity,
   '$.observation_id',d.observation_id,'$.source_key','ocr:' || d.id,
   '$.payload',json_object('source_event_id',e.id,
     'source_observation_id',e.observation_id,'sha256',d.sha256,
     'engine_version',d.engine_version,'ocr',json(o.result))) AS envelope
FROM ocr_evidence d JOIN events e ON e.id=d.source_event_id
JOIN ocr_jobs o USING(sha256,engine_version);

CREATE TABLE daily_sweeps (
 day TEXT PRIMARY KEY CHECK(length(day)=10 AND day GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' AND date(day) IS day),
 start_at INTEGER NOT NULL REFERENCES windows(start_at), generation INTEGER NOT NULL,
 started INTEGER NOT NULL DEFAULT 0 CHECK(started IN (0,1))
) STRICT;

CREATE TRIGGER events_id_collision BEFORE INSERT ON events
WHEN EXISTS(SELECT 1 FROM ocr_evidence WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in ocr_evidence');
END;
CREATE TRIGGER ocr_evidence_id_collision BEFORE INSERT ON ocr_evidence
WHEN EXISTS(SELECT 1 FROM events WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in events');
END;

-- recursive_triggers=ON is essential: REPLACE performs an implicit DELETE.
CREATE TRIGGER immutable_events_update BEFORE UPDATE ON events BEGIN
 SELECT RAISE(ABORT,'evidence and interpretation revisions are immutable');
END;
CREATE TRIGGER immutable_events_delete BEFORE DELETE ON events BEGIN
 SELECT RAISE(ABORT,'evidence and interpretation revisions are immutable');
END;
CREATE TRIGGER immutable_revisions_update BEFORE UPDATE ON revisions BEGIN
 SELECT RAISE(ABORT,'evidence and interpretation revisions are immutable');
END;
CREATE TRIGGER immutable_revisions_delete BEFORE DELETE ON revisions BEGIN
 SELECT RAISE(ABORT,'evidence and interpretation revisions are immutable');
END;
CREATE TRIGGER window_input_changed AFTER UPDATE OF generation ON windows
WHEN NEW.generation IS NOT OLD.generation BEGIN
 UPDATE windows SET input_changed_at=CAST(unixepoch('subsec')*1000000 AS INTEGER) WHERE start_at=NEW.start_at;
END;
