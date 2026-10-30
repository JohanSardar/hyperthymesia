ALTER TABLE revisions ADD COLUMN output_key text UNIQUE;
CREATE FUNCTION immutable_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'evidence and interpretation revisions are immutable';
END;
$$;
CREATE TRIGGER immutable_events BEFORE UPDATE OR DELETE ON events FOR EACH ROW EXECUTE FUNCTION immutable_evidence();
CREATE TRIGGER immutable_revisions BEFORE UPDATE OR DELETE ON revisions FOR EACH ROW EXECUTE FUNCTION immutable_evidence();
