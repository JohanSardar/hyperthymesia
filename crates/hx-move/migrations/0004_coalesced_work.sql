ALTER TABLE jobs ADD COLUMN request_version bigint NOT NULL DEFAULT 1;
ALTER TABLE jobs ADD COLUMN leased_version bigint NOT NULL DEFAULT 0;

-- Retire the per-event/per-generation backlog, including its old lease tokens.
UPDATE jobs SET state='done',lease_owner=NULL,lease_until=NULL
WHERE kind IN ('normalize','compile') AND state IN ('ready','running','failed');
INSERT INTO jobs(output_key,kind,payload)
SELECT 'compile:' || to_char(start_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || ' UTC','compile',
 jsonb_build_object('window',jsonb_build_object('start',start_at,'end',end_at),'generation',generation)
FROM windows WHERE dirty
ON CONFLICT DO NOTHING;
INSERT INTO jobs(output_key,kind,payload)
SELECT 'normalize:' || to_char(w.start_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || ' UTC','normalize',
 jsonb_build_object('window',jsonb_build_object('start',w.start_at,'end',w.end_at),'generation',w.generation)
FROM windows w WHERE EXISTS (
 SELECT 1 FROM events e WHERE e.source_start>=w.start_at AND e.source_start<w.end_at
 AND NOT EXISTS(SELECT 1 FROM normalized n WHERE n.event_id=e.id)
 AND NOT EXISTS(SELECT 1 FROM quarantine q WHERE q.event_id=e.id))
ON CONFLICT DO NOTHING;
