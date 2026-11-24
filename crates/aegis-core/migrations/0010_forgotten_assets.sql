-- Capability #6 (4.3): Forgotten Asset Recovery visual header cache and per-file dismissals.

CREATE TABLE IF NOT EXISTS asset_meta (
    file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    asset_family TEXT NOT NULL,
    width INTEGER,
    height INTEGER,
    aspect_label TEXT,
    dominant_color TEXT,
    inspected_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_asset_meta_family ON asset_meta(asset_family);

CREATE TABLE IF NOT EXISTS asset_dismissals (
    file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    reason TEXT NOT NULL DEFAULT 'dismissed',
    dismissed_at INTEGER NOT NULL
);
