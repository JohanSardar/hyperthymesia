-- 0002_sources.sql is reserved. Migration versions need not be contiguous.
CREATE TABLE pair_codes (
 hash TEXT PRIMARY KEY CHECK(length(hash)=64 AND hash NOT GLOB '*[^0-9a-f]*'),
 name TEXT NOT NULL,
 expires_at INTEGER NOT NULL,
 used_at INTEGER,
 revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1)),
 token_hash TEXT REFERENCES device_tokens(hash),
 CHECK((used_at IS NULL AND token_hash IS NULL) OR (used_at IS NOT NULL AND token_hash IS NOT NULL))
) STRICT;
