ALTER TABLE windows ADD COLUMN timeline_hash text;
ALTER TABLE windows ADD COLUMN revision24_finished boolean NOT NULL DEFAULT false;
-- Existing history has already passed its one automatic 24h opportunity.
UPDATE windows SET revision24_finished=true WHERE end_at+interval '24 hours'<=now();
-- One globally admitted sweep window and at most one start per UTC day.
CREATE TABLE daily_sweeps (
 day date PRIMARY KEY,
 start_at timestamptz NOT NULL REFERENCES windows(start_at),
 generation bigint NOT NULL,
 started boolean NOT NULL DEFAULT false
);
