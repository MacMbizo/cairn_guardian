# B4 — Data & Vector Architecture Specification (v2.2)

> **SUPERSEDED by `B4_Data_Vector_Schema_v2.3.md`** — v2.3 fixes the hardlink `UNIQUE` regression introduced here (`idx_files_os_path` restores `path_hash` + adds `idx_files_inode` + rename-vs-hardlink resolution §3.1), adds `unit_split_override`, `idx_files_virtual`, `UNIQUE(file_id,seq)`, `accepted_risk`, and `rollback_conflict` status values. Retained for history only.

**ID:** B4 · **Version:** 2.2 · **Status:** Superseded
**Owner:** Staff Data & Storage Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B8_ADRs.md` (ADR-001–004), `B1_B2_Architecture.md`, `C1_IPC_Action_Contracts.md`

> **v2.2 changes vs canonical v2.1** (per `suite_reconciliation.md` §3.3, I5 + Devin-B4 merges):
> 1. **FIX:** `idx_files_os_identity` — dropped `path_hash` from unique key; rename window no longer permits duplicate live identities.
> 2. **ADD:** `actions.pre_state_json`/`post_state_json` — full rollback-verifiable snapshots (metadata mutations incl. xattrs/ACLs), complementing `pre_blake3`/`post_blake3`.
> 3. **MERGE:** §4.5 PostgreSQL migration mechanics + §4.6 Qdrant parameter detail (from Devin B4).
> 4. Lifecycle stays folded into `files.state` (v2.1 decision confirmed — no separate `lifecycle_state` column).

---

## 1. Purpose & Scope

Canonical single-source-of-truth schema for SQLite (WAL + FTS5), local vector shards (`qdrant-edge`), SQLite↔Qdrant outbox synchronisation, privacy/encryption posture, and V2 PostgreSQL migration readiness. Excludes cloud-sync wire protocols (V2).

## 2. Storage Topology & Consistency Model

```
[Rust Ingestion Worker]
   │
   ├──(1) ACID Tx──▶ SQLite (WAL, FTS5 triggers, outbox_events)
   │
   └──(2) Async Drain──▶ Vector Sync Worker ──▶ qdrant-edge (local HNSW shards)
                                                   └─ reconciled by `aegis doctor`
```

**PRAGMAs (mandatory at connect):** `journal_mode=WAL`, `synchronous=NORMAL` (→`FULL` during `actions`/`audit_log` commits), `foreign_keys=ON`, `busy_timeout=5000`, `temp_store=MEMORY`, `cache_size=-262144` (256MB page cache cap per A2 §4.1).

## 3. Relational Schema (SQLite DDL — v2.2 baseline)

```sql
PRAGMA foreign_keys = ON;

-- 1. Volume registry (ADR-003 per-volume quarantine)
CREATE TABLE volumes (
  id TEXT PRIMARY KEY,                  -- OS volume GUID / UUID
  mount_point TEXT NOT NULL,
  fs_type TEXT NOT NULL,                -- ntfs|apfs|ext4|exfat|smb
  is_removable INTEGER NOT NULL DEFAULT 0,
  is_readonly INTEGER NOT NULL DEFAULT 0,
  quarantine_dir TEXT,                  -- "<mount>/.aegis-quarantine"
  status TEXT NOT NULL DEFAULT 'online',-- online|offline|degraded
  last_seen INTEGER NOT NULL
);

-- 2. Atomic Project Units (ADR-004)
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

-- 3. Projects
CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT,
  domain TEXT NOT NULL DEFAULT 'general', -- general|developer|research|trader|household
  repo_url TEXT,
  health_score REAL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- 4. Files & Virtual Archive Members (FR-001.6 P0, FR-001.9, FR-001.10)
