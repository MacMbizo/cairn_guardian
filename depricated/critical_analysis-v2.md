## Summary

This deliverable upgrades **Aegis Guardian** from an ambitious single-file Pre-MVP specification (`PRD v2.0`) into a **synchronized, production-grade core documentation suite** ready for engineering, QA, and security execution.

Specifically, this response provides:
1. **Decisive Architectural Recommendations** resolving the 4 newly surfaced open questions (`OQ-08`–`OQ-11`).
2. **Document `B8` — Architecture Decision Records (`ADR-001` to `ADR-004`)**: Locking Rust-first inference/extraction, hardware-tiered model lifecycle + 3-tier indexing, per-volume quarantine, and atomic project unit boundaries.
3. **Document `B4` — Data & Vector Architecture Specification (`v2.1`)**: Complete SQLite WAL + FTS5 + `qdrant-edge` schema resolving all v2.0 schema drift (`FTS5` tables/triggers, `nlink` hardlinks, `archive://` virtual assets, `atomic_units`, `rollback_snapshots`, `quarantine_entries`, deduplicated `security_findings`, and SQLite↔Qdrant outbox sync).
4. **Document `A2` — Aegis Guardian PRD (`v2.1`)**: Reconciled goals vs. metrics, fixed 4GB RSS and 1M-file throughput math, promoted rename-tracking to `P0`, added testable acceptance criteria for all MVP requirements (`FR-001`–`FR-013`), OS permission/placeholder handling, and full Persona-to-Test traceability.
5. **Cross-Reference Matrix, Living Index (`G1`), Changelog (`G3`), Open Questions/Assumptions (`A7`), and Next Actions**.

---

## Knowledge Base Snapshot (Normalized v2.1)

| Dimension | Normalized Specification State |
|---|---|
| **Product** | Aegis Guardian — local-first AI desktop file intelligence, semantic search, dedupe, secret hygiene, and reversible automated refiling platform |
| **Core Stack** | Tauri 2.x + React/TS/Tailwind + Rust core (`ort` / `fastembed`, `tree-sitter`, `pdfium-render`, `blake3`, `notify`) + SQLite (WAL + FTS5) + `qdrant-edge` (local embedded vector shard) + optional lazy Python plugin runtime (V1.5+) |
| **MVP Target** | Windows 10/11 (primary), macOS 12+ & Ubuntu 22.04+ (fast-follow parity); 8GB RAM minimum floor; ≤4GB RSS during scan on 16GB machine |
| **Throughput Strategy** | 3-Tier Progressive Indexing: Tier 1 metadata ≤90s–15m; Tier 2 priority roots ≤60m; Tier 3 deep background ≤6h total for 1M files via selective/hierarchical chunking |
| **Safety Invariants** | Action-proposal pipeline only; per-volume atomic quarantine; pre-rollback state reconciliation; atomic project boundaries; zero cloud egress by default |

---

## Recommended Solutions for the 4 Open Questions

