-- 0002 is reserved for device OCR. Migration versions need not be contiguous.
CREATE TABLE pair_codes (
 hash TEXT PRIMARY KEY CHECK(length(hash)=64 AND hash NOT GLOB '*[^0-9a-f]*'),
 name TEXT NOT NULL,
 expires_at INTEGER NOT NULL,
 used_at INTEGER,
 token_hash TEXT REFERENCES device_tokens(hash),
 CHECK((used_at IS NULL AND token_hash IS NULL) OR (used_at IS NOT NULL AND token_hash IS NOT NULL))
) STRICT;