CREATE TABLE files (
  id TEXT PRIMARY KEY,                  -- UUIDv7; survives rename via (volume_id, file_index)
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  file_index INTEGER,                   -- OS inode / NTFS FileId (NULL ONLY for archive:// virtuals)
  nlink INTEGER NOT NULL DEFAULT 1,
  container_file_id TEXT REFERENCES files(id) ON DELETE CASCADE,
  virtual_uri TEXT,                     -- "archive://<container_id>/docs/report.pdf"
  atomic_unit_id TEXT REFERENCES atomic_units(id) ON DELETE SET NULL,
  path TEXT NOT NULL,
  path_hash BLOB NOT NULL,              -- BLAKE3(normalized_path)
  name TEXT NOT NULL,
  ext TEXT,
  mime TEXT,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  ctime INTEGER NOT NULL,
  blake3 BLOB,                          -- NULL until Tier-2/background hash completes
  simhash_text BLOB,                    -- 64/128-bit MinHash/SimHash for near-dupe
  dhash_image BLOB,                     -- 128-bit perceptual hash for images
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
  CHECK ((container_file_id IS NULL AND file_index IS NOT NULL)
      OR (container_file_id IS NOT NULL AND virtual_uri IS NOT NULL))
);
-- v2.2 FIX: identity is (volume_id, file_index) ONLY. path_hash removed —
-- a rename changes path_hash; including it permitted two live rows sharing
-- file_index during the rename-detection window (reconciliation §3.3).
-- Rename handling: watcher matches file_index → UPDATE path/path_hash on same id.
CREATE UNIQUE INDEX idx_files_os_identity
  ON files(volume_id, file_index)
  WHERE file_index IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_files_path_hash ON files(path_hash);
CREATE INDEX idx_files_blake3 ON files(blake3) WHERE blake3 IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_files_project ON files(project_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_files_atomic_unit ON files(atomic_unit_id);
CREATE INDEX idx_files_state ON files(state) WHERE deleted_at IS NULL;

-- 5. Chunks & FTS5 Hybrid Search (resolves §7.7 vs §9 drift)
CREATE TABLE chunks (
  id INTEGER PRIMARY KEY AUTOINCREMENT, -- rowid required for FTS5 external-content sync
  chunk_uuid TEXT NOT NULL UNIQUE,      -- Qdrant point ID
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  text TEXT NOT NULL,
  token_count INTEGER NOT NULL,
  is_summary INTEGER NOT NULL DEFAULT 0,
  embedding_model TEXT,                 -- e.g., "bge-small-en-v1.5-q8"
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_chunks_file ON chunks(file_id, seq);

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

-- 6. Knowledge Graph
CREATE TABLE entities (
  id TEXT PRIMARY KEY,
  entity_type TEXT NOT NULL,            -- person|company|topic|repo|account|event|collection
  name TEXT NOT NULL,
  attrs_json TEXT NOT NULL DEFAULT '{}',
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE relationships (
  id TEXT PRIMARY KEY,
  src_id TEXT NOT NULL,
  src_kind TEXT NOT NULL,               -- file|entity|project
  dst_id TEXT NOT NULL,
  dst_kind TEXT NOT NULL,
  rel_type TEXT NOT NULL,               -- references|belongs_to|derived_from|duplicate_of|version_of|authored_by|mentions|related_to|supersedes
  weight REAL NOT NULL DEFAULT 1.0,
  evidence_json TEXT NOT NULL DEFAULT '[]',
  valid_from INTEGER NOT NULL,
  valid_to INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_rel_src ON relationships(src_id, rel_type);
CREATE INDEX idx_rel_dst ON relationships(dst_id, rel_type);

-- 7. Automation: batches, actions, pre-rollback snapshots, quarantine
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
  file_id TEXT NOT NULL REFERENCES files(id),
  from_path TEXT NOT NULL,
  to_path TEXT,
  pre_blake3 BLOB,
  post_blake3 BLOB,
  pre_state_json TEXT,                  -- v2.2 ADD: full pre-mutation snapshot
                                       -- {existed,size,mtime,mode,xattrs[],acl_ref,nlink,cloud_state}
                                       -- enables rollback verify for metadata mutations, not just content
  post_state_json TEXT,
  reason TEXT NOT NULL,
  confidence REAL NOT NULL,
  evidence_json TEXT NOT NULL,
  status TEXT NOT NULL,                 -- proposed|approved|executed|failed|skipped|
                                       -- rolled_back|rejected|superseded|expired (C1 §3)
  rollback_conflict TEXT,               -- NULL|modified|missing_target|collision (C1 §4 / FR-005.4)
  error_message TEXT,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER
);
CREATE INDEX idx_actions_batch ON actions(batch_id, status);
CREATE INDEX idx_actions_file_status ON actions(file_id, status)
  WHERE status IN ('proposed','approved');   -- one live proposal per file (C1 §3)

CREATE TABLE quarantine_entries (
  id TEXT PRIMARY KEY,
  action_id TEXT NOT NULL REFERENCES actions(id),
  file_id TEXT NOT NULL REFERENCES files(id),
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  original_path TEXT NOT NULL,
  quarantine_path TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  blake3 BLOB NOT NULL,
  quarantined_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,          -- +30d TTL default
  restored_at INTEGER,
  purged_at INTEGER
);
CREATE INDEX idx_quarantine_expiry ON quarantine_entries(expires_at)
  WHERE restored_at IS NULL AND purged_at IS NULL;

-- 8. Hash-chained audit log (tamper-EVIDENT; see B1/B2 §7 threat model, B6)
CREATE TABLE audit_log (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,                  -- user|agent:<name>|system|rule:<id>
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  detail_json TEXT NOT NULL,
  prev_hash BLOB NOT NULL,              -- BLAKE3 of prior row (0x00*32 genesis)
  hash BLOB NOT NULL                    -- BLAKE3(seq||id||ts||actor||action||target||detail_json||prev_hash)
);

-- 9. Jobs, rules, exemplars, security findings, outbox, settings
CREATE TABLE scan_jobs (
  id TEXT PRIMARY KEY,
  root_path TEXT NOT NULL,
  tier INTEGER NOT NULL,                -- 1=Metadata|2=Priority Semantic|3=Deep Background
  status TEXT NOT NULL,                 -- queued|running|paused|completed|degraded|failed
  files_seen INTEGER NOT NULL DEFAULT 0,
  files_indexed INTEGER NOT NULL DEFAULT 0,
  files_skipped_perm INTEGER NOT NULL DEFAULT 0,
  files_skipped_cloud INTEGER NOT NULL DEFAULT 0,
  checkpoint_json TEXT,                 -- resume token (C1 §2 scan.status)
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

CREATE TABLE routing_exemplars (         -- local taxonomy calibration (few-shot)
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
  secret_fingerprint BLOB NOT NULL,     -- HMAC-BLAKE3(per-install keychain salt, normalized_secret)
                                       -- RAW SECRET NEVER STORED
  finding_type TEXT NOT NULL,           -- aws_key|github_pat|private_key|password_file|id_doc|financial|pii
  base_severity TEXT NOT NULL,          -- critical|high|medium|low
  context_risk_score REAL NOT NULL,     -- 0–100 path-context weighted
  status TEXT NOT NULL DEFAULT 'open',  -- open|false_positive|vaulted|resolved
  resolved_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_sec_fingerprint ON security_findings(secret_fingerprint);

CREATE TABLE security_finding_locations (
  finding_id TEXT NOT NULL REFERENCES security_findings(id) ON DELETE CASCADE,
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  line_number INTEGER,
  redacted_snippet TEXT NOT NULL,       -- first4/last2 only: "AKIA...9X2Q"
  PRIMARY KEY (finding_id, file_id)
);

CREATE TABLE outbox_events (             -- SQLite→Qdrant sync queue
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  aggregate_type TEXT NOT NULL,         -- chunk|entity|project|exemplar
  aggregate_id TEXT NOT NULL,
  op TEXT NOT NULL,                     -- upsert|delete
  payload_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  processed_at INTEGER
);
CREATE INDEX idx_outbox_pending ON outbox_events(seq) WHERE processed_at IS NULL;

CREATE TABLE settings (
  key TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'global', -- global|project:<id>|root:<id>
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (key, scope)
);
```

## 4. Vector Store Architecture (`qdrant-edge`)

| Collection | Vector Dim (by Profile) | Distance | Payload Indexes | Purpose |
|---|---|---|---|---|
| `chunks` | 384 (Eco/Std) or 1024 (Pro) | Cosine | `file_id`, `project_id`, `sensitivity`, `ext`, `mtime` | Hybrid semantic search + semantic dedupe |
| `entities` | 384 or 1024 | Cosine | `entity_type` | Graph resolution & entity linking |
| `projects` | 384 or 1024 | Cosine | `domain` | Project clustering |
| `routing_exemplars` | 384 or 1024 | Cosine | `decision`, `dest_folder` | Taxonomy preference calibration |

**HNSW params:** `m=16`, `ef_construct=100`; query `ef=64`, oversample for RRF fusion. Scalar INT8 quantization on all collections.

**Model upgrade policy:** profile change (384↔1024) creates versioned collection (`chunks_v2_1024`), drains background re-embed queue, atomically swaps active pointer in `settings` — search never degrades mid-migration.

**Outbox contract:** every chunk/entity/exemplar write enqueues `outbox_events` row in same SQLite txn; vector sync worker drains ≤1,000 points/tx; `processed_at` marks done; `aegis doctor` detects lag and replays. Qdrant reads concurrent with single-writer batches (assumption A-04).

## 4.5 PostgreSQL Migration Path (v2.2 merge — was Devin B4 §7)

Deferred to sync/enterprise but designed now:

- All SQL behind `db` trait module in `store/`; zero raw SQL outside it.
- Type map: `TEXT`→`text/uuid`, `INTEGER`→`bigint`, `BLOB`→`bytea`, `JSON`→`jsonb` (store canonical JSON text — lossless both ways).
- `chunks_fts` → PG `tsvector` column + GIN index; triggers become generated-column/trigger equivalents.
- Migration tool `aegis migrate --to pg`: streams tables in FK order, verifies row counts + audit chain head hash, sets cutover flag in `settings`.
- Multi-writer PG mode (V2 sync) requires moving journal authority to PG — flagged as ADR-needed then.

## 4.6 Privacy & Encryption

- **Secret zero-storage:** `security_findings` stores only `HMAC-BLAKE3(per-install keychain salt, normalized_secret)` + masked snippet. Raw secrets never enter SQLite, logs, or vector payloads.
- **At-rest (V1.5):** opt-in SQLCipher (AES-256) + encrypted `qdrant-edge` dir; Argon2id→DEK in OS Keychain.
- **Audit:** hash-chained, tamper-evident (not tamper-proof — threat model in `B6_Threat_Model.md`).

## 5. Consistency & Integrity Rules

- `actions.pre_state_json`/`pre_blake3` captured **before** mutation; `post_*` after; rollback verify per `C1_IPC_Action_Contracts.md` §4.
- FTS/vector drift tolerated; reconciler rebuilds from SQLite truth.
- Deletes soft (`state='quarantine'`/`'missing'`); physical purge only after quarantine TTL expiry job.
- `path_hash` enables move detection: same hash + new path + same `file_index` ⇒ rename (UPDATE path, keep id).
- Hardlink reclaim rule (ADR-003): `nlink>1` groups report **0 reclaimable bytes** until all-but-keeper links removed; exact-dupe groups report `size×(n−1)`. All storage metrics derive from this — no inflated reclaim (G6 honesty).

## 6. Acceptance / Done Criteria

- Migrations run cleanly from empty DB in <200ms via embedded migration runner.
- `chunks_fts` 100% row-count consistent across insert/update/delete stress (100k ops).
- `aegis doctor` detects+repairs simulated outbox lag and broken `audit_log` chains.
- Rename fuzz: 10k rename/move cycles preserve `files.id` and graph edges (v2.2 index fix verified).

## 7. Change Log

| Version | Date | Summary |
|---|---|---|
| 2.0 | Pre-MVP | Initial §9 inline schema (PRD v2.0) |
| 2.1 | 2026-11-20 | volumes, atomic_units, FTS5+triggers, nlink, archive:// CHECK, quarantine_entries, outbox_events, dedup security_findings, routing_exemplars (Suite Athena) |
| 2.2 | 2026-11-20 | idx_files_os_identity fix; actions pre/post_state_json; PG migration §4.5; Qdrant params §4.6→§4 merge notes; lifecycle folded confirmation (Devin merges per reconciliation) |
