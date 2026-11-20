# B4 — Data & Vector Architecture Specification (v2.3.1)

**ID:** B4 · **Version:** 2.3.1 · **Status:** Approved for MVP Implementation
**Owner:** Staff Data & Storage Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B1_B2_Architecture.md` (v2.3), `B6_Threat_Model.md` (v2.4), `B8_ADRs.md` (v2.3), `C1_IPC_Action_Contracts.md` (v1.2), `C4_Policy_Engine_Confidence_Spec.md` (v1.1)

> **v2.3.1 changes vs v2.3:**
> 1. **FIX (`idx_actions_file_status`):** Changed from non-unique `(file_id, status)` to `CREATE UNIQUE INDEX idx_actions_file_status ON actions(file_id) WHERE file_id IS NOT NULL AND status IN ('proposed','approved');` so the one-live-proposal-per-file invariant is enforced by SQLite.
> 2. **ADD (`cache_dirs` & directory/unit targets):** Added `cache_dirs` table for build-cache accounting (`FR-011.2` Disk Rescue without writing interior cache files into `files`/`chunks`), plus `target_kind ('file'|'dir'|'atomic_unit')` and `atomic_unit_id` on `actions` and `quarantine_entries`.
> 3. **FIX (`volumes` & `files` identity):** Added `volumes.has_stable_inode` + `volumes.case_sensitive`; changed `files.file_index` to `BLOB` (8-byte LE `u64` or 16-byte Windows `FILE_ID_128`); added `idx_files_live_path`; documented canonical path normalization; rewrote §3.1 Rename-vs-Hardlink resolution to handle atomic-save replacements, case-only renames, inode reuse, and `exFAT`/`SMB` fallback.
> 4. **FIX (`security_finding_locations`, `audit_log`, `outbox_events`):** Changed `security_finding_locations` PK to `(finding_id, file_id, line_number)` (`NOT NULL DEFAULT 0`); specified canonical length-prefixed `BLAKE3` framing for `audit_log`; added `file_payload` to `outbox_events.aggregate_type`; mandated SQLite `files.state != 'quarantine'` as authoritative `search.query` post-filter; documented soft-delete/purge semantics and MIH/LSH near-dup indexing.

> **v2.3 changes vs v2.2:**
> 1. **BLOCKER FIX (`idx_files_os_path`):** Restored `path_hash` to unique key so hardlinks (`nlink > 1`) sharing `(volume_id, file_index)` do not throw `UNIQUE constraint failed`; added non-unique `idx_files_inode` + deterministic rename-vs-hardlink resolution rule (§3.1).
> 2. **ADD (`files.unit_split_override`):** Per-file atomic unit unlock flag aligned with `B8` `ADR-004`.
> 3. **ADD Guards:** `idx_files_virtual` for `archive://` deduplication; `UNIQUE(file_id, seq)` on `chunks`.
> 4. **SYNC (`security_findings.status`):** Added `accepted_risk` to match `C1` §2 `findings.action`.
> 5. **SYNC (§4):** Clarified `Standard` profile supports `384` (default) or `1024` (optional) vector dims.

---

## 1. Purpose & Scope
Canonical single-source-of-truth schema for SQLite (`WAL` + `FTS5`), local vector shards (`qdrant-edge`), SQLite↔Qdrant outbox sync, secret zero-storage invariants, and V2 PostgreSQL migration readiness. Excludes cloud-sync wire protocols (V2).

**Audience:** Rust core engineers, Search/ML engineers, QA, Security reviewers.

## 2. Storage Topology & Consistency Model
```
[Rust Ingestion Worker]
   │
   ├──(1) ACID Tx──▶ SQLite (WAL, FTS5 triggers, outbox_events)
   │
   └──(2) Async Drain──▶ Vector Sync Worker ──▶ qdrant-edge (local HNSW shards)
                                                   └─ reconciled by `aegis doctor`
```
**Mandatory PRAGMAs:** `journal_mode=WAL`, `synchronous=NORMAL` (`FULL` during `actions`/`audit_log` commits), `foreign_keys=ON`, `busy_timeout=5000`, `temp_store=MEMORY`, `cache_size=-262144` (256MB cap per `A2` §4.1).

