# B4 — Data & Vector Schema Specification

> **SUPERSEDED** — canonical schema is `B4_Data_Vector_Schema_v2.2.md` (v2.1 extracted from `critical_analysis-v2.md` §Document 2 + v2.2 patches; see `suite_reconciliation.md` §2–3). This file's unique contributions are merged: PG migration → v2.2 §4.5, Qdrant params → v2.2 §4. Lifecycle confirmed folded into `files.state`.

**Aegis Guardian · v2.1 · Status: Authoritative for MVP**
Closes audit gaps F4/F5-adjacent: adds FTS5, `archive://` virtual assets, `nlink`, chunking strategy, SQLite→PG migration path.

---

## 1. Storage Layout

| Store | Tech | Role |
|-------|------|------|
| `aegis.db` | SQLite 3.4x, WAL, `synchronous=FULL` during action windows | Relational truth: files, graph, actions, audit, rules, jobs |
| `vectors/` | Qdrant embedded (Rust `qdrant` client or embedded mode) | `chunks`, `entities`, `projects` collections |
| `journal/` | Append-only hash-chained log (also mirrored in `audit_log` table) | Mutation journal for rollback |
| `<volume>/.aegis-quarantine/` | Per-volume quarantine (ADR-002) | Reversible deletes |
| `models/` | Downloaded ONNX/LLM artifacts | Checksum-pinned |

Encryption-at-rest option (V1.5): SQLCipher build of `aegis.db`; Argon2id(passphrase)→DEK; key in OS keychain.

---

## 2. Core Tables (DDL)

```sql
PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;

CREATE TABLE files (
  id            TEXT PRIMARY KEY,          -- uuid v7
  volume_id     TEXT NOT NULL,             -- volume serial/uuid (stable per mount source)
  file_index    INTEGER,                   -- NTFS file_index / st_ino (NULL for virtual)
  path          TEXT NOT NULL,
  path_hash     BLOB NOT NULL,             -- blake3(lower-canonical path) for dedupe-safe lookup
  name          TEXT NOT NULL,
  ext           TEXT,
  mime          TEXT,
  size          INTEGER NOT NULL,
  mtime         INTEGER NOT NULL,
  ctime         INTEGER,
  blake3        BLOB,                      -- NULL until hashed
  nlink         INTEGER NOT NULL DEFAULT 1,-- hardlink count → honest reclaim math
  kind          TEXT NOT NULL DEFAULT 'file', -- file|dir|virtual|remote-only
  container_id  TEXT REFERENCES files(id), -- archive:// parent (NULL for real files)
  state         TEXT NOT NULL DEFAULT 'active',
                -- active|warm|cold|archive|quarantine|missing|remote-only
  project_id    TEXT REFERENCES projects(id),
  classification TEXT,
  confidence    REAL,
  sensitivity   TEXT NOT NULL DEFAULT 'public', -- public|internal|confidential|secret
  lifecycle_state TEXT NOT NULL DEFAULT 'active',
  first_seen    INTEGER NOT NULL,
  last_seen     INTEGER NOT NULL,
  access_count  INTEGER NOT NULL DEFAULT 0,
  last_access   INTEGER,
  watcher_alive INTEGER NOT NULL DEFAULT 1,
  UNIQUE(volume_id, file_index)            -- NULL file_index rows excluded via partial index below
);
CREATE UNIQUE INDEX files_real_identity
  ON files(volume_id, file_index)
  WHERE file_index IS NOT NULL AND kind IN ('file','dir');
CREATE INDEX files_path ON files(path_hash);
CREATE INDEX files_project ON files(project_id);
CREATE INDEX files_state ON files(state, lifecycle_state);
CREATE INDEX files_mtime ON files(mtime);
```

```sql
CREATE TABLE chunks (
  id          TEXT PRIMARY KEY,
  file_id     TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,
  text        TEXT NOT NULL,
  token_count INTEGER NOT NULL,
  vector_ref  TEXT NOT NULL,               -- qdrant point id
  UNIQUE(file_id, seq)
);
CREATE INDEX chunks_file ON chunks(file_id);

-- FTS5 mirror of chunk+doc text (BM25 leg of hybrid search)
CREATE VIRTUAL TABLE chunks_fts USING fts5(
  text, file_id UNINDEXED, seq UNINDEXED,
  tokenize='unicode61 remove_diacritics 2'
);
-- Maintained by triggers on chunks insert/update/delete.
-- Rebuild path: `aegis doctor --rebuild-fts`.
```

