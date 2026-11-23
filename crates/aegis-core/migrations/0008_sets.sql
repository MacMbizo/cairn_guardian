-- Sets and photo subjects (spec 2026-11-22-context-aware-filing-design §10, §14).
-- Collection-level fingerprint for loose-file sets (`set:<normalized subject>`).
ALTER TABLE collections ADD COLUMN fingerprint TEXT;

-- File-level members for collections that consist of loose files (design sets and photo
-- subjects) rather than whole folders. Kept separate from `collection_locations` so multiple
-- sets in the same folder (`Pictures`, `Downloads`) never clash on UNIQUE(volume_id, root_path)
-- and never masquerade as folder-owned projects.
CREATE TABLE collection_members (
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL,
    volume_id TEXT NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    PRIMARY KEY (collection_id, file_id)
);
CREATE INDEX idx_collection_members_file ON collection_members(file_id);
CREATE INDEX idx_collection_members_volume ON collection_members(volume_id);
