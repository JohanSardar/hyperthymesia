-- Owner-authored, credential-free checks run before setup can buy model work.
CREATE TABLE source_checks (
    source_id TEXT PRIMARY KEY REFERENCES sources(id),
    request TEXT NOT NULL CHECK(json_valid(request))
);
