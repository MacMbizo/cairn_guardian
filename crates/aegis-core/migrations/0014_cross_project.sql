-- Migration 0014: Cross-Project Intelligence (3.3) triage & dismissal state (D1, K1, R5).

CREATE TABLE IF NOT EXISTS cross_project_insight_state (
    insight_key TEXT PRIMARY KEY,
    status      TEXT NOT NULL DEFAULT 'open',
    note        TEXT NOT NULL DEFAULT '',
    updated_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cross_project_insight_status
    ON cross_project_insight_state(status, updated_at DESC);
