-- Semantic Version Tracking (Capability #7 / 3.4): persisted chain preferences and non-sensitive diff summaries (D1).
CREATE TABLE IF NOT EXISTS version_chain_prefs (
    chain_key             TEXT PRIMARY KEY,
    pinned_latest_file_id TEXT,
    dismissed             INTEGER NOT NULL DEFAULT 0,
    dismissed_at          INTEGER,
    updated_at            INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS version_diff_summaries (
    older_file_id TEXT NOT NULL,
    newer_file_id TEXT NOT NULL,
    older_hash    BLOB,
    newer_hash    BLOB,
    model_id      TEXT NOT NULL,
    summary       TEXT NOT NULL,
    badges_json   TEXT NOT NULL DEFAULT '[]',
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (older_file_id, newer_file_id, model_id)
);

CREATE INDEX IF NOT EXISTS idx_version_diff_summaries_pair
    ON version_diff_summaries(older_file_id, newer_file_id);
