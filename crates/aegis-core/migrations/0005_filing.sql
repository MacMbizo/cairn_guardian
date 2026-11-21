-- Auto-filing (spec 2026-11-21-auto-filing-design): what Aegis read and concluded about each
-- file, where files used to be, the structure chosen per drive, and learned filing rules.
CREATE TABLE file_meta (
    file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    category TEXT,
    doc_type TEXT,
    who TEXT,
    doc_date TEXT,
    date_source TEXT,
    title TEXT,
    exif_taken_at INTEGER,
    camera TEXT,
    artist TEXT,
    album TEXT,
    confidence REAL NOT NULL DEFAULT 0,
    evidence_json TEXT NOT NULL DEFAULT '[]',
    read_at INTEGER,
    read_error TEXT
);

CREATE TABLE file_history (
    id TEXT PRIMARY KEY,
    file_id TEXT,
    old_path TEXT NOT NULL,
    new_path TEXT NOT NULL,
    batch_id TEXT,
    changed_at INTEGER NOT NULL
);
CREATE INDEX idx_file_history_old ON file_history(old_path);
CREATE INDEX idx_file_history_file ON file_history(file_id);

CREATE TABLE filing_blueprints (
    scope_key TEXT PRIMARY KEY,
    choice TEXT NOT NULL,
    chosen_at INTEGER NOT NULL
);

CREATE TABLE filing_rules (
    id TEXT PRIMARY KEY,
    scope_key TEXT NOT NULL,
    rule_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    category TEXT,
    who TEXT,
    dest_template TEXT NOT NULL,
    rename INTEGER NOT NULL,
    approvals INTEGER NOT NULL DEFAULT 0,
    rejections INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'asking',
    approved_scopes_json TEXT NOT NULL DEFAULT '[]',
    updated_at INTEGER NOT NULL,
    UNIQUE (scope_key, rule_key)
);

CREATE TABLE filing_decisions (
    id TEXT PRIMARY KEY,
    rule_id TEXT,
    file_id TEXT,
    batch_id TEXT,
    outcome TEXT NOT NULL,
    at INTEGER NOT NULL
);