```sql
CREATE TABLE entities (
  id TEXT PRIMARY KEY, type TEXT NOT NULL,   -- file|folder|project|person|company|topic|repo|account|event|collection
  name TEXT NOT NULL, attrs JSON NOT NULL DEFAULT '{}',
  embedding_ref TEXT                          -- qdrant point id (entity collection)
);
CREATE INDEX entities_type ON entities(type);

CREATE TABLE relationships (
  id TEXT PRIMARY KEY,
  src_id TEXT NOT NULL REFERENCES entities(id),
  dst_id TEXT NOT NULL REFERENCES entities(id),
  rel_type TEXT NOT NULL,                    -- references|belongs_to|derived_from|duplicate_of|version_of|authored_by|mentions|related_to|supersedes
  weight REAL NOT NULL DEFAULT 1.0,
  evidence JSON NOT NULL DEFAULT '[]',
  valid_from INTEGER NOT NULL,
  valid_to   INTEGER                          -- NULL = current
);
CREATE INDEX rel_src ON relationships(src_id, rel_type);
CREATE INDEX rel_dst ON relationships(dst_id, rel_type);
```

```sql
CREATE TABLE projects (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT,
  domain TEXT,                               -- general|research|developer|trader|...
  repo_url TEXT, health_score REAL,
  atomic_unit INTEGER NOT NULL DEFAULT 1,    -- ADR-003 boundary enforcement flag
  created_at INTEGER NOT NULL
);

CREATE TABLE actions (                        -- proposal + execution lifecycle (C1 §state machine)
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,                    -- simulator batch grouping
  agent TEXT,                                -- NULL = user/rule
  rule_id TEXT REFERENCES rules(id),
  action_type TEXT NOT NULL,                 -- move|copy|rename|quarantine|restore|delete|archive|encrypt|lifecycle
  file_id TEXT REFERENCES files(id),
  from_path TEXT NOT NULL, to_path TEXT,
  pre_state JSON NOT NULL,                   -- {existed, blake3, size, nlink, attrs} for rollback verify
  post_state JSON,                           -- filled at execute
  reason TEXT NOT NULL, confidence REAL NOT NULL,
  evidence JSON NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'proposed',   -- proposed|approved|executed|failed|rolled_back|rejected
  created_at INTEGER NOT NULL,
  decided_at INTEGER, executed_at INTEGER, rolled_back_at INTEGER,
  rollback_conflict TEXT                     -- NULL|modified|missing_target|collision (S2.3)
);
CREATE INDEX actions_status ON actions(status);
CREATE INDEX actions_batch ON actions(batch_id);
CREATE INDEX actions_file ON actions(file_id);
```

```sql
CREATE TABLE audit_log (                      -- hash-chained, tamper-EVIDENT (B1/B2 §7)
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,                       -- user|agent:<name>|system|rule:<id>
  action TEXT NOT NULL, target TEXT,
  detail JSON NOT NULL DEFAULT '{}',
  prev_hash BLOB NOT NULL,
  hash BLOB NOT NULL                          -- blake3(prev_hash || canonical(row))
);

CREATE TABLE scan_jobs (
  id TEXT PRIMARY KEY, root TEXT NOT NULL,
  tier INTEGER NOT NULL DEFAULT 3,           -- 1|2|3 progressive funnel
  started_at INTEGER, finished_at INTEGER,
  status TEXT NOT NULL DEFAULT 'running',    -- running|paused|done|failed|partial
  files_seen INTEGER DEFAULT 0, files_changed INTEGER DEFAULT 0,
  files_skipped INTEGER DEFAULT 0,
  errors JSON NOT NULL DEFAULT '[]',
  coverage_pct REAL,
  checkpoint JSON                            -- resume token
);
CREATE INDEX scan_jobs_root ON scan_jobs(root, status);

CREATE TABLE rules (                         -- deterministic, human-editable (FR-023 source)
  id TEXT PRIMARY KEY, name TEXT NOT NULL,
  scope_json JSON NOT NULL,                  -- roots/projects/types globs
  condition_json JSON NOT NULL,              -- match predicates
  action_json JSON NOT NULL,                 -- move|tag|quarantine template
  priority INTEGER NOT NULL DEFAULT 100,
  enabled INTEGER NOT NULL DEFAULT 1,
  source TEXT NOT NULL DEFAULT 'user',       -- user|builtin|graduated|plugin
  approval_count INTEGER NOT NULL DEFAULT 0, -- few-shot exemplar weight (S3.4)
  rejection_count INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE security_findings (
  id TEXT PRIMARY KEY,
  secret_hash BLOB,                          -- dedupe key: identical secrets → 1 finding
  file_id TEXT REFERENCES files(id),
  finding_type TEXT NOT NULL,                -- api_key|password|private_key|pii|identity|financial
  severity TEXT NOT NULL,                    -- critical|high|medium|low|info
  context_score REAL NOT NULL,               -- risk-weighted triage (PRD §7.9)
  locations JSON NOT NULL DEFAULT '[]',      -- all paths sharing secret_hash
  evidence JSON NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'open',       -- open|false_positive|resolved|accepted_risk
  first_seen INTEGER NOT NULL, resolved_at INTEGER
);
CREATE INDEX findings_status ON security_findings(status, severity);

CREATE TABLE settings (
  key TEXT NOT NULL, scope TEXT NOT NULL DEFAULT 'global',  -- global|project:<id>|root:<path>
  value JSON NOT NULL,
  PRIMARY KEY(key, scope)
);

CREATE TABLE quarantine_items (               -- mirrors per-volume quarantine dirs
  id TEXT PRIMARY KEY,
  action_id TEXT REFERENCES actions(id),
  volume_id TEXT NOT NULL,
  orig_path TEXT NOT NULL,
  quarantine_path TEXT NOT NULL,
  blake3 BLOB NOT NULL, size INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,               -- TTL 30d default
  restored_at INTEGER
);
```

