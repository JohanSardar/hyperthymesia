# Automatic GC is off

`serve` does not schedule `Store::gc()`. The only caller is the explicit
`hx-server gc` command, and it has never run on real data.

GC deletes published, unpinned `chunk` blobs whose row is over 30 days old. It
keeps `keep` blobs (including stills) and pinned blobs, and holds the row lock
that upload also takes while it unlinks each file.

Uploads are safe against it: a re-upload restores a missing file and restarts
the 30 days, so a 200 means the bytes are there and will stay.

Two things must be settled before it runs unattended:

- **References.** A new live record pointing at an old hash raises `refcount` but
  doesn't restart the 30 days, and GC never looks at references or age of use. It
  could delete bytes a new record needs. Active jobs also take no pins, although
  stills are `keep`, which protects normal OCR.
- **The owner's first run.** It permanently deletes every chunk over 30 days old
  in one pass, so turning it on is the owner's decision, not a side effect of an
  upgrade.

Tests for that change must cover an old hash getting a fresh reference, active
OCR, retention and pins, and a failure part way through a run.
