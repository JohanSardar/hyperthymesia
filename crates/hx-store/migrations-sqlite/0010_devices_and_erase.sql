-- Owner names for recording devices. Evidence keeps the name each device
-- reported; `merged_into` shows a duplicate under another device everywhere.
CREATE TABLE device_names (
 device TEXT PRIMARY KEY CHECK(length(device) BETWEEN 1 AND 255),
 name TEXT CHECK(name IS NULL OR length(name) BETWEEN 1 AND 120),
 merged_into TEXT REFERENCES device_names(device),
 CHECK(merged_into IS NULL OR merged_into<>device)
) STRICT;
CREATE INDEX events_device ON events(device,source_start) WHERE device IS NOT NULL;
-- Time ranges the owner deleted. Records from these times are not stored again,
-- so a device's queued uploads cannot bring them back. Restoring a backup taken
-- before the delete restores this table as it was, too.
CREATE TABLE erased_ranges (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 start_at INTEGER NOT NULL, end_at INTEGER NOT NULL CHECK(end_at > start_at),
 created_at INTEGER NOT NULL
) STRICT;
