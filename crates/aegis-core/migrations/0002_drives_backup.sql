-- Drives & Duplicates (spec 2026-11-21 §8): drive identity, fingerprints, dupe decisions, backup mirror.

ALTER TABLE volumes ADD COLUMN passport_id TEXT;
ALTER TABLE volumes ADD COLUMN guid_path TEXT;
ALTER TABLE volumes ADD COLUMN label TEXT;
ALTER TABLE volumes ADD COLUMN serial TEXT;
ALTER TABLE volumes ADD COLUMN total_bytes INTEGER;
ALTER TABLE volumes ADD COLUMN role TEXT NOT NULL DEFAULT 'unassigned'
  CHECK (role IN ('unassigned','general','backup'));
ALTER TABLE volumes ADD COLUMN usn_journal_id TEXT;
ALTER TABLE volumes ADD COLUMN usn_next INTEGER;
ALTER TABLE volumes ADD COLUMN last_catalog_complete_at INTEGER;
CREATE UNIQUE INDEX idx_volumes_passport ON volumes(passport_id) WHERE passport_id IS NOT NULL;

ALTER TABLE files ADD COLUMN quick_hash BLOB;
CREATE INDEX idx_files_size_live ON files(size) WHERE deleted_at IS NULL;
CREATE INDEX idx_files_quick_hash ON files(size, quick_hash) WHERE quick_hash IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE actions ADD COLUMN survivor_file_id TEXT REFERENCES files(id);

CREATE TABLE dupe_decisions (
  id TEXT PRIMARY KEY,
  a_passport TEXT NOT NULL,
  a_rel_path TEXT NOT NULL,
  b_passport TEXT NOT NULL,
  b_rel_path TEXT NOT NULL,
  decision TEXT NOT NULL CHECK (decision IN ('backup','keep_both','delete')),
  decided_at INTEGER NOT NULL,
  undone_at INTEGER
);
CREATE INDEX idx_dupe_decisions_pair ON dupe_decisions(a_passport, a_rel_path, b_passport, b_rel_path)
  WHERE undone_at IS NULL;

CREATE TABLE backup_sources (
  id TEXT PRIMARY KEY,
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  rel_path TEXT NOT NULL,
  tier INTEGER NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0,
  added_at INTEGER NOT NULL,
  removed_at INTEGER
);
CREATE UNIQUE INDEX idx_backup_sources_live ON backup_sources(volume_id, rel_path) WHERE removed_at IS NULL;

CREATE TABLE backup_copies (
  id TEXT PRIMARY KEY,
  source_file_id TEXT REFERENCES files(id),
  backup_volume_id TEXT NOT NULL REFERENCES volumes(id),
  backup_rel_path TEXT NOT NULL,
  source_blake3 BLOB NOT NULL,
  copied_at INTEGER NOT NULL,
  last_verified_at INTEGER
);
CREATE UNIQUE INDEX idx_backup_copies_path ON backup_copies(backup_volume_id, backup_rel_path);
CREATE INDEX idx_backup_copies_source ON backup_copies(source_file_id);

CREATE TABLE backup_versions (
  id TEXT PRIMARY KEY,
  backup_volume_id TEXT NOT NULL REFERENCES volumes(id),
  rel_path TEXT NOT NULL,
  original_rel_path TEXT NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('changed','deleted','uncovered')),
  size_bytes INTEGER NOT NULL DEFAULT 0,
  moved_at INTEGER NOT NULL,
  prune_after INTEGER NOT NULL,
  pruned_at INTEGER
);
CREATE INDEX idx_backup_versions_prune ON backup_versions(backup_volume_id, prune_after) WHERE pruned_at IS NULL;

CREATE TABLE standing_approvals (
  id TEXT PRIMARY KEY,
  scope TEXT NOT NULL,
  volume_passport TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  revoked_at INTEGER,
  revoke_reason TEXT
);

CREATE TABLE backup_jobs (
  id TEXT PRIMARY KEY,
  backup_volume_id TEXT NOT NULL REFERENCES volumes(id),
  authority TEXT NOT NULL,
  status TEXT NOT NULL,          -- running|completed|interrupted|failed
  done_count INTEGER NOT NULL DEFAULT 0,
  total_count INTEGER NOT NULL DEFAULT 0,
  bytes_copied INTEGER NOT NULL DEFAULT 0,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  halted_reason TEXT
);
