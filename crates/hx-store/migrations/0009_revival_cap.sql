-- Retry rounds belong to the data version, not to scheduler cycles or leases.
ALTER TABLE jobs ADD COLUMN revivals integer NOT NULL DEFAULT 0;
