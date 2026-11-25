-- AI Chief Digital Officer Agent (Capability #14 / 5.2, D1, K1, R4, R5, R7, R11):
-- persisted recommendation triage, single-click K1 ratification undo snapshots, and
-- grounded executive briefing history inside SQLCipher `aegis.db`.
CREATE TABLE IF NOT EXISTS cdo_recommendation_state (
    rec_key    TEXT PRIMARY KEY,
    status     TEXT NOT NULL DEFAULT 'open',
    note       TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cdo_ratifications (
    id              TEXT PRIMARY KEY,
    rec_key         TEXT NOT NULL,
    rec_title       TEXT NOT NULL,
    action_kind     TEXT NOT NULL,
    target_ref      TEXT NOT NULL,
    previous_json   TEXT,
    applied_summary TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'applied',
    created_at      INTEGER NOT NULL,
    undone_at       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_cdo_ratifications_created ON cdo_ratifications(created_at DESC);

CREATE TABLE IF NOT EXISTS cdo_briefing_snapshots (
    id               TEXT PRIMARY KEY,
    horizon_days     INTEGER NOT NULL,
    executive_score  REAL NOT NULL,
    headline         TEXT NOT NULL,
    summary_markdown TEXT NOT NULL,
    citations_json   TEXT NOT NULL DEFAULT '[]',
    model_id         TEXT NOT NULL,
    model_name       TEXT NOT NULL,
    backend          TEXT NOT NULL,
    created_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cdo_briefings_created ON cdo_briefing_snapshots(created_at DESC);
