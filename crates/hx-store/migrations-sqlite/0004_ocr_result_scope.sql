-- A device result belongs to the connector instance of the still it describes,
-- so one device's result can never reach another device's copy of the same hash.
-- The empty scope is a job the server ran itself; connector instances are non-empty.
-- Dependent objects go first: RENAME validates every trigger and view in the schema.
-- DROP TABLE also removes triggers ON these tables without warning; all are recreated.
DROP TRIGGER events_id_collision;
DROP TRIGGER live_snapshots_id_collision;
DROP VIEW ocr_derived;
DROP TRIGGER ocr_reset;

CREATE TABLE ocr_jobs_scoped (
 sha256 TEXT NOT NULL REFERENCES blobs(sha256), engine_version TEXT NOT NULL,
 scope TEXT NOT NULL DEFAULT '',
 source_event_id INTEGER NOT NULL REFERENCES events(id),
 attempts INTEGER NOT NULL DEFAULT 0, available_at INTEGER NOT NULL,
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','complete','sampled','failed')),
 perceptual_hash TEXT, sampled_from TEXT REFERENCES blobs(sha256),
 result TEXT CHECK(result IS NULL OR json_valid(result)),
 PRIMARY KEY(sha256,engine_version,scope)
) STRICT;
-- Device lineage takes the first stored success in rowid order, so keep rowids.
INSERT INTO ocr_jobs_scoped(rowid,sha256,engine_version,source_event_id,attempts,available_at,status,perceptual_hash,sampled_from,result)
SELECT rowid,sha256,engine_version,source_event_id,attempts,available_at,status,perceptual_hash,sampled_from,result FROM ocr_jobs;

CREATE TABLE ocr_evidence_scoped (
 id INTEGER PRIMARY KEY, observation_id TEXT NOT NULL UNIQUE,
 source_event_id INTEGER NOT NULL REFERENCES events(id),
 sha256 TEXT NOT NULL, engine_version TEXT NOT NULL,
 scope TEXT NOT NULL DEFAULT '',
 kind TEXT NOT NULL DEFAULT 'ocr.text' CHECK(kind='ocr.text'),
 FOREIGN KEY(sha256,engine_version,scope) REFERENCES ocr_jobs_scoped(sha256,engine_version,scope),
 UNIQUE(source_event_id,sha256,engine_version)
) STRICT;
INSERT INTO ocr_evidence_scoped(id,observation_id,source_event_id,sha256,engine_version,kind)
SELECT id,observation_id,source_event_id,sha256,engine_version,kind FROM ocr_evidence;

DROP TABLE ocr_evidence;
DROP TABLE ocr_jobs;
ALTER TABLE ocr_jobs_scoped RENAME TO ocr_jobs;
ALTER TABLE ocr_evidence_scoped RENAME TO ocr_evidence;

CREATE TRIGGER ocr_reset AFTER UPDATE OF status ON ocr_jobs
WHEN NEW.status='pending' AND OLD.status IN ('failed','sampled') BEGIN
 UPDATE ocr_pending_sources SET settled=0 WHERE sha256=NEW.sha256 AND engine_version=NEW.engine_version;
END;
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
JOIN ocr_jobs o USING(sha256,engine_version,scope);

CREATE TRIGGER events_id_collision BEFORE INSERT ON events
WHEN EXISTS(SELECT 1 FROM ocr_evidence WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in ocr_evidence');
END;
CREATE TRIGGER ocr_evidence_id_collision BEFORE INSERT ON ocr_evidence
WHEN EXISTS(SELECT 1 FROM events WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in events');
END;
CREATE TRIGGER live_snapshots_id_collision BEFORE INSERT ON live_snapshots BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in events') WHERE EXISTS(SELECT 1 FROM events WHERE id=NEW.id);
 SELECT RAISE(ABORT,'evidence ID already exists in ocr_evidence') WHERE EXISTS(SELECT 1 FROM ocr_evidence WHERE id=NEW.id);
END;
CREATE TRIGGER ocr_live_id_collision BEFORE INSERT ON ocr_evidence
WHEN EXISTS(SELECT 1 FROM live_snapshots WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in live_snapshots');
END;
CREATE TRIGGER ocr_live_id_update_collision BEFORE UPDATE OF id ON ocr_evidence
WHEN EXISTS(SELECT 1 FROM live_snapshots WHERE id=NEW.id) BEGIN
 SELECT RAISE(ABORT,'evidence ID already exists in live_snapshots');
END;
