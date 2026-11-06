-- Every generation change (ingest, OCR, or prior context) restarts settling.
ALTER TABLE windows ADD COLUMN input_changed_at timestamptz NOT NULL DEFAULT now();
CREATE FUNCTION track_window_input() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.generation IS DISTINCT FROM OLD.generation THEN
    NEW.input_changed_at = statement_timestamp();
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER window_input_changed BEFORE UPDATE ON windows
FOR EACH ROW EXECUTE FUNCTION track_window_input();

-- Keep one durable slot per cadence and window. Prefer an existing canonical
-- key, then a live lease; fence retired duplicates so their workers cannot finish.
CREATE TEMP TABLE interpret_slots ON COMMIT DROP AS
SELECT j.id, w.start_at, w.end_at, w.generation,
 CASE WHEN j.output_key LIKE 'revision24:%' THEN 'revision24' ELSE 'interpret' END AS slot,
 row_number() OVER (PARTITION BY w.start_at, j.output_key LIKE 'revision24:%'
 ORDER BY (j.output_key IN (
   'interpret:' || to_char(w.start_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || ' UTC',
   'revision24:' || to_char(w.start_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || ' UTC')) DESC,
 (j.state='running') DESC, j.id DESC) AS rank
FROM jobs j JOIN windows w ON w.start_at=(j.payload->'window'->>'start')::timestamptz
WHERE j.kind='interpret' AND (j.output_key LIKE 'interpret:%' OR j.output_key LIKE 'revision24:%')
AND EXISTS (SELECT 1 FROM jobs pending WHERE pending.kind='interpret'
 AND pending.state IN ('ready','running','failed')
 AND (pending.payload->'window'->>'start')::timestamptz=w.start_at
 AND (pending.output_key LIKE 'revision24:%')=(j.output_key LIKE 'revision24:%'));
UPDATE jobs j SET state='done',lease_owner=NULL,lease_until=NULL
FROM interpret_slots s WHERE j.id=s.id AND s.rank>1 AND j.state IN ('ready','running','failed');
UPDATE jobs j SET
 output_key=s.slot || ':' || to_char(s.start_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || ' UTC',
 payload=jsonb_build_object('window',jsonb_build_object('start',s.start_at,'end',s.end_at),
   'generation',s.generation,'revision24',s.slot='revision24'),
 state=CASE WHEN j.state='running' THEN 'running' ELSE 'ready' END,
 attempts=CASE WHEN j.state='running' THEN j.attempts ELSE 0 END,
 request_version=j.request_version+1,
 available_at=greatest(j.available_at,s.end_at + CASE WHEN s.slot='revision24' THEN interval '24 hours' ELSE interval '0' END)
FROM interpret_slots s WHERE j.id=s.id AND s.rank=1;
CREATE INDEX revisions_input_identity ON revisions(start_at,input_generation);