---

## 3. Virtual Assets (`archive://`, attachments, remote-only)

Real files have OS identity (`file_index`/`inode`). Virtual assets do not — model explicitly:

- `kind='virtual'`, `file_index=NULL`, `container_id` → parent archive file row.
- `path` = `archive://<parent-path>#<member-path>` (canonical, dedupe-safe).
- Members get their own `chunks`/embeddings (searchable) but **cannot be individually moved** — refiling proposals target the container only (consistent with Atomic Units).
- `kind='remote-only'` for dehydrated cloud placeholders: indexed metadata-only; content ops gated (B1/B2 §5).
- Email attachments (V2) follow same pattern with `kind='virtual'`, container = `.eml`/`.mbox` artifact.

Partial unique index excludes NULL `file_index` so virtual rows coexist:

```sql
-- files_real_identity already covers this via WHERE file_index IS NOT NULL
```

---

## 4. Chunking Strategy (B4-authoritative)

- **Selective:** docs ≤1,500 tokens → exactly 1 chunk. Larger docs → sliding window 512 tokens, stride 64, hard cap 64 chunks/doc (tail truncated, flagged `truncated=true` in chunk metadata JSON).
- **Structure-aware:** PDFs split at page boundaries where possible; code split at tree-sitter top-level decls; spreadsheets per-sheet+header-row chunks; slides per-slide.
- **Two vector payloads per chunk:** `file_id`, `seq`, `project_id`, `sensitivity`, `mtime` — all indexed for filtered ANN.
- **Doc-level embedding:** mean-pool or dedicated summary chunk (`seq=-1`) for document retrieval leg.

Expected ratio ~1.3 chunks/file on real corpora → 1M files ≈ 1.3M vectors (MVP-tested to 5M).

---

## 5. Qdrant Collections

| Collection | Vector | Dim | Payload indexes | Quantization |
|------------|--------|-----|-----------------|--------------|
| `chunks` | embedding model output | 384 (Eco/Std-small) / 1024 (bge-m3) | `file_id`, `project_id`, `sensitivity`, `mtime` | scalar INT8 |
| `entities` | same model | same | `type` | scalar INT8 |
| `projects` | same model | same | `domain` | scalar INT8 |

HNSW: `m=16, ef_construct=100`; query `ef=64` (hnsw), oversample for RRF fusion.

---

## 6. Hardlink & Reclaim Accounting (F4 fix)

- `nlink>1` ⇒ file shares blocks. Dedupe groups containing hardlinks: **reclaimable_bytes = 0** for the group (deleting one link frees nothing). Report shows "N hardlinked copies — no space recovered by dedupe".
- Exact-dupe groups (distinct inodes, same blake3): reclaimable = `size × (group_size − 1)`.
- All storage metrics derive from this rule — no path may report inflated reclaim (honesty invariant, PRD §5.6).

---

## 7. SQLite → PostgreSQL Migration Path

Deferred to sync/enterprise (PRD §18.3) but designed now:

- All SQL behind a `db` trait module; no raw SQL outside `store/`.
- Types mapped: `TEXT`→`text/uuid`, `INTEGER`→`bigint`, `BLOB`→`bytea`, `JSON`→`jsonb` (lossless both ways — store canonical JSON text).
- FTS5 → `tsvector` + GIN; Qdrant unchanged; journal file format unchanged.
- Migration tool: `aegis migrate --to pg` streams tables in FK order, verifies row counts + audit chain head hash, cutover flag in `settings`.
- Multi-writer PG mode (V2 sync) requires moving journal authority to PG — flagged as ADR-needed at that time.

---

## 8. Consistency & Integrity Rules

- `actions.pre_state` snapshot taken **before** mutation; `post_state` after; rollback verify reads `post_state.blake3`.
- FTS/vector drift tolerated; reconciler (`aegis doctor`) rebuilds from SQLite truth.
- Deletes are soft (`state='quarantine'`/`'missing'`); physical purge only after quarantine TTL expiry job.
- `path_hash` enables move detection: same hash + new path + same `file_index` ⇒ rename (update path, keep id).
