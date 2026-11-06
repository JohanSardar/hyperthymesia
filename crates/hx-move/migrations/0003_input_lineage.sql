ALTER TABLE revisions ADD COLUMN input_namespaces text[] NOT NULL DEFAULT '{}';
