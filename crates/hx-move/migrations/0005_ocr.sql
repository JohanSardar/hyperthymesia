-- Exact blob identities are cached per engine/configuration. Similarity only skips sampling;
-- it never supplies another frame's text. Failed attempts are bounded and retryable.
CREATE TABLE ocr_jobs (
 sha256 text NOT NULL REFERENCES blobs(sha256), engine_version text NOT NULL,
 source_event_id bigint NOT NULL REFERENCES events(id),
 attempts integer NOT NULL DEFAULT 0, available_at timestamptz NOT NULL DEFAULT now(),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','complete','sampled','failed')),
 perceptual_hash text, sampled_from text REFERENCES blobs(sha256), result jsonb,
 PRIMARY KEY(sha256,engine_version)
);
-- IDs share the evidence sequence, so OCR can be cited and retrieved without collisions.
CREATE TABLE ocr_evidence (
 id bigint PRIMARY KEY DEFAULT nextval('events_id_seq'),
 observation_id text NOT NULL UNIQUE,
 source_event_id bigint NOT NULL REFERENCES events(id),
 sha256 text NOT NULL, engine_version text NOT NULL,
 kind text NOT NULL DEFAULT 'ocr.text' CHECK(kind='ocr.text'),
 FOREIGN KEY(sha256,engine_version) REFERENCES ocr_jobs(sha256,engine_version),
 UNIQUE(source_event_id,sha256,engine_version)
);
CREATE INDEX ocr_pending ON ocr_jobs(available_at) WHERE status='pending';
CREATE INDEX ocr_source ON ocr_evidence(source_event_id);
-- Resolve lineage and effective sensitivity at read time rather than copying a
-- producer label into a new disclosure grant. All OCR reads go through a source.
CREATE VIEW ocr_derived AS
SELECT d.id,d.source_event_id,d.sha256,d.engine_version,e.source_start,
 e.connector_instance,e.device,e.sensitivity,
 (e.envelope || jsonb_build_object('kind','ocr.text','sensitivity',e.sensitivity,
   'observation_id',d.observation_id,'source_key','ocr:' || d.id,
   'payload',jsonb_build_object('source_event_id',e.id,
     'source_observation_id',e.observation_id,'sha256',d.sha256,
     'engine_version',d.engine_version,'ocr',o.result))) AS envelope
FROM ocr_evidence d JOIN events e ON e.id=d.source_event_id
JOIN ocr_jobs o USING(sha256,engine_version);
