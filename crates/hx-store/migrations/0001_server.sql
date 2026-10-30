CREATE TABLE events (
 id bigserial PRIMARY KEY, observation_id text NOT NULL, connector_instance text NOT NULL,
 source_key text NOT NULL, revision text NOT NULL, kind text NOT NULL,
 source_start timestamptz NOT NULL, source_end timestamptz, observed_at timestamptz NOT NULL,
 device text, sensitivity text NOT NULL, envelope jsonb NOT NULL,
 received_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(connector_instance,source_key,revision)
);
CREATE INDEX events_time ON events(source_start,source_end);
CREATE TABLE normalized (
 event_id bigint PRIMARY KEY REFERENCES events(id), normalizer_version text NOT NULL,
 payload jsonb NOT NULL, search_text text NOT NULL DEFAULT '',
 search_vector tsvector GENERATED ALWAYS AS (to_tsvector('simple', search_text)) STORED
);
CREATE INDEX normalized_fts ON normalized USING gin(search_vector);
CREATE TABLE quarantine (event_id bigint PRIMARY KEY REFERENCES events(id), reason text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE blobs (
 sha256 text PRIMARY KEY CHECK (sha256 ~ '^[0-9a-f]{64}$'), bytes bigint,
 published boolean NOT NULL DEFAULT false, evidence_expired boolean NOT NULL DEFAULT false,
 retention_class text NOT NULL DEFAULT 'chunk', refcount bigint NOT NULL DEFAULT 0,
 pins bigint NOT NULL DEFAULT 0 CHECK(pins>=0), created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE event_blobs(event_id bigint REFERENCES events(id), sha256 text REFERENCES blobs(sha256), PRIMARY KEY(event_id,sha256));
CREATE TABLE windows (
 start_at timestamptz PRIMARY KEY, end_at timestamptz NOT NULL, generation bigint NOT NULL DEFAULT 1,
 dirty boolean NOT NULL DEFAULT true, timeline jsonb, compiler_version text, compiled_generation bigint,
 canonical_revision bigint
);
CREATE TABLE revisions (
 id bigserial PRIMARY KEY, start_at timestamptz NOT NULL REFERENCES windows(start_at),
 input_generation bigint NOT NULL, interpretation jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE windows ADD FOREIGN KEY (canonical_revision) REFERENCES revisions(id);
CREATE TABLE watermarks (connector_instance text PRIMARY KEY, observed_through timestamptz NOT NULL, sequence numeric(20));
CREATE TABLE device_tokens (hash text PRIMARY KEY, name text NOT NULL, prefixes text[] NOT NULL, scopes text[] NOT NULL, revoked boolean NOT NULL DEFAULT false);
CREATE TABLE entity_assertions (id bigserial PRIMARY KEY, entity text NOT NULL, assertion jsonb NOT NULL, evidence bigint[] NOT NULL, sensitivity text NOT NULL DEFAULT 'restricted');
CREATE TABLE jobs (
 id bigserial PRIMARY KEY, output_key text NOT NULL UNIQUE, kind text NOT NULL, payload jsonb NOT NULL,
 queue text NOT NULL DEFAULT 'interactive' CHECK(queue IN ('interactive','backfill')),
 state text NOT NULL DEFAULT 'ready', available_at timestamptz NOT NULL DEFAULT now(),
 lease_until timestamptz, lease_owner text, attempts integer NOT NULL DEFAULT 0,
 max_attempts integer NOT NULL DEFAULT 5, last_error text
);
CREATE INDEX jobs_ready ON jobs(state,available_at);
