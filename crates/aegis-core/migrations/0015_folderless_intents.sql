-- Migration 0015: AI Folderless Mode (2.1, reshaped R6) visible intent-compiled filing rules (D1, K1).

CREATE TABLE IF NOT EXISTS folderless_rules (
    id                    TEXT PRIMARY KEY,
    name                  TEXT NOT NULL,
    intent_text           TEXT NOT NULL,
    compiled_by           TEXT NOT NULL DEFAULT 'deterministic',
    enabled               INTEGER NOT NULL DEFAULT 1,
    scope_root            TEXT,
    match_kinds_json      TEXT NOT NULL DEFAULT '[]',
    match_exts_json       TEXT NOT NULL DEFAULT '[]',
    match_categories_json TEXT NOT NULL DEFAULT '[]',
    match_doc_types_json  TEXT NOT NULL DEFAULT '[]',
    match_keywords_json   TEXT NOT NULL DEFAULT '[]',
    match_orgs_json       TEXT NOT NULL DEFAULT '[]',
    min_age_days          INTEGER,
    dest_root             TEXT NOT NULL,
    path_template         TEXT NOT NULL,
    rename_template       TEXT,
    approvals             INTEGER NOT NULL DEFAULT 0,
    rejections            INTEGER NOT NULL DEFAULT 0,
    state                 TEXT NOT NULL DEFAULT 'asking',
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_folderless_rules_enabled
    ON folderless_rules(enabled, updated_at DESC);
