-- B4 Data & Vector Architecture Specification (v2.3.1) — Initial SQLite Schema

CREATE TABLE volumes (
  id TEXT PRIMARY KEY,
  mount_point TEXT NOT NULL,
  fs_type TEXT NOT NULL,                     -- ntfs|apfs|ext4|exfat|smb
  is_removable INTEGER NOT NULL DEFAULT 0,
  is_readonly INTEGER NOT NULL DEFAULT 0,
  has_stable_inode INTEGER NOT NULL DEFAULT 1, -- 0 for exfat / non-persistent smb
  case_sensitive INTEGER NOT NULL DEFAULT 0,   -- 1 for ext4, 0 for default ntfs/apfs
  quarantine_dir TEXT,                       -- "<mount>/.aegis-quarantine"
  status TEXT NOT NULL DEFAULT 'online',     -- online|offline|degraded
  last_seen INTEGER NOT NULL
);

CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT,
  domain TEXT NOT NULL DEFAULT 'general',
  repo_url TEXT,
  health_score REAL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE atomic_units (
  id TEXT PRIMARY KEY,                       -- UUIDv7
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  root_path TEXT NOT NULL,
  unit_type TEXT NOT NULL,                   -- git_repo|node_pkg|rust_crate|python_pkg|mac_bundle|latex_proj|custom
  marker_file TEXT NOT NULL,
  project_id TEXT REFERENCES projects(id),
  allow_split_override INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_atomic_units_path ON atomic_units(volume_id, root_path);

CREATE TABLE files (
  id TEXT PRIMARY KEY,
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  file_index BLOB,                           -- 8-byte LE u64 or 16-byte Windows FILE_ID_128; NULL for archive:// or has_stable_inode=0
  nlink INTEGER NOT NULL DEFAULT 1,
  container_file_id TEXT REFERENCES files(id) ON DELETE CASCADE,
  virtual_uri TEXT,                          -- "archive://<container_id>/docs/report.pdf"
  atomic_unit_id TEXT REFERENCES atomic_units(id) ON DELETE SET NULL,
  unit_split_override INTEGER NOT NULL DEFAULT 0,
  path TEXT NOT NULL,                        -- original path preserved even while state='quarantine'
  path_hash BLOB NOT NULL,                   -- BLAKE3(canonical_normalized_path)
  name TEXT NOT NULL,
  ext TEXT,
  mime TEXT,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  ctime INTEGER NOT NULL,
  blake3 BLOB,
  simhash_text BLOB,
  dhash_image BLOB,
  cloud_state TEXT NOT NULL DEFAULT 'local', -- local|placeholder_remote|syncing
  state TEXT NOT NULL DEFAULT 'active',      -- active|warm|cold|archive|quarantine|missing
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  classification TEXT,
  confidence REAL,
  sensitivity TEXT NOT NULL DEFAULT 'internal', -- public|internal|confidential|secret
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  access_count INTEGER NOT NULL DEFAULT 0,
  last_access INTEGER,
  deleted_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK ((container_file_id IS NULL)
      OR (container_file_id IS NOT NULL AND virtual_uri IS NOT NULL))
);

CREATE UNIQUE INDEX idx_files_os_path
  ON files(volume_id, file_index, path_hash)
  WHERE file_index IS NOT NULL AND deleted_at IS NULL;
CREATE UNIQUE INDEX idx_files_live_path
  ON files(volume_id, path_hash)
  WHERE container_file_id IS NULL AND deleted_at IS NULL AND state NOT IN ('quarantine','missing');
CREATE INDEX idx_files_inode
  ON files(volume_id, file_index)
  WHERE file_index IS NOT NULL AND deleted_at IS NULL;
CREATE UNIQUE INDEX idx_files_virtual
  ON files(container_file_id, virtual_uri)
  WHERE container_file_id IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_files_path_hash ON files(path_hash);
CREATE INDEX idx_files_blake3 ON files(blake3) WHERE blake3 IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_files_project ON files(project_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_files_atomic_unit ON files(atomic_unit_id);
CREATE INDEX idx_files_state ON files(state) WHERE deleted_at IS NULL;

-- Tracked build-cache directories (FR-011.2 Disk Rescue; interior files produce 0 rows in files/chunks)
CREATE TABLE cache_dirs (
  id TEXT PRIMARY KEY,
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  atomic_unit_id TEXT REFERENCES atomic_units(id) ON DELETE SET NULL,
  path TEXT NOT NULL,
  path_hash BLOB NOT NULL,
  cache_kind TEXT NOT NULL,                  -- node_modules|rust_target|python_venv|gradle|build_cache
  size_bytes INTEGER NOT NULL DEFAULT 0,
  file_count INTEGER NOT NULL DEFAULT 0,
  source_max_mtime INTEGER,
  state TEXT NOT NULL DEFAULT 'active',      -- active|quarantine|missing
  last_seen INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_cache_dirs_path ON cache_dirs(volume_id, path_hash) WHERE state = 'active';

CREATE TABLE chunks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  chunk_uuid TEXT NOT NULL UNIQUE,
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  text TEXT NOT NULL,
  token_count INTEGER NOT NULL,
  is_summary INTEGER NOT NULL DEFAULT 0,
  embedding_model TEXT,
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  UNIQUE(file_id, seq)
);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
  text,
  content='chunks',
  content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER chunks_au AFTER UPDATE OF text ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE entities (
  id TEXT PRIMARY KEY,
  entity_type TEXT NOT NULL,
  name TEXT NOT NULL,
  attrs_json TEXT NOT NULL DEFAULT '{}',
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE relationships (
  id TEXT PRIMARY KEY,
  src_id TEXT NOT NULL,
  src_kind TEXT NOT NULL,
  dst_id TEXT NOT NULL,
  dst_kind TEXT NOT NULL,
  rel_type TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1.0,
  evidence_json TEXT NOT NULL DEFAULT '[]',
  valid_from INTEGER NOT NULL,
  valid_to INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_rel_src ON relationships(src_id, rel_type);
CREATE INDEX idx_rel_dst ON relationships(dst_id, rel_type);

CREATE TABLE action_batches (
  id TEXT PRIMARY KEY,
  origin TEXT NOT NULL,                      -- simulator|rules|dedupe|security|user_undo
  status TEXT NOT NULL,                      -- draft|approved|executing|executed|partial|rolled_back|failed
  total_actions INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER
);

CREATE TABLE actions (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL REFERENCES action_batches(id) ON DELETE CASCADE,
  agent TEXT,                                -- NULL = user/deterministic rule
  action_type TEXT NOT NULL,                 -- move|copy|rename|quarantine|purge|archive|restore
  target_kind TEXT NOT NULL DEFAULT 'file',  -- file|dir|atomic_unit
  file_id TEXT REFERENCES files(id),
  atomic_unit_id TEXT REFERENCES atomic_units(id),
  from_path TEXT NOT NULL,
  to_path TEXT,
  pre_blake3 BLOB,
  post_blake3 BLOB,
  pre_state_json TEXT,
  post_state_json TEXT,
  reason TEXT NOT NULL,
  confidence REAL NOT NULL,
  evidence_json TEXT NOT NULL,
  status TEXT NOT NULL,                      -- proposed|approved|executed|failed|skipped|rolled_back|rejected|superseded|expired|unrecoverable_missing
  rollback_conflict TEXT,                    -- NULL|modified|missing_target|collision
  error_message TEXT,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER,
  CHECK ((target_kind = 'file' AND file_id IS NOT NULL)
      OR (target_kind = 'atomic_unit' AND atomic_unit_id IS NOT NULL)
      OR (target_kind = 'dir'))
);
CREATE INDEX idx_actions_batch ON actions(batch_id, status);
CREATE UNIQUE INDEX idx_actions_file_status ON actions(file_id)
  WHERE file_id IS NOT NULL AND status IN ('proposed','approved');

CREATE TABLE quarantine_entries (
  id TEXT PRIMARY KEY,
  action_id TEXT NOT NULL REFERENCES actions(id),
  target_kind TEXT NOT NULL DEFAULT 'file',  -- file|dir|atomic_unit
  file_id TEXT REFERENCES files(id),
  atomic_unit_id TEXT REFERENCES atomic_units(id),
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  original_path TEXT NOT NULL,
  quarantine_path TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  blake3 BLOB,
  quarantined_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  restored_at INTEGER,
  purged_at INTEGER
);
CREATE INDEX idx_quarantine_expiry ON quarantine_entries(expires_at)
  WHERE restored_at IS NULL AND purged_at IS NULL;

CREATE TABLE audit_log (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  detail_json TEXT NOT NULL,
  prev_hash BLOB NOT NULL,
  hash BLOB NOT NULL
);

CREATE TABLE scan_jobs (
  id TEXT PRIMARY KEY,
  root_path TEXT NOT NULL,
  tier INTEGER NOT NULL,
  status TEXT NOT NULL,
  files_seen INTEGER NOT NULL DEFAULT 0,
  files_indexed INTEGER NOT NULL DEFAULT 0,
  files_skipped_perm INTEGER NOT NULL DEFAULT 0,
  files_skipped_cloud INTEGER NOT NULL DEFAULT 0,
  checkpoint_json TEXT,
  errors_json TEXT NOT NULL DEFAULT '[]',
  started_at INTEGER NOT NULL,
  finished_at INTEGER
);
CREATE INDEX idx_scan_jobs_root ON scan_jobs(root_path, status);

CREATE TABLE rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  scope_json TEXT NOT NULL,
  condition_json TEXT NOT NULL,
  action_json TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 100,
  enabled INTEGER NOT NULL DEFAULT 1,
  source TEXT NOT NULL,                      -- builtin|user|graduated_proposal|plugin
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE routing_exemplars (
  id TEXT PRIMARY KEY,
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  dest_folder TEXT NOT NULL,
  decision TEXT NOT NULL,                    -- approved|rejected
  features_json TEXT NOT NULL,
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE security_findings (
  id TEXT PRIMARY KEY,
  secret_fingerprint BLOB NOT NULL,
  finding_type TEXT NOT NULL,
  base_severity TEXT NOT NULL,
  context_risk_score REAL NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',       -- open|false_positive|accepted_risk|vaulted|resolved
  resolved_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_sec_fingerprint ON security_findings(secret_fingerprint);

CREATE TABLE security_finding_locations (
  finding_id TEXT NOT NULL REFERENCES security_findings(id) ON DELETE CASCADE,
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  line_number INTEGER NOT NULL DEFAULT 0,
  redacted_snippet TEXT NOT NULL,
  PRIMARY KEY (finding_id, file_id, line_number)
);

CREATE TABLE outbox_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  aggregate_type TEXT NOT NULL,              -- chunk|entity|project|exemplar|file_payload
  aggregate_id TEXT NOT NULL,
  op TEXT NOT NULL,                          -- upsert|delete
  payload_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  processed_at INTEGER
);
CREATE INDEX idx_outbox_pending ON outbox_events(seq) WHERE processed_at IS NULL;

CREATE TABLE settings (
  key TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'global',
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (key, scope)
);
