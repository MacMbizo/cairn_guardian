-- Context layer (spec 2026-11-22-context-aware-filing-design §1A, §14): collections of files that
-- belong together (projects first), where each one lives, and the owner's answers about them.
-- The legacy `projects` table stays untouched: its NOT NULL `domain` has other semantics.
CREATE TABLE collections (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    project_kind TEXT,
    confidence REAL NOT NULL,
    state TEXT NOT NULL,
    evidence_json TEXT NOT NULL DEFAULT '[]',
    sub_projects_json TEXT NOT NULL DEFAULT '[]',
    computed_at INTEGER NOT NULL
);

CREATE TABLE collection_locations (
    id TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    volume_id TEXT NOT NULL,
    root_path TEXT NOT NULL,
    relationship TEXT NOT NULL,
    files INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    newest_mtime INTEGER NOT NULL,
    shared_files INTEGER NOT NULL,
    compared_by TEXT NOT NULL,
    is_repo INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    UNIQUE (volume_id, root_path)
);
CREATE INDEX idx_collection_locations_fingerprint ON collection_locations(fingerprint);
CREATE INDEX idx_collection_locations_collection ON collection_locations(collection_id);

-- Owner's answers, keyed by location fingerprints so they survive renames and drive letters.
CREATE TABLE context_decisions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    fingerprint_a TEXT NOT NULL,
    fingerprint_b TEXT,
    value TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX idx_context_decisions_a ON context_decisions(fingerprint_a);