## 3. Relational Schema (SQLite DDL — v2.3)

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE volumes (
  id TEXT PRIMARY KEY,
  mount_point TEXT NOT NULL,
  fs_type TEXT NOT NULL,                -- ntfs|apfs|ext4|exfat|smb
  is_removable INTEGER NOT NULL DEFAULT 0,
  is_readonly INTEGER NOT NULL DEFAULT 0,
  has_stable_inode INTEGER NOT NULL DEFAULT 1, -- 0 for exfat / non-persistent smb
  case_sensitive INTEGER NOT NULL DEFAULT 0,   -- 1 for ext4, 0 for default ntfs/apfs
  quarantine_dir TEXT,                  -- "<mount>/.aegis-quarantine"
  status TEXT NOT NULL DEFAULT 'online',-- online|offline|degraded
  last_seen INTEGER NOT NULL
);

CREATE TABLE atomic_units (
  id TEXT PRIMARY KEY,                  -- UUIDv7
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  root_path TEXT NOT NULL,
  unit_type TEXT NOT NULL,              -- git_repo|node_pkg|rust_crate|python_pkg|mac_bundle|latex_proj|custom
  marker_file TEXT NOT NULL,
  project_id TEXT REFERENCES projects(id),
  allow_split_override INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_atomic_units_path ON atomic_units(volume_id, root_path);

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

CREATE TABLE files (
  id TEXT PRIMARY KEY,
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  file_index BLOB,                      -- 8-byte LE u64 (POSIX/NTFS) or 16-byte Windows FILE_ID_128; NULL for archive:// or has_stable_inode=0
  nlink INTEGER NOT NULL DEFAULT 1,
  container_file_id TEXT REFERENCES files(id) ON DELETE CASCADE,
  virtual_uri TEXT,                     -- "archive://<container_id>/docs/report.pdf"
  atomic_unit_id TEXT REFERENCES atomic_units(id) ON DELETE SET NULL,
  unit_split_override INTEGER NOT NULL DEFAULT 0, -- v2.3 ADD: per-file unlock (B8 ADR-004)
  path TEXT NOT NULL,                   -- original path preserved even while state='quarantine'
  path_hash BLOB NOT NULL,              -- BLAKE3(canonical_normalized_path)
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
  state TEXT NOT NULL DEFAULT 'active', -- active|warm|cold|archive|quarantine|missing
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
  -- App layer enforces file_index IS NOT NULL whenever container_file_id IS NULL and volumes.has_stable_inode = 1
  CHECK ((container_file_id IS NULL)
      OR (container_file_id IS NOT NULL AND virtual_uri IS NOT NULL))
);

-- v2.3 FIX: Allows hardlinks (shared file_index, distinct path_hash) while preventing duplicate live paths
CREATE UNIQUE INDEX idx_files_os_path
  ON files(volume_id, file_index, path_hash)
  WHERE file_index IS NOT NULL AND deleted_at IS NULL;
-- v2.3.1 ADD: Prevents duplicate active live rows at the same normalized path (e.g. atomic-save inode swap)
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
  cache_kind TEXT NOT NULL,             -- node_modules|rust_target|python_venv|gradle|build_cache
  size_bytes INTEGER NOT NULL DEFAULT 0,
  file_count INTEGER NOT NULL DEFAULT 0,
  source_max_mtime INTEGER,
  state TEXT NOT NULL DEFAULT 'active', -- active|quarantine|missing
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
  origin TEXT NOT NULL,                 -- simulator|rules|dedupe|security|user_undo
  status TEXT NOT NULL,                 -- draft|approved|executing|executed|partial|rolled_back|failed
  total_actions INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER
);

CREATE TABLE actions (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL REFERENCES action_batches(id) ON DELETE CASCADE,
  agent TEXT,                           -- NULL = user/deterministic rule
  action_type TEXT NOT NULL,            -- move|copy|rename|quarantine|purge|archive|restore
  target_kind TEXT NOT NULL DEFAULT 'file', -- file|dir|atomic_unit
  file_id TEXT REFERENCES files(id),    -- nullable for dir / atomic_unit targets
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
  status TEXT NOT NULL,                 -- proposed|approved|executed|failed|skipped|rolled_back|rejected|superseded|expired|unrecoverable_missing
  rollback_conflict TEXT,               -- NULL|modified|missing_target|collision
  error_message TEXT,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER,
  CHECK ((target_kind = 'file' AND file_id IS NOT NULL)
      OR (target_kind = 'atomic_unit' AND atomic_unit_id IS NOT NULL)
      OR (target_kind = 'dir'))
);
CREATE INDEX idx_actions_batch ON actions(batch_id, status);
-- Enforces at most 1 live (proposed|approved) action per file_id (C1 §3)
CREATE UNIQUE INDEX idx_actions_file_status ON actions(file_id)
  WHERE file_id IS NOT NULL AND status IN ('proposed','approved');

