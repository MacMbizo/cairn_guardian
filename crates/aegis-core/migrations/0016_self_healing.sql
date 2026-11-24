-- Capability #12: Self-Healing Workspace (`4.1`)
-- Stores reversible K1 text-patch snapshots, external/scanner rename history for link healing,
-- and persistent issue dismissals inside SQLCipher-encrypted aegis.db.

CREATE TABLE heal_patch_payloads (
  action_id TEXT PRIMARY KEY REFERENCES actions(id) ON DELETE CASCADE,
  original_content TEXT NOT NULL,
  patched_content TEXT NOT NULL,
  original_blake3 BLOB NOT NULL,
  patch_kind TEXT NOT NULL,                  -- links | tags | mixed
  summary_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL
);

CREATE TABLE heal_move_log (
  id TEXT PRIMARY KEY,
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  volume_id TEXT,
  old_path TEXT NOT NULL,
  new_path TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'scanner_rename',
  detected_at INTEGER NOT NULL
);
CREATE INDEX idx_heal_move_log_old ON heal_move_log(old_path);
CREATE INDEX idx_heal_move_log_file ON heal_move_log(file_id, detected_at);

CREATE TABLE heal_dismissals (
  id TEXT PRIMARY KEY,
  issue_key TEXT NOT NULL UNIQUE,
  issue_kind TEXT NOT NULL,                  -- broken_link | tag_weed | rule_path
  note TEXT,
  created_at INTEGER NOT NULL
);
