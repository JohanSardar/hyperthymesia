# Automatic GC is disabled

N1 (#35) deliberately does not schedule `Store::gc()` in `serve`. The issue
plan reports that GC has never run in production; the only production call in
`31810ed` is the explicit `hx-server gc` command. No owner data was inspected.

The implementation in `crates/hx-store/src/lib.rs` selects published, unpinned
`chunk` blobs whose **blob row** is over 30 days old. It preserves `keep` blobs
(including stills) and pins, and locks each selected blob row. Upload and ingest
also lock that row, so they serialize with GC. These protections are insufficient
to turn on unattended deletion:

- A new live chunk record referencing an old hash increments `refcount` without
  refreshing `created_at`. Re-uploading expired bytes also keeps that timestamp.
  GC can therefore immediately expire bytes just acknowledged to a concurrent
  uploader or needed by a new live record. It never considers reference age.
- The selection does not check live references or OCR jobs. Stills currently
  promote the shared hash to `keep`, which protects normal OCR discovery, but
  active jobs do not acquire pins (the only `pin` callers are tests). There is
  no general active-reader/job protection.
- It unlinks real files **before** committing the database transaction. Failure
  on a later file, database update, or commit rolls back database changes without
  restoring already deleted bytes. Those rows can still say `published=true`;
  a retry upload then returns early without repairing the missing file.

Before enabling daily GC, the store owner must define reference-aware retention
and upload acknowledgement semantics, protect active jobs, and make deletion
recoverable across filesystem/database failures. Synthetic tests must cover an
old hash receiving a fresh reference or upload, active OCR, retention/pins, and
partial deletion/commit failure. N1 does not change `hx-store` or `hx-jobs`, and
the existing manual GC command remains available with these limitations.
