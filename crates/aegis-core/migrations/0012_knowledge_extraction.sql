-- Automatic Knowledge Extraction (Capability #8 / 2.2): canonical entity aliases,
-- provisional K1 review inbox with undo snapshots, and per-file extraction watermarks (D1, R7).
CREATE TABLE IF NOT EXISTS entity_aliases (
    normalized_key TEXT NOT NULL,
    entity_type    TEXT NOT NULL,
    entity_id      TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    alias_text     TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (entity_type, normalized_key)
);
CREATE INDEX IF NOT EXISTS idx_entity_aliases_entity ON entity_aliases(entity_id);

CREATE TABLE IF NOT EXISTS knowledge_proposals (
    id                            TEXT PRIMARY KEY,
    dedupe_key                    TEXT NOT NULL UNIQUE,
    entity_type                   TEXT NOT NULL,
    name                          TEXT NOT NULL,
    summary                       TEXT NOT NULL DEFAULT '',
    attrs_json                    TEXT NOT NULL DEFAULT '{}',
    source_file_id                TEXT REFERENCES files(id) ON DELETE SET NULL,
    source_path                   TEXT NOT NULL,
    source_filename               TEXT NOT NULL,
    source_kind                   TEXT NOT NULL DEFAULT 'document',
    evidence_quote                TEXT NOT NULL DEFAULT '',
    extractor                     TEXT NOT NULL DEFAULT 'deterministic',
    confidence                    REAL NOT NULL DEFAULT 0.80,
    contains_sensitive            INTEGER NOT NULL DEFAULT 0,
    matched_entity_id             TEXT REFERENCES entities(id) ON DELETE SET NULL,
    status                        TEXT NOT NULL DEFAULT 'pending',
    committed_entity_id           TEXT REFERENCES entities(id) ON DELETE SET NULL,
    committed_created_new         INTEGER NOT NULL DEFAULT 0,
    committed_rel_ids_json        TEXT NOT NULL DEFAULT '[]',
    previous_entity_snapshot_json TEXT,
    created_at                    INTEGER NOT NULL,
    reviewed_at                   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_knowledge_proposals_status ON knowledge_proposals(status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_knowledge_proposals_file ON knowledge_proposals(source_file_id);

CREATE TABLE IF NOT EXISTS knowledge_file_runs (
    file_id      TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    content_hash BLOB,
    mtime        INTEGER NOT NULL,
    extractor    TEXT NOT NULL,
    extracted_at INTEGER NOT NULL
);