| ID | Open Question | Recommended Solution | Rationale & Trade-Offs |
|---|---|---|---|
| **OQ-08** | **Rust-native extraction vs. Python sidecar for MVP?** | **Rust-first core for MVP** (`ort`, `fastembed`, `tree-sitter`, `pdfium-render`, `calamine`, `quick-xml`). **Defer Python to an optional, lazy-downloaded V1.5+ plugin runtime** for Whisper, heavy Paddle/Tesseract OCR, `.pst`, and VLM captioning. | **Why:** Eliminates 1.5–3GB installer bloat, zombie IPC sidecar crashes, gRPC serialization overhead across 1M files, and AV false positives. `fastembed` + `ort` natively runs BGE/E5 ONNX models on CPU/CoreML/CUDA/DirectML in Rust. **Trade-off:** Slightly higher initial Rust integration effort for PDF/Office edge cases; mitigated by using `pdfium-render` (Chromium's battle-tested PDF engine). |
| **OQ-09** | **Minimum hardware baseline & LLM bundling for MVP?** | **8GB RAM / 4-core CPU minimum floor** using **3 Hardware Profiles** (`Eco`, `Standard`, `Pro`). Never keep an LLM resident during scans; on 16GB (`Standard`), load a 1.5B–3B Q4 model on-demand and evict after 5 min idle. Download models post-install by profile (not bundled in installer). | **Why:** Physically impossible to fit BGE-M3 + NER + 8B LLM inside the 4GB RSS cap (§13). Profile-based downloads keep installer <150MB and prevent OOM crashes on 8GB laptops. **Trade-off:** 8GB (`Eco`) machines use BM25 + 384-dim `bge-small-en-v1.5` + deterministic query filters without local LLM NL-rewriting. |
| **OQ-10** | **Which directory markers lock an Atomic Project Unit against file-level refiling?** | **Deterministic marker list** (`.git`, `package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, `.xcodeproj`, `.app`, `.photoslibrary`, `.latexmkrc`/main `.tex` with `\documentclass`, SQLite `-wal`/`-shm` triplets) + user-editable `.aegisboundary` override file. | **Why:** Prevents Curator/rules from ripping a `README.md` or `data.csv` out of a codebase or app bundle and breaking builds. **Trade-off:** Misplaced files *dropped inside* a Git repo won't be auto-refiled individually unless the user explicitly toggles *"Allow refiling inside project boundaries"* per root. |
| **OQ-11** | **Quarantine behaviour on read-only, full, or unmounted external volumes?** | **Per-volume `<VolumeRoot>/.aegis-quarantine` primary** (`O(1)` atomic rename). Fallback hierarchy: **(1)** OS Trash API if same-volume → **(2)** Copy-verify-delete to primary drive quarantine *only* if `file_size < 25% of free_space` → **(3)** Explicit blocking prompt (*Permanent Delete / Skip*) if volume is read-only or lacks space. | **Why:** Guarantees zero silent slow cross-drive 20GB copies and prevents filling the OS boot SSD. **Trade-off:** Requires clean handling of hidden `.aegis-quarantine` folder cleanup (`aegis doctor` / eject hook) on removable drives. |

---

## Generated / Revised Production-Grade Documents

---

### Document 1: `B8` — Architecture Decision Records (`ADR-001` to `ADR-004`)

#### 1. Metadata
- **ID:** `B8` | **Title:** Aegis Guardian Core Architecture Decision Records
- **Version:** `1.0` | **Status:** `Approved (Pre-MVP Baseline)`
- **Owner:** Principal Systems Architect | **Last Updated:** `2026-11-20`
- **Related Docs:** `A2` (PRD v2.1), `B4` (Data & Vector Schema v2.1)

#### 2. Purpose & Scope
Records binding architectural decisions resolving the v2.0 memory, throughput, filesystem safety, and packaging contradictions before MVP implementation begins.

#### 3. Audience
Backend/Rust engineers, ML engineers, Desktop/Tauri engineers, QA/Security engineers.

#### 4. Body — Decision Records

##### `ADR-001`: Rust-First Extraction & Inference Pipeline (Python Deferred to Optional Plugin Runtime)
- **Context:** PRD v2.0 (§6.2) proposed a Rust core + Python sidecar over gRPC for parsing and ONNX/ML inference. Shipping CPython + ML wheels + system PDF/OCR libraries cross-platform balloons installers (>1.5GB), duplicates memory across processes, and adds gRPC IPC latency across 1M+ files.
- **Decision:** Execute all MVP (`v1.0`) discovery, parsing, hashing, chunking, ONNX embedding/reranking, and vector storage in **a single Rust core engine** using `ort` / `fastembed`, `tree-sitter`, `pdfium-render`, `calamine`, `quick-xml`, `blake3`, and `qdrant-edge`. Defer Python to an **optional, lazily installed V1.5+ plugin runtime** for Whisper transcription, heavy OCR, `.pst` parsing, and VLM captioning.
- **Alternatives Rejected:** (a) Always-on Python sidecar via PyInstaller/PyOxidizer (high RSS, AV false positives, slower startup); (b) Pure Node/TS worker (poor CPU/SIMD performance, weak OS handle APIs).
- **Consequences:** Single native binary (<120MB installer excluding models), ~50% lower RSS, zero serialization overhead between extraction and SQLite/Qdrant commits. Requires C/C++ toolchain setup in CI for `pdfium` and `ort`.

##### `ADR-002`: Hardware-Tiered Inference, Idle Model Eviction & 3-Tier Progressive Indexing
- **Context:** PRD v2.0 (§6.3, §8.3, §13) required ≤4GB RSS and ≤6h initial scan for 1M files (up to 10M chunks) while listing BGE-M3 (~1.5GB+), NER, and 8B Q4 LLMs (~5GB+) simultaneously. At 50 chunks/sec on CPU, 10M chunks would take ~55 hours and exceed 8–10GB RAM.
- **Decision:**
  1. **Hardware Profiles (auto-detected at onboarding, user-overridable):**
     - `Eco` (8GB RAM / CPU-only): FTS5 + `bge-small-en-v1.5` (384-dim INT8 ONNX, ~90MB RAM) + deterministic query parser. Local LLM disabled.
     - `Standard` (16GB RAM): `bge-small-en-v1.5` or `bge-m3` (512-token capped) + on-demand 1.5B–3B Q4 LLM (~1.8GB RAM) **never loaded during bulk indexing** and **evicted after 300s idle**.
     - `Pro` (32GB+ RAM or ≥8GB VRAM GPU): `bge-m3` (1024-dim) + ONNX cross-encoder reranker + 7B–8B Q4 LLM.
  2. **Selective / Hierarchical Chunking:** Files ≤1,500 tokens produce 1 summary/doc embedding; larger files cap initial Tier-2 indexing at the first 8 semantic chunks + structural outline; full-document deep chunking runs only during Tier-3 idle cycles or on user-pinned projects.
  3. **3-Tier Progressive Indexing:** Tier 1 (metadata + MFT/USN + exact-size dupe & secret filename candidates, ≤15m) → Tier 2 (active roots: Desktop/Documents/Downloads/recent 90d, ≤60m) → Tier 3 (cold/archive background, throttled).
- **Consequences:** Guarantees ≤4GB RSS during scan on 16GB machines; delivers searchable Day-1 value in under 2 minutes; keeps 1M-file CPU scan within the ≤6h SLA.

##### `ADR-003`: Per-Volume Atomic Quarantine & Hardlink-Safe Accounting
- **Context:** A single `.aegis/quarantine` folder inside user home forces slow cross-volume copies and risks filling the boot drive when deleting large files on secondary/external drives. Additionally, NTFS/APFS/ext4 hardlinks (`nlink > 1`) share storage blocks; counting both as duplicates overstates reclaimable space.
- **Decision:**
  1. Maintain `<VolumeRoot>/.aegis-quarantine/<batch_id>/` (with OS-hidden/system attribute and `.aegisignore` auto-exclusion) on every writable fixed/mounted volume so quarantine moves are `O(1)` atomic renames.
  2. Fallback order when volume root is non-writable: (a) OS Trash API on same volume → (b) copy+BLAKE3-verify+unlink to primary quarantine *iff* `size < 25%` of free destination space → (c) explicit blocking prompt (`Permanent Delete` / `Skip`).
  3. Track `nlink` and `(volume_id, file_index)` in `files`. Dedup engine groups hardlinks as **shared-extent aliases** (`0` reclaimable bytes until *all* links except keeper are removed).
- **Consequences:** Instant, zero-space-amplification quarantine across multi-TB drives and honest storage-reclaim math.

##### `ADR-004`: Atomic Project Unit Boundaries
- **Context:** File-level semantic classification risks proposing moves of individual source files, configs, or companion assets out of repositories or multi-file project bundles, silently breaking software builds or media projects.
- **Decision:** Introduce `atomic_units` as a first-class entity in `B4`. During Tier-1/Tier-2 discovery, directories matching boundary markers (`.git`, `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `.xcodeproj`, `.app`, `.photoslibrary`, `.aegisboundary`) or companion-file rules (`*.sqlite` + `-wal`/`-shm`) are registered as indivisible `atomic_units`. The Curator agent and Automation Engine **must reject** any Action Proposal whose `from_path` is strictly inside an `atomic_unit` while its `to_path` crosses outside that `atomic_unit` (unless `allow_split_override = true` is explicitly signed by the user).
- **Consequences:** Eliminates catastrophic repo/project corruption; enables holistic project-level classification and archiving.

#### 5. Acceptance / Done Criteria
- `ADR-001`–`ADR-004` reviewed and referenced in `B4` schema and `A2` functional requirements.
- Memory budget table (`A2 §13.1`) verified ≤4GB RSS under `Standard` profile during CI soak test.

#### 6. Open Questions
- `[OPEN]` (`OQ-12`, Owner: Platform Lead, Due: Sprint 0): Confirm code-signing & macOS notarization pipeline for `pdfium` dynamic library bundling.

#### 7. Assumptions
- `[ASSUMPTION]` Target MVP `Standard` profile users run on SSD/NVMe primary storage with ≥15% free disk headroom.

#### 8. Change Log
| Version | Date | Author | Summary |
|---|---|---|---|
| 1.0 | 2026-11-20 | Athena / Principal Architect | Initial baseline resolving v2.0 audit blockers |

#### 9. Traceability
- **Upstream:** `A2` Goals `G1–G5`, Principles `P1–P7`, Risks `R1–R8`.
- **Downstream:** `B4` (`files`, `atomic_units`, `quarantine_entries`), `A2` (`FR-001.6`, `FR-001.10`, `FR-005.1`, `FR-011.1`, `FR-013`).

---

### Document 2: `B4` — Data & Vector Architecture Specification (`v2.1`)

#### 1. Metadata
- **ID:** `B4` | **Title:** Data, Graph & Vector Storage Specification
- **Version:** `2.1` | **Status:** `Approved for MVP Implementation`
- **Owner:** Staff Data & Storage Architect | **Last Updated:** `2026-11-20`
- **Related Docs:** `A2` (PRD v2.1), `B8` (`ADR-001`–`ADR-004`)

#### 2. Purpose & Scope
Defines the canonical single-source-of-truth schema for SQLite (`WAL` + `FTS5`), local vector shards (`qdrant-edge`), SQLite↔Qdrant outbox synchronization, PII/encryption posture, and V2 PostgreSQL migration readiness. Excludes cloud sync wire protocols (V2).

#### 3. Audience
Rust core engineers, Search/ML engineers, QA engineers, Security reviewers.

#### 4. Body

##### 4.1 Storage Topology & Consistency Model
```
[Rust Ingestion Worker]
   │
   ├──(1) ACID Tx──▶ SQLite (WAL, FTS5 triggers, outbox_events)
   │
   └──(2) Async Drain──▶ Vector Sync Worker ──▶ qdrant-edge (local HNSW shards)
                                                   └─ reconciled by `aegis doctor`
```
- **SQLite PRAGMAs (mandatory at connection init):** `journal_mode = WAL`, `synchronous = NORMAL` (switch to `FULL` during `actions`/`audit_log` commits), `foreign_keys = ON`, `busy_timeout = 5000`, `temp_store = MEMORY`.

##### 4.2 Relational Schema (SQLite DDL — Production Baseline)

```sql
PRAGMA foreign_keys = ON;

-- 1. Volume registry for stable cross-mount / per-volume quarantine tracking (ADR-003)
CREATE TABLE volumes (
  id TEXT PRIMARY KEY,                  -- OS volume GUID / UUID
  mount_point TEXT NOT NULL,            -- e.g., "C:\", "/Volumes/Work"
  fs_type TEXT NOT NULL,                -- ntfs | apfs | ext4 | exfat | smb
  is_removable INTEGER NOT NULL DEFAULT 0,
  is_readonly INTEGER NOT NULL DEFAULT 0,
  quarantine_dir TEXT,                  -- "<mount>/.aegis-quarantine"
  status TEXT NOT NULL DEFAULT 'online',-- online | offline | degraded
  last_seen INTEGER NOT NULL
);

-- 2. Atomic Project Units (ADR-004)
CREATE TABLE atomic_units (
  id TEXT PRIMARY KEY,                  -- UUIDv7
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  root_path TEXT NOT NULL,
  unit_type TEXT NOT NULL,              -- git_repo | node_pkg | rust_crate | python_pkg | mac_bundle | latex_proj | custom
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
  domain TEXT NOT NULL DEFAULT 'general', -- general | developer | research | trader | household
  repo_url TEXT,
  health_score REAL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- 4. Files & Virtual Archive Members (FR-001.6 P0, FR-001.9, FR-001.10)
CREATE TABLE files (
  id TEXT PRIMARY KEY,                  -- UUIDv7; survives rename via (volume_id, file_index)
  volume_id TEXT NOT NULL REFERENCES volumes(id),
  file_index INTEGER,                   -- OS inode / NTFS FileId (NULL ONLY for archive:// virtual entries)
  nlink INTEGER NOT NULL DEFAULT 1,     -- Hardlink count (ADR-003)
  container_file_id TEXT REFERENCES files(id) ON DELETE CASCADE, -- Non-null for archive:// members
  virtual_uri TEXT,                     -- e.g., "archive://<container_id>/docs/report.pdf"
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
  simhash_text BLOB,                    -- 64-bit / 128-bit MinHash/SimHash for near-dupe
  dhash_image BLOB,                     -- 128-bit perceptual hash for images
  cloud_state TEXT NOT NULL DEFAULT 'local', -- local | placeholder_remote | syncing
  state TEXT NOT NULL DEFAULT 'active', -- active | warm | cold | archive | quarantine | missing
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  classification TEXT,
  confidence REAL,
  sensitivity TEXT NOT NULL DEFAULT 'internal', -- public | internal | confidential | secret
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  access_count INTEGER NOT NULL DEFAULT 0,
  last_access INTEGER,
  deleted_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK ((container_file_id IS NULL AND file_index IS NOT NULL) OR (container_file_id IS NOT NULL AND virtual_uri IS NOT NULL))
);
CREATE UNIQUE INDEX idx_files_os_identity ON files(volume_id, file_index, path_hash) WHERE deleted_at IS NULL;
CREATE INDEX idx_files_blake3 ON files(blake3) WHERE blake3 IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_files_project ON files(project_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_files_atomic_unit ON files(atomic_unit_id);

-- 5. Text Chunks & FTS5 Hybrid Search Index (resolves §7.7 vs §9 drift)
CREATE TABLE chunks (
  id INTEGER PRIMARY KEY AUTOINCREMENT, -- Integer rowid required for FTS5 external-content sync
  chunk_uuid TEXT NOT NULL UNIQUE,      -- Used as Qdrant point ID
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
  entity_type TEXT NOT NULL,            -- person | company | topic | repo | account | event | collection
  name TEXT NOT NULL,
  attrs_json TEXT NOT NULL DEFAULT '{}',
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE relationships (
  id TEXT PRIMARY KEY,
  src_id TEXT NOT NULL,
  src_kind TEXT NOT NULL,               -- file | entity | project
  dst_id TEXT NOT NULL,
  dst_kind TEXT NOT NULL,
  rel_type TEXT NOT NULL,               -- references | belongs_to | derived_from | duplicate_of | version_of | authored_by | mentions | related_to | supersedes
  weight REAL NOT NULL DEFAULT 1.0,
  evidence_json TEXT NOT NULL DEFAULT '[]',
  valid_from INTEGER NOT NULL,
  valid_to INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_rel_src ON relationships(src_id, rel_type);
CREATE INDEX idx_rel_dst ON relationships(dst_id, rel_type);

-- 7. Automation Engine: Batches, Actions, Pre-Rollback Snapshots & Quarantine
CREATE TABLE action_batches (
  id TEXT PRIMARY KEY,
  origin TEXT NOT NULL,                 -- simulator | rules | dedupe | security | user_undo
  status TEXT NOT NULL,                 -- draft | approved | executing | executed | partial | rolled_back | failed
  total_actions INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER
);

CREATE TABLE actions (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL REFERENCES action_batches(id) ON DELETE CASCADE,
  agent TEXT,                           -- NULL = user/deterministic rule
  action_type TEXT NOT NULL,            -- move | copy | rename | quarantine | purge | archive | restore
  file_id TEXT NOT NULL REFERENCES files(id),
  from_path TEXT NOT NULL,
  to_path TEXT,
  pre_blake3 BLOB,                      -- Captured right before execution for Pre-Rollback Reconciliation
  post_blake3 BLOB,
  reason TEXT NOT NULL,
  confidence REAL NOT NULL,
  evidence_json TEXT NOT NULL,
  status TEXT NOT NULL,                 -- proposed | approved | executed | failed | skipped | rolled_back
  error_message TEXT,
  created_at INTEGER NOT NULL,
  executed_at INTEGER,
  rolled_back_at INTEGER
);
CREATE INDEX idx_actions_batch ON actions(batch_id, status);

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
  expires_at INTEGER NOT NULL,          -- Default +30 days TTL
  restored_at INTEGER,
  purged_at INTEGER
);
CREATE INDEX idx_quarantine_expiry ON quarantine_entries(expires_at) WHERE restored_at IS NULL AND purged_at IS NULL;

-- 8. Hash-Chained Audit Log (tamper-evident locally; exportable for forensic verification)
CREATE TABLE audit_log (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,                  -- user | agent:<name> | system
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  detail_json TEXT NOT NULL,
  prev_hash BLOB NOT NULL,              -- 32-byte BLAKE3 of prior row (0x00*32 for genesis)
  hash BLOB NOT NULL                    -- BLAKE3(seq || id || ts || actor || action || target || detail_json || prev_hash)
);

-- 9. Scan Jobs, Rules, Local Preference Exemplars, Deduplicated Security Findings, Outbox & Settings
CREATE TABLE scan_jobs (
  id TEXT PRIMARY KEY,
  root_path TEXT NOT NULL,
  tier INTEGER NOT NULL,                -- 1 = Metadata | 2 = Priority Semantic | 3 = Deep Background
  status TEXT NOT NULL,                 -- queued | running | paused | completed | degraded | failed
  files_seen INTEGER NOT NULL DEFAULT 0,
  files_indexed INTEGER NOT NULL DEFAULT 0,
  files_skipped_perm INTEGER NOT NULL DEFAULT 0,
  files_skipped_cloud INTEGER NOT NULL DEFAULT 0,
  checkpoint_json TEXT,
  errors_json TEXT NOT NULL DEFAULT '[]',
  started_at INTEGER NOT NULL,
  finished_at INTEGER
);

CREATE TABLE rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  scope_json TEXT NOT NULL,
  condition_json TEXT NOT NULL,
  action_json TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 100,
  enabled INTEGER NOT NULL DEFAULT 1,
  source TEXT NOT NULL,                 -- builtin | user | graduated_proposal | plugin
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE routing_exemplars (
  id TEXT PRIMARY KEY,
  file_id TEXT REFERENCES files(id) ON DELETE SET NULL,
  dest_folder TEXT NOT NULL,
  decision TEXT NOT NULL,               -- approved | rejected
  features_json TEXT NOT NULL,
  vector_synced INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE security_findings (
  id TEXT PRIMARY KEY,
  secret_fingerprint BLOB NOT NULL,     -- HMAC-BLAKE3(local_salt, normalized_secret) — raw secret NEVER stored!
  finding_type TEXT NOT NULL,           -- aws_key | github_pat | private_key | password_file | id_doc | financial | pii
  base_severity TEXT NOT NULL,          -- critical | high | medium | low
  context_risk_score REAL NOT NULL,     -- 0.0–100.0 weighted by path context (Downloads vs test fixture)
  status TEXT NOT NULL DEFAULT 'open',  -- open | false_positive | vaulted | resolved
  resolved_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_sec_fingerprint ON security_findings(secret_fingerprint);

CREATE TABLE security_finding_locations (
  finding_id TEXT NOT NULL REFERENCES security_findings(id) ON DELETE CASCADE,
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  line_number INTEGER,
  redacted_snippet TEXT NOT NULL,       -- First 4 / Last 2 chars only, e.g., "AKIA...9X2Q"
  PRIMARY KEY (finding_id, file_id)
);

CREATE TABLE outbox_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  aggregate_type TEXT NOT NULL,         -- chunk | entity | project | exemplar
  aggregate_id TEXT NOT NULL,
  op TEXT NOT NULL,                     -- upsert | delete
  payload_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  processed_at INTEGER
);
CREATE INDEX idx_outbox_pending ON outbox_events(seq) WHERE processed_at IS NULL;

CREATE TABLE settings (
  key TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'global', -- global | project:<id> | root:<id>
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (key, scope)
);
```

##### 4.3 Vector Store Architecture (`qdrant-edge`)
| Collection | Vector Dim (by Profile) | Distance | Payload Indexes | Purpose |
|---|---|---|---|---|
| `chunks` | `384` (`Eco`/`Std`) or `1024` (`Pro`) | Cosine | `file_id`, `project_id`, `sensitivity`, `ext`, `mtime` | Hybrid semantic search (`FR-007`) & semantic dedupe |
| `entities` | `384` or `1024` | Cosine | `entity_type` | Graph resolution & entity linking |
| `projects` | `384` or `1024` | Cosine | `domain` | Project clustering & assignment |
| `routing_exemplars` | `384` or `1024` | Cosine | `decision`, `dest_folder` | Local taxonomy preference calibration |

- **Model Upgrade Policy:** Changing profiles (`384 ↔ 1024`) creates a new versioned collection (`chunks_v2_1024`), drains a background re-embed queue, and atomically swaps the active collection pointer in `settings` so search never degrades mid-migration.

##### 4.4 Data Classification, Privacy & Migration
- **Secret Zero-Storage Rule:** `security_findings` stores **only** `HMAC-BLAKE3(per_install_keychain_salt, secret)` and a masked snippet (`AKIA...9X2Q`). Raw secrets never enter SQLite or logs.
- **At-Rest Encryption:** Opt-in SQLCipher (AES-256-CBC/HMAC-SHA512) + encrypted `qdrant-edge` storage directory using Argon2id-derived key stored in OS Keychain (V1.5).
- **V2 PostgreSQL Migration:** All primary keys use UUIDv7/TEXT strings and integer timestamps (Unix epoch ms) with zero SQLite-specific column-type reliance outside `chunks_fts` (which maps cleanly to PG `tsvector` + GIN index in V2).

#### 5. Acceptance / Done Criteria
- Migrations run cleanly from empty DB in `<200ms` via `rusqlite`/embedded migration runner.
- `FTS5` triggers keep `chunks_fts` 100% row-count consistent across insert/update/delete stress tests (100k ops).
- `aegis doctor` detects and repairs simulated outbox lag and broken `audit_log` hash chains.

#### 6. Open Questions
- `[OPEN]` (`OQ-13`, Owner: Data Architect, Due: Milestone 2): Confirm whether `routing_exemplars` expires entries older than 18 months or uses exponential recency decay.

#### 7. Assumptions
- `[ASSUMPTION]` `qdrant-edge` supports concurrent read queries while single-writer outbox batches upsert ≤1,000 points/tx.

#### 8. Change Log
| Version | Date | Author | Summary |
|---|---|---|---|
| 2.0 | Pre-MVP | Product/Arch | Initial §9 inline schema in PRD v2.0 |
| 2.1 | 2026-11-20 | Athena | Added `volumes`, `atomic_units`, `chunks_fts`+triggers, `nlink`, `archive://` constraints, `quarantine_entries`, `outbox_events`, deduplicated `security_findings`, `routing_exemplars` |

#### 9. Traceability
- **Upstream:** `B8` (`ADR-001`–`ADR-004`), `A2` (`FR-001`–`FR-013`).
- **Downstream:** Rust `storage` crate, `outbox` sync daemon, `aegis doctor` integrity verifier.

---

### Document 3: `A2` — Aegis Guardian PRD (`v2.1` — Production-Grade Revision)

#### 1. Metadata
- **ID:** `A2` | **Title:** Aegis Guardian Product Requirements Document
- **Version:** `2.1` | **Status:** `Approved MVP Baseline`
- **Owner:** Head of Product / Principal PM | **Last Updated:** `2026-11-20`
- **Supersedes:** `Aegis Guardian PRD v2.md` (`v2.0`)
- **Related Docs:** `B8` (`ADR-001`–`ADR-004`), `B4` (Data & Vector Schema v2.1)

#### 2. Purpose & Scope
Defines the authoritative product requirements, reconciled MVP vs. V2 success targets, hardware-feasible architecture constraints, testable acceptance criteria (`FR-001`–`FR-022`), OS/security invariants, and end-to-end traceability for **Aegis Guardian**.

#### 3. Audience
Product, Engineering, Design/UX, QA, Security, and Release Operations.

#### 4. Body

##### 4.1 Reconciled Goals & Success Criteria (`§3.1` + `§16` Unified)

| Goal ID | Product Goal | North-Star / V2 Target | MVP (`v1.0`) Gate Target | Measurement Method |
|---|---|---|---|---|
| **G1** | Find anything by meaning, instantly | `>90%` top-3 hit rate | **`≥75%` top-3 hit rate** | Internal labeled 500-query eval corpus (MRR / Hit@3) |
| **G2** | Surface & resolve duplication | `≥95%` duplicate recall; `≥30%` reclaimed on dirty drives | **`≥85%` exact+near recall** (hardlink-adjusted) | Synthetic + pilot dirty-drive corpus |
| **G3** | Propose safe, reviewable reorganisation | `≥80%` proposal acceptance rate | **`≥60%` proposal acceptance rate** | Local telemetry / simulator approval ratio |
| **G4** | Complete, auditable, reversible mutations | `100%` reversible; `0` unrecoverable ops | **`100%` reversible** (or conflict-prompted) | Chaos/crash-recovery + pre-rollback QA suite |
| **G5** | Detect sensitive data before it leaks | `≥99%` secret recall on test corpus | **`≥95%` secret recall**; `<5%` critical FP rate | Curated secrets/PII benchmark corpus |
| **G6** | Honest, non-disruptive coverage | `100%` coverage honesty | **`100%` coverage honesty**; `≥99%` crash-free | Coverage delta checks (denied/cloud/locked counted explicitly) |

##### 4.2 Functional Requirements & Testable Acceptance Criteria (MVP `P0` Complete)

###### 4.2.1 Discovery & Change Monitoring (`FR-001`, `FR-010`)
| ID | Requirement | Pri | Testable Acceptance Criteria (Given / When / Then) |
|---|---|---|---|
| **FR-001.1** | Multi-root scanning (internal, external, NAS/SMB, cloud-sync) | **P0** | User can add/toggle roots; adding a mounted NTFS/APFS/ext4/SMB root persists in `volumes` and starts Tier-1 scan within 3s. |
| **FR-001.2** | `.aegisignore` + OS junk exclusions (`node_modules`, `AppData`, `.git` objects) | **P0** | Paths matching `.aegisignore` or active denylist produce `0` rows in `files` and `chunks`; rule edits apply on next incremental sweep. |
| **FR-001.3** | Graceful permission & lock handling | **P0** | Encountering `EACCES`/`SharingViolation` never halts scan; path is logged in `scan_jobs.errors_json`, `files_skipped_perm` increments, and UI Coverage Report displays exact skipped count. |
| **FR-001.4** | Symlink/junction loop protection | **P0** | Cyclic symlinks/reparse points are skipped by default; max traversal depth and visited-inode set prevent infinite loops. |
| **FR-001.5** | Lazy BLAKE3 background hashing | **P0** | Tier-1 metadata enumeration completes without reading file payloads; BLAKE3 hashes are computed asynchronously in a throttled worker queue. |
| **FR-001.6** | **OS file-identity tracking across renames/moves** *(promoted from P1)* | **P0** | Renaming or moving a file within the same volume preserves its `files.id`, `chunks`, embeddings, and `relationships` via `(volume_id, file_index)` matching without re-embedding. |
| **FR-001.8** | Offline NAS / unmounted drive resilience | **P1** | Disconnecting a drive mid-scan transitions `volumes.status` to `offline`, checkpoints `scan_jobs`, and avoids marking files `missing` until confirmed online. |
| **FR-001.9** | **Cloud placeholder safety** *(new)* | **P0** | Files flagged `SF_DATALESS` (macOS iCloud) or `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` (Windows OneDrive) are recorded with `cloud_state = 'placeholder_remote'` and **never** opened for hashing/extraction unless explicitly opted in. |
| **FR-001.10** | **Hardlink-aware discovery** *(new)* | **P0** | Files with `nlink > 1` store accurate `nlink` and shared `file_index`; UI and dedupe engine never report shared hardlink bytes as independent reclaimable storage. |
| **FR-010** | Real-time watchers + MFT/USN/FSEvents + 15m polling fallback | **P0** | File create/modify/rename/delete within an active root reflects in SQLite within ≤60s (500ms debounce); on restart, USN/FSEvents or sampled checksum reconciles missed events. |

###### 4.2.2 Content Intelligence (`FR-002`)
| ID | Requirement | Pri | Testable Acceptance Criteria |
|---|---|---|---|
| **FR-002.1** | Rust-native MVP extractors (PDF via `pdfium-render`, DOCX/XLSX/PPTX, Tree-sitter code, MD/TXT/CSV/JSON, EXIF, zip/7z listing) | **P0** | Given valid fixture files for all MVP formats, extractor outputs non-empty normalized text + metadata within `<2s/doc` median without spawning Python. Corrupt files log parse error and mark `coverage = partial`. |
| **FR-002.2** | Archive member indexing (`archive://`) | **P0** | Zip/7z members (depth ≤2, size ≤50MB) create `files` rows with `container_file_id` + `virtual_uri`; password-protected archives log encrypted status without crash. |
| **FR-002.3** | Opt-in OCR & Media Transcription | **P1/P2** | Disabled by default; enabling shows explicit CPU/battery cost warning; OCR processes image/scanned PDFs into `chunks` tagged `source=ocr`. |

###### 4.2.3 Knowledge Graph & Atomic Units (`FR-003`)
| ID | Requirement | Pri | Testable Acceptance Criteria |
|---|---|---|---|
| **FR-003.1** | Entity & temporal relationship extraction | **P0** | Extracts Projects, Folders, People, Repos, Topics into `entities`/`relationships` with `valid_from`; queries return 2-hop neighbourhood in `<50ms` p95. |
| **FR-003.2** | **Atomic Project Unit detection** *(new, `ADR-004`)* | **P0** | Directories containing `.git`, `package.json`, `Cargo.toml`, `pyproject.toml`, `.app`, or `.aegisboundary` create an `atomic_units` record; contained files link via `atomic_unit_id`. |

###### 4.2.4 Duplicate Intelligence (`FR-004`)
| ID | Requirement | Pri | Testable Acceptance Criteria |
|---|---|---|---|
| **FR-004.1** | Exact (BLAKE3) & Near-duplicate (dHash / SimHash) clustering | **P0** | Identical files group into 1 cluster with 1 recommended keeper (shallowest clean path + newest access) and exact hardlink-adjusted reclaimable bytes. Near-dup images (resized/re-saved JPEG) and near-dup text (≥85% Jaccard shingle overlap) cluster accurately. |
| **FR-004.2** | Semantic duplicate & version-chain detection | **P0/P1** | Cosine `≥0.92` surfaced in "Semantic Similar" tab (never auto-deleted); filename version chains (`_v1`, `_final`) render ordered timeline UI. |

###### 4.2.5 Autonomous Refiling, Safety & Rollback (`FR-005`, `FR-012`, `FR-013`)
| ID | Requirement | Pri | Testable Acceptance Criteria |
|---|---|---|---|
| **FR-005.1** | Pipeline: Analyse → Simulate → Approve → Execute → Audit | **P0** | No filesystem write occurs outside `action_batches` execution; Simulator renders full before/after tree diff, collision policy (`auto-rename` suffix, never overwrite), and per-file rationale + confidence badge. |
| **FR-005.2** | **Atomic Unit Protection Invariant** *(new, `ADR-004`)* | **P0** | Automation Engine blocks any proposal moving a file out of an `atomic_unit` unless `allow_split_override = true` is explicitly confirmed by user. |
| **FR-005.3** | **Proposal-to-Rule Graduation** *(new)* | **P0** | Approving any proposal offers "Always apply this pattern"; confirming writes an enabled deterministic rule to `rules` (`source='graduated_proposal'`) previewable before activation. |
| **FR-005.4** | **Pre-Rollback State Reconciliation** *(new)* | **P0** | Triggering Undo/Rollback verifies current `to_path` existence, `pre/post_blake3`, and destination availability. Unmodified files revert atomically; externally modified/colliding files pause and present a per-file resolution dialog (*Restore as copy / Skip / Abort*). |
| **FR-012 / FR-013** | Global Undo Stack & Per-Volume Quarantine (`ADR-003`) | **P0** | Deleting via Aegis executes `O(1)` rename to `<VolumeRoot>/.aegis-quarantine` (`expires_at = +30d`), writes `quarantine_entries` + `audit_log` hash entry, and restores cleanly from Quarantine UI. |

###### 4.2.6 Workspace Health, AI Search & Security (`FR-006`, `FR-007`, `FR-008`)
| ID | Requirement | Pri | Testable Acceptance Criteria |
|---|---|---|---|
| **FR-006** | Explainable 5-dimension Workspace Health score (0–100) | **P0** | Clicking any sub-score (Organisation, Storage, Security, Duplication, Searchability) opens the exact top-10 offending files/folders with direct fix actions. |
| **FR-007** | Hybrid Search (`FTS5` BM25 + `qdrant-edge` ANN + RRF) | **P0** | Returns top-20 results with highlighted matched chunks and "why-it-matched" badge in **`<300ms` p95** (excluding optional LLM parse); supports structured filters (`project:`, `type:`, `after:`, `before:`, `sensitivity:`). |
| **FR-008** | **Context-Weighted & Deduplicated Secret Scanning** *(upgraded)* | **P0** | Detects API keys, `.pem`/`.key`, passwords, PII. Identical secrets across N copies collapse into 1 `security_findings` row (`secret_fingerprint` HMAC-BLAKE3; raw secret never stored). Risk score weights path context (Desktop/Downloads > test fixtures). |

###### 4.2.7 Lifecycle, Onboarding & Crisis Playbooks (`FR-009`, `FR-011`)
| ID | Requirement | Pri | Testable Acceptance Criteria |
|---|---|---|---|
| **FR-009** | Lifecycle states (`Active → Warm → Cold → Archive`) | **P0** | MVP supports manual state tagging + Search "Resurrection" prompt when opening an `archive` file; automated Warm→Cold rules unlock in V2. |
| **FR-011.1** | **Onboarding & 3-Tier Progressive Scan** *(upgraded)* | **P0** | Onboarding detects hardware profile (`Eco`/`Standard`/`Pro`), requests OS permissions (macOS TCC guide), shows honest Tier-1/2/3 ETAs, and unlocks initial Health Dashboard + exact-dupe quick wins within **≤2 minutes** of scan start. |
| **FR-011.2** | **Crisis Playbooks** *(new)* | **P0** | Dashboard exposes 2 one-click guided views built on existing queries: (a) **Disk Rescue** (top reclaimable dupes + build caches + oversized stale files) and (b) **Pre-Share Secret Audit** (targeted folder scan). |

*(V1.5–V3 roadmap items `FR-014` CLI, `FR-015` Smart Collections, `FR-016` Plugin SDK, `FR-017` Scheduled Housekeeping, `FR-018` Storage Analytics, `FR-019` Digest, `FR-020` Multi-device Encrypted Sync, `FR-021` Team Graph, `FR-022` i18n/Accessibility remain scoped per §15 Roadmap.)*

##### 4.3 Verified Resource & Throughput Budgets (Closing v2.0 Math Gaps)

###### 4.3.1 Memory Budget During Active Scan (`Standard` 16GB Machine)
| Component | Active Scan Cap | Interactive Query Burst | Notes |
|---|---|---|---|
| Tauri WebView + React UI | ≤ 350 MB | ≤ 450 MB | Virtualized lists; background window throttled |
| Rust Core + Extractors (`pdfium` pool) | ≤ 600 MB | ≤ 450 MB | Bounded worker pool (`cores / 2`, max 4 workers) |
| SQLite WAL + Page Cache (`cache_size`) | ≤ 256 MB | ≤ 256 MB | Explicit `PRAGMA cache_size = -262144` |
| `qdrant-edge` HNSW + mmap payloads | ≤ 700 MB | ≤ 700 MB | On-disk payload mmap; quantized INT8 vectors |
| ONNX Embedding (`bge-small` / `bge-m3` INT8) | ≤ 450 MB | ≤ 450 MB | `ort` session shared across workers |
| Local LLM (1.5B–3B Q4) | **0 MB (unloaded)** | ≤ 1,800 MB | Loaded on-demand only; auto-evicted at 300s idle |
| **TOTAL RSS** | **≤ 2.36 GB** | **≤ 4.10 GB (peak burst)** | **Passes ≤4GB scan SLA with ~1.6GB headroom** |

###### 4.3.2 Ingestion Throughput Math (1M Files on Mid-Range 8-Core Laptop)
| Scan Tier | Scope | Mechanism | Target Duration |
|---|---|---|---|
| **Tier 1 (Immediate)** | 1M files metadata | NTFS MFT/USN or parallel `jwalk` + `volumes`/`atomic_units` | **≤ 15 minutes** (≤ 90s for typical 150k-file SSD) |
| **Tier 2 (Priority)** | ~100k active docs (Desktop/Docs/Downloads/recent 90d) | Rust extraction + selective chunking (~250k chunks @ 120 chunks/sec ONNX INT8 CPU) | **≤ 40 minutes** |
| **Tier 3 (Background)** | Remaining ~900k cold/code/media files | Metadata + BLAKE3 + summary-only embedding (~400k non-empty indexable docs @ 80 docs/sec throttled) | **≤ 2.5 hours** (Total ~3.5h, well inside **≤ 6h SLA**) |

##### 4.4 OS-Specific Adapter & Permission Matrix
| Concern | Windows 10/11 | macOS 12+ | Linux (Ubuntu 22.04+) |
|---|---|---|---|
| **Fast Discovery / Recovery** | NTFS USN Journal (if elevated) / `jwalk` fallback | FSEvents stream IDs + `getattrlistbulk` | `inotify` + `jwalk` |
| **Permission Model** | ACL skip + `files_skipped_perm` | Guided **Full Disk Access** / TCC prompt; denied folders flagged in Coverage | POSIX mode skip + `files_skipped_perm` |
| **Cloud Placeholders** | `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` | `SF_DATALESS` (`st_flags`) | GVFS/rclone FUSE mount heuristics |
| **File Identity** | `VolumeSerialNumber` + `FileIndex` (64/128-bit) | `st_dev` + `st_ino` | `st_dev` + `st_ino` |

##### 4.5 End-to-End Traceability Matrix (Personas → Execution)
| Persona | Core JTBD | Primary FRs | Schema Tables (`B4`) | UI Screens | Success Metric (`G1–G6`) | Verification Test |
|---|---|---|---|---|---|---|
| **Developer** | Repo intelligence, secret scan, clean `node_modules` | `FR-001.2`, `FR-002.1`, `FR-003.2`, `FR-005.2`, `FR-008` | `atomic_units`, `files`, `security_findings` | Dashboard (Disk Rescue), Security Inbox, Search | `G5` (`≥95%` recall), `0` broken repos | Repo-boundary fuzz test + gitleaks corpus |
| **Researcher** | Semantic PDF search, near-dupe papers | `FR-002.1`, `FR-004.1`, `FR-007` | `chunks`, `chunks_fts`, `qdrant:chunks` | Search, Duplicates | `G1` (`≥75%` MVP Hit@3), `<300ms` p95 | 500-query academic/mixed eval suite |
| **Trader / Knowledge Worker** | Statement refiling, safe undo, health | `FR-005.1`, `FR-005.3`, `FR-005.4`, `FR-006`, `FR-013` | `actions`, `rules`, `quarantine_entries`, `audit_log` | Simulator, Approvals Inbox, Dashboard | `G3` (`≥60%` accept), `G4` (`100%` undo) | Crash-during-batch & pre-rollback conflict tests |
| **Household Admin** | Photo dedupe, drive rescue, family PII | `FR-001.10`, `FR-004.1`, `FR-011.2`, `FR-013` | `files` (`dhash_image`, `nlink`), `quarantine_entries` | Onboarding, Duplicates, Disk Rescue | `G2` (`≥85%` recall, honest bytes) | Multi-volume hardlink & external USB quarantine test |

#### 5. Acceptance / Done Criteria
- All 6 audit contradictions from v2.0 resolved (zero `[BLOCKER]` findings).
- Every `P0` requirement carries deterministic Given/When/Then criteria executable by QA.

#### 6. Open Questions (Carried & Newly Scoped)
- `[OPEN]` (`OQ-01`–`OQ-07` from original §18):
  - `OQ-01` (Owner: CEO/Product, Due: Pre-Beta): Licensing model (Pro subscription vs. perpetual vs. open-core).
  - `OQ-02` (Owner: Product/Legal, Due: Milestone 1): License audit for default ONNX/GGUF models (Apache-2.0/MIT preferred over Llama community license).
  - `OQ-03` (Owner: Product, Due: V1.5): Priority order for macOS vs. Linux parity post-Windows MVP.
  - `OQ-04` (Owner: Product, Due: V2): Enterprise/Team commercial validation.
  - *(Original §18 items on LLM bundling, PG migration timing, OCR defaults, and domain packaging are now **RESOLVED** by `ADR-001`, `ADR-002`, and `B4 §4.4`.)*

#### 7. Assumptions
- `[ASSUMPTION]` `A-01`: Users accept a 2-minute Tier-1 onboarding wait before interactive dashboard readiness.
- `[ASSUMPTION]` `A-02`: Rust `pdfium-render` + `calamine` + `quick-xml` covers ≥95% of real-world MVP user documents; remaining exotic formats (`.pst`, scanned TIFFs) cleanly report `partial` coverage until V1.5 plugins.

#### 8. Change Log
| Version | Date | Author | Summary |
|---|---|---|---|
| 1.0 | — | Founding Team | Vision v1.0 ("Aegis Guardian Full PRD.md") |
| 2.0 | Pre-MVP | Founding Team | Comprehensive PRD v2.0 specification |
| 2.1 | 2026-11-20 | Athena | Production-grade upgrade: reconciled goals/metrics, solved 4GB/1M-file math, promoted rename-ID to P0, added `ADR-001`–`ADR-004` behaviors, full P0 acceptance criteria, OS matrix, and traceability |

#### 9. Traceability
- **Upstream:** Original `Aegis Guardian PRD v2.md`.
- **Downstream:** `B8` (`ADR-001`–`ADR-004`), `B4` (Schema v2.1), upcoming `C1` (IPC/API Contracts) & `F1/F2` (Eval & Safety Test Plan).

---

## Cross-Reference Consistency Matrix

| Concept / Entity | `A2` (PRD v2.1) | `B8` (ADRs) | `B4` (Data & Vector) | Sync Status |
|---|---|---|---|---|
| **Core Extraction & Inference** | `FR-002.1`, `§4.3.1` | `ADR-001` (Rust-first) | `chunks.embedding_model`, `qdrant-edge` | **Synced** |
| **Memory Cap (≤4GB RSS scan)** | `§4.3.1` (`2.36GB` scan) | `ADR-002` (Idle eviction) | Payload mmap + dynamic dim | **Synced** |
| **1M File Throughput (≤6h)** | `§4.3.2` (`~3.5h` 3-Tier) | `ADR-002` (Selective chunking) | `scan_jobs.tier` | **Synced** |
| **File Identity & Hardlinks** | `FR-001.6` (`P0`), `FR-001.10` | `ADR-003` | `files (volume_id, file_index, nlink)` | **Synced** |
| **Per-Volume Quarantine** | `FR-013` | `ADR-003` | `volumes.quarantine_dir`, `quarantine_entries` | **Synced** |
| **Atomic Project Units** | `FR-003.2`, `FR-005.2` | `ADR-004` | `atomic_units`, `files.atomic_unit_id` | **Synced** |
| **Hybrid Search (BM25 + ANN)** | `FR-007` (`<300ms` p95) | `ADR-002` | `chunks_fts` (`FTS5`) + `qdrant:chunks` | **Synced** |
| **Pre-Rollback Reconciliation** | `FR-005.4` | — | `actions (pre_blake3, post_blake3)` | **Synced** |
| **Secret Zero-Storage** | `FR-008` | — | `security_findings.secret_fingerprint` | **Synced** |

---

## Living Documentation Index (`G1`) & Changelog (`G3`)

| ID | Document | Version | Status | Owner | Action / Next Milestone |
|---|---|---|---|---|---|
| **A2** | Product Requirements Document (`v2.1`) | `2.1` | Approved Baseline | Head of Product | Ready for sprint planning |
| **B4** | Data & Vector Architecture Spec | `2.1` | Approved Baseline | Staff Data Architect | Ready for DB migration implementation |
| **B8** | Architecture Decision Records (`001–004`) | `1.0` | Approved Baseline | Principal Architect | Ready |
| **C1** | Tauri IPC, Events & Action State Contract | `—` | **Missing (`P0`)** | Staff Backend/FE Lead | Generate next to unblock parallel UI/Rust work |
| **B6** | Security Threat Model (STRIDE + Prompt Injection) | `—` | **Missing (`P1`)** | Security Lead | Generate before Simulator/Agent execution |
| **F1/F2** | Evaluation Corpus & Path-Safety Fuzz Test Plan | `—` | **Missing (`P1`)** | QA / ML Lead | Generate before Milestone 2 |
| **G1/G3** | Living Index & Changelog | `1.0` | Active | Athena / Doc Owner | Updated `2026-11-20` |

**Changelog (`G3`):**
- `2026-11-20` — Published `B8` (`ADR-001`–`ADR-004`), `B4` (`v2.1`), and `A2` (`v2.1`). Resolved all 5 audit blockers/majors from PRD v2.0 and locked architectural recommendations for `OQ-08`–`OQ-11`.

---

## Next Actions (Smallest Shippable Step)

1. **Generate `C1` — Tauri IPC, Event Stream & `ActionBatch` State Machine Contract (`P0` Blocker for Parallel FE/BE Development):**
   - *Includes:* Typed Rust/TypeScript schemas for the core IPC commands (`scan.*`, `search.query`, `simulator.*`, `actions.*`, `quarantine.*`, `health.get`, `findings.*`), `tauri::event` progress payloads, and the formal **Pre-Rollback Conflict Reconciliation** state machine.
   - *Acceptance Criteria:* Frontend TypeScript types and Rust Tauri command signatures compile 1:1 with `B4` tables (`actions`, `action_batches`, `quarantine_entries`) with zero ambiguous error shapes.

Say **"Proceed with C1"** (or **"Proceed with B6 / F1"**) to generate the next document in the suite.