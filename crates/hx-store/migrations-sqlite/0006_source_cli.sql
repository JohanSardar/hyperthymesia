-- Owner-bound identity, separate from model-authored request recipes.
CREATE TABLE source_cli (
    source_id TEXT PRIMARY KEY REFERENCES sources(id),
    command TEXT NOT NULL,
    executable TEXT NOT NULL,
    home TEXT NOT NULL
) STRICT;