CREATE TABLE quarantine_entries (
  id TEXT PRIMARY KEY,
  action_id TEXT NOT NULL REFERENCES actions(id),
  target_kind TEXT NOT NULL DEFAULT 'file', -- file|dir|atomic_unit
  file_id TEXT REFERENCES files(id),    -- nullable for dir / atomic_unit targets
  atomic_unit_id TEXT REFERENCES atomic_units(id),
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  original_path TEXT NOT NULL,
  quarantine_path TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  blake3 BLOB,                          -- nullable for dir / atomic_unit targets
  quarantined_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  restored_at INTEGER,
  purged_at INTEGER
);
CREATE INDEX idx_quarantine_expiry ON quarantine_entries(expires_at)
  WHERE restored_at IS NULL AND purged_at IS NULL;

-- Canonical hash chain computed in serialized BEGIN IMMEDIATE tx with explicit seq = COALESCE(MAX(seq), 0) + 1:
-- hash = BLAKE3(le64(seq) || lp(id) || le64(ts) || lp(actor) || lp(action) || lp(target) || lp(detail_json) || prev_hash)
-- where lp(s) = le32(utf8_len) || utf8_bytes and genesis prev_hash = 0x00 * 32.
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
  source TEXT NOT NULL,                 -- builtin|user|graduated_proposal|plugin
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE routing_exemplars (
  id TEXT PRIMARY KEY,
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  dest_folder TEXT NOT NULL,
  decision TEXT NOT NULL,               -- approved|rejected
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
  status TEXT NOT NULL DEFAULT 'open',  -- open|false_positive|accepted_risk|vaulted|resolved
  resolved_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_sec_fingerprint ON security_findings(secret_fingerprint);

CREATE TABLE security_finding_locations (
  finding_id TEXT NOT NULL REFERENCES security_findings(id) ON DELETE CASCADE,
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  line_number INTEGER NOT NULL DEFAULT 0, -- 0 when line unknown or whole-file finding
  redacted_snippet TEXT NOT NULL,
  PRIMARY KEY (finding_id, file_id, line_number)
);

CREATE TABLE outbox_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  aggregate_type TEXT NOT NULL,         -- chunk|entity|project|exemplar|file_payload
  aggregate_id TEXT NOT NULL,
  op TEXT NOT NULL,                     -- upsert|delete
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
```

### 3.1 Canonical Path Normalization, Rename-vs-Hardlink Resolution & Purge Semantics
- **Canonical Path Normalization (`path_hash`):** Strip Windows verbatim `\\?\` prefix, normalize separators (`/` on POSIX, canonical volume root + separators on Windows), apply Unicode NFC normalization, and lowercase when `volumes.case_sensitive = 0` before computing `BLAKE3(normalized_path)`. Store exact display casing in `files.path`.
- **State Transition Semantics:** Moving a file into quarantine sets `files.state = 'quarantine'` while retaining `files.path = original_path`; `idx_files_live_path` excludes `'quarantine'` and `'missing'` so creating a new file at `original_path` never collides. Restoring flips `files.state = 'active'` (failing cleanly with collision resolution if a new live row now occupies `original_path`).
- **Rename vs. Hardlink Resolution (Mandatory for Watcher & Scanner):**
  1. **Unstable-inode volumes (`volumes.has_stable_inode = 0`, e.g. `exFAT`/`SMB`):** Look up active row by `(volume_id, path_hash)`; if missing, match candidate `'missing'` or disappeared rows using `(volume_id, size, mtime, blake3)` before `INSERT`ing a new row.
  2. **Stable-inode volumes (`has_stable_inode = 1`):** First check `(volume_id, path_hash)` for an existing live row at that exact path (updating metadata if the same inode, or marking the old row `'missing'`/`deleted_at` and inserting a new row if replaced via atomic save).
  3. When `(volume_id, path_hash)` is not yet live in `files`, query `SELECT id, path, path_hash, size, mtime, ctime, blake3 FROM files WHERE volume_id = ? AND file_index = ? AND deleted_at IS NULL AND state NOT IN ('quarantine')`.
  4. If zero rows → `INSERT` new `files` row.
  5. If `>= 1` rows exist:
     - Call `stat(row.path)` (never bare `fs::try_exists`, which breaks on atomic-save replacements and case-only renames) for each candidate row.
     - If `nlink == 1` OR an existing row's `row.path` either no longer exists, maps to a *different* `file_index` (atomic-save replacement), or matches a case-only rename (`normalized_path == row.normalized_path` with `case_sensitive = 0`): verify inode-reuse sanity (`row.size == size && (row.mtime == mtime || row.ctime == ctime || blake3_matches)`). If valid, treat as **rename/move**: `UPDATE files SET path = ?, path_hash = ?, name = ?, nlink = ?, updated_at = ? WHERE id = row.id` (preserving `id`, `chunks`, embeddings, `relationships`). If inode-reuse check fails, mark old row `deleted_at = now()` and `INSERT` a fresh `files` row.
     - Only when `nlink > 1` AND **all** existing rows still `stat` to the identical `file_index`: treat as a **hardlink** (`INSERT` new `files` row sharing `(volume_id, file_index)` and `UPDATE files SET nlink = ? WHERE volume_id = ? AND file_index = ? AND deleted_at IS NULL`).
- **Soft-Delete & Purge Semantics:** Because `actions` and `quarantine_entries` reference `files(id)` with `ON DELETE RESTRICT`, purging a file retains its tombstoned `files` row (`deleted_at = now()`, `state = 'missing'`) while explicitly executing `DELETE FROM chunks WHERE file_id = ?` (firing FTS5 delete triggers), enqueuing Qdrant point deletes in `outbox_events`, and stamping `relationships.valid_to = now()`.

## 4. Vector Store Architecture (`qdrant-edge`)
| Collection | Vector Dim (by Profile) | Distance | Payload Indexes | Purpose |
|---|---|---|---|---|
| `chunks` | `384` (Eco / Std default) or `1024` (Std opt-in / Pro) | Cosine | `file_id`, `project_id`, `sensitivity`, `state`, `ext`, `mtime` | Hybrid semantic search + semantic dedupe |
| `entities` | `384` or `1024` | Cosine | `entity_type` | Graph resolution |
| `projects` | `384` or `1024` | Cosine | `domain` | Project clustering |
| `routing_exemplars` | `384` or `1024` | Cosine | `decision`, `dest_folder` | Local taxonomy calibration |

- **HNSW:** `m=16`, `ef_construct=100`, query `ef=64`, scalar INT8 quantization.
- **Authoritative Search Safety Post-Filter (`B6` §2):** Qdrant payload filter (`state != 'quarantine'`) is used for pre-filtering efficiency only. Because vector sync is asynchronous via `outbox_events`, **SQLite `files.state != 'quarantine'` (unless `include_quarantined=true`) and `sensitivity` scoping are mandatory authoritative post-filters on every `search.query`**; state/sensitivity flips also enqueue `aggregate_type = 'file_payload'` updates in `outbox_events`.
- **Near-Duplicate Indexing (`FR-004.1`):** Do **not** run `O(N^2)` scans over `simhash_text` / `dhash_image` `BLOB`s. Build in-memory Multi-Index Hashing (MIH) / LSH band tables (4×16-bit bands for 64-bit hashes, 8×16-bit for 128-bit) over active candidates, and confirm text near-duplicates with exact token-shingle Jaccard `≥ 0.85` verification.
- **Outbox & Upgrade Policy:** Versioned collection swap (`chunks_v2_1024`) on profile upgrade; outbox batches ≤1,000 points/tx; `aegis doctor` repairs lag.

## 4.5 PostgreSQL Migration Path (V2+)
All SQL behind `store::db` trait; UUIDv7 TEXT PKs + epoch-ms BIGINTs + canonical JSON text; `chunks_fts` maps to PG `tsvector` + GIN; `aegis migrate --to pg` verifies row counts + `audit_log` head hash.

## 5. Acceptance Criteria, Open Questions, Assumptions & Traceability
- **Acceptance:** Migrations run `<200ms`; 10k rename + 1k hardlink + atomic-save + case-rename + `exFAT` fuzz test passes with `0` constraint errors and `0` broken `files.id`s; `idx_actions_file_status` rejects duplicate live proposals; `chunks_fts` stays 100% synced over 100k ops.
- **Open:** `[OPEN]` `OQ-13` (Owner: Data Architect, Milestone 2): `routing_exemplars` 18-month expiry vs. exponential decay.
- **Assumptions:** `[ASSUMPTION]` `qdrant-edge` supports concurrent reads during single-writer batches ≤1,000 points.
- **Traceability:** Upstream `A2` (`FR-001..FR-013`), `B8` (`ADR-001`–`ADR-004`); Downstream `C1 v1.2`, `C4 v1.1`, `E2/E4 v1.2`, `F1/F2 v2.3`.
