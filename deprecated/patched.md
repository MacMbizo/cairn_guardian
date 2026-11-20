Below is the complete, cross-synchronized production documentation package. It:
1. **Resolves all 6 bugs/contradictions** found across your pasted files (`B4 v2.3`, `B8 v2.2`, `C1 v1.1`, `C1_B4_Alignment_Check v1.1`, `F1/F2 v2.2`, `B6 v2.3`, and `A2 v2.1.1` patch).
2. **Produces the missing P0/P1 execution documents** (`C4` Policy Engine Spec, `D3` Critical UX Flows & A11y Baseline, `E2/E4` Packaging & Observability Spec, `G2` Glossary, and updated `suite_reconciliation.md` Living Index).
3. **Flags `B1_B2_Architecture.md`** honestly as `[OPEN — Pending Source Paste]` so nothing is fabricated about an unseen file.

---

## Part A: Patched Core Documents (Bug & Drift Fixes)

```markdown
--- START OF FILE B4_Data_Vector_Schema_v2.3.md ---

# B4 — Data & Vector Architecture Specification (v2.3)

**ID:** B4 · **Version:** 2.3 · **Status:** Approved for MVP Implementation
**Owner:** Staff Data & Storage Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B8_ADRs.md` (v2.2), `C1_IPC_Action_Contracts.md` (v1.1), `C4_Policy_Engine_Confidence_Spec.md`

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
  file_index INTEGER,                   -- OS inode/FileId (NULL ONLY for archive://)
  nlink INTEGER NOT NULL DEFAULT 1,
  container_file_id TEXT REFERENCES files(id) ON DELETE CASCADE,
  virtual_uri TEXT,                     -- "archive://<container_id>/docs/report.pdf"
  atomic_unit_id TEXT REFERENCES atomic_units(id) ON DELETE SET NULL,
  unit_split_override INTEGER NOT NULL DEFAULT 0, -- v2.3 ADD: per-file unlock (B8 ADR-004)
  path TEXT NOT NULL,
  path_hash BLOB NOT NULL,              -- BLAKE3(normalized_path)
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
  CHECK ((container_file_id IS NULL AND file_index IS NOT NULL)
      OR (container_file_id IS NOT NULL AND virtual_uri IS NOT NULL))
);

-- v2.3 FIX: Allows hardlinks (shared file_index, distinct path_hash) while preventing duplicate live paths
CREATE UNIQUE INDEX idx_files_os_path
  ON files(volume_id, file_index, path_hash)
  WHERE file_index IS NOT NULL AND deleted_at IS NULL;
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
  file_id TEXT NOT NULL REFERENCES files(id),
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
  rolled_back_at INTEGER
);
CREATE INDEX idx_actions_batch ON actions(batch_id, status);
CREATE INDEX idx_actions_file_status ON actions(file_id, status)
  WHERE status IN ('proposed','approved');

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
  line_number INTEGER,
  redacted_snippet TEXT NOT NULL,
  PRIMARY KEY (finding_id, file_id)
);

CREATE TABLE outbox_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  op TEXT NOT NULL,
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

### 3.1 Rename vs. Hardlink Resolution Algorithm (Mandatory for Watcher & Scanner)
When encountering `(volume_id, file_index, path, path_hash)` where `path_hash` is not yet live in `files`:
1. Query `SELECT id, path FROM files WHERE volume_id = ? AND file_index = ? AND deleted_at IS NULL`.
2. If zero rows → `INSERT` new file row.
3. If `>= 1` rows exist, check `fs::try_exists(row.path)` for each existing row:
   - If an existing row's `row.path` **no longer exists** on disk → it was a **rename/move**: `UPDATE files SET path = ?, path_hash = ?, name = ?, updated_at = ? WHERE id = row.id` (preserves `id`, `chunks`, embeddings, `relationships`).
   - If **all** existing rows' paths still exist on disk → it is a **hardlink**: `INSERT` new `files` row sharing `(volume_id, file_index)` and update `nlink` across all rows sharing that `(volume_id, file_index)`.

## 4. Vector Store Architecture (`qdrant-edge`)
| Collection | Vector Dim (by Profile) | Distance | Payload Indexes | Purpose |
|---|---|---|---|---|
| `chunks` | `384` (Eco / Std default) or `1024` (Std opt-in / Pro) | Cosine | `file_id`, `project_id`, `sensitivity`, `state`, `ext`, `mtime` | Hybrid semantic search + semantic dedupe |
| `entities` | `384` or `1024` | Cosine | `entity_type` | Graph resolution |
| `projects` | `384` or `1024` | Cosine | `domain` | Project clustering |
| `routing_exemplars` | `384` or `1024` | Cosine | `decision`, `dest_folder` | Local taxonomy calibration |

- **HNSW:** `m=16`, `ef_construct=100`, query `ef=64`, scalar INT8 quantization.
- **Default Search Safety Filter (`B6` §2):** Every `search.query` automatically applies Qdrant filter `state != 'quarantine'` unless `include_quarantined=true` is explicitly requested, and enforces `sensitivity` scoping.
- **Outbox & Upgrade Policy:** Versioned collection swap (`chunks_v2_1024`) on profile upgrade; outbox batches ≤1,000 points/tx; `aegis doctor` repairs lag.

## 4.5 PostgreSQL Migration Path (V2+)
All SQL behind `store::db` trait; UUIDv7 TEXT PKs + epoch-ms BIGINTs + canonical JSON text; `chunks_fts` maps to PG `tsvector` + GIN; `aegis migrate --to pg` verifies row counts + `audit_log` head hash.

## 5. Acceptance Criteria, Open Questions, Assumptions & Traceability
- **Acceptance:** Migrations run `<200ms`; 10k rename + 1k hardlink fuzz test passes with `0` constraint errors and `0` broken `files.id`s; `chunks_fts` stays 100% synced over 100k ops.
- **Open:** `[OPEN]` `OQ-13` (Owner: Data Architect, Milestone 2): `routing_exemplars` 18-month expiry vs. exponential decay.
- **Assumptions:** `[ASSUMPTION]` `qdrant-edge` supports concurrent reads during single-writer batches ≤1,000 points.
- **Traceability:** Upstream `A2` (`FR-001..FR-013`), `B8` (`ADR-001`–`ADR-004`); Downstream `C1`, `C4`, `E4`, `F1/F2`.
--- END OF FILE B4_Data_Vector_Schema_v2.3.md ---
```

```markdown
--- START OF FILE B8_ADRs.md ---

# B8 — Architecture Decision Records (v2.2)

**ID:** B8 · **Version:** 2.2 · **Status:** Approved MVP Baseline
**Owner:** Principal Systems Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B4_Data_Vector_Schema_v2.3.md`, `C4_Policy_Engine_Confidence_Spec.md`

> **v2.2 changes:** Reordered `ADR-001 → ADR-002 → ADR-003 → ADR-004`; updated `quarantine_items` → `quarantine_entries`; added hardlink (`nlink > 1`) accounting to `ADR-003`; standardized `.aegisboundary` marker name in `ADR-004`; added `files.unit_split_override` traceability.

---

## ADR-001 — Rust-First Extraction; Python Deferred to V1.5+
- **Status:** Accepted (MVP)
- **Context:** PRD v2.0 proposed an always-on Python gRPC sidecar. Bundling CPython + ML wheels + Poppler + Tesseract yields 1.5–3GB installers, ~1GB extra RSS, AV false positives, and IPC serialization overhead across 1M+ files.
- **Decision:** MVP hot path runs 100% in a single Rust binary: `ort`/`fastembed`, `tree-sitter`, `calamine`/`quick-xml`/`kamadak-exif`, `pdfium-render`, `blake3`, `notify`, `jwalk`, and `qdrant-edge`. Only `llama.cpp` runs on-demand for `Standard`/`Pro` LLM features. Python (or WASM plugins) returns in V1.5+ as an optional lazy-downloaded runtime for Whisper, heavy OCR, `.pst`, and VLM captioning.
- **Consequences:** Single native binary, ~50% lower RSS, simpler CI; weaker MVP extraction on scanned PDFs (handled cleanly via `partial` coverage reporting until V1.5).

## ADR-002 — Hardware-Tiered Inference, Idle Eviction & 3-Tier Progressive Indexing
- **Status:** Accepted (MVP)
- **Context:** v2.0's BGE-M3 + NER + 4-bit 8B LLM stack exceeds the ≤4GB RSS scan cap, and 50 emb/sec × 5–10M chunks = 28–56h on CPU.
- **Decision:**
  1. Three profiles (`Eco` <12GB RAM, `Standard` 12–24GB, `Pro` ≥24GB or ≥8GB VRAM GPU).
  2. **LLM never resident during bulk scans:** loaded on-demand for interactive query/proposal bursts, hard-evicted after 300s idle; disabled on `Eco`.
  3. **3-Tier Progressive Indexing:** Tier 1 metadata (≤15m, ≤90s on 150k SSD) → Tier 2 active roots (≤40m) → Tier 3 background throttled (≤2.5h).
  4. **Selective Chunking:** ≤1,500-token docs = 1 chunk (~1.3 chunks/file avg across 1M files); INT8 ONNX + Qdrant scalar quantization everywhere.
- **Consequences:** Memory closes ≤2.36GB target (≤3.65GB conservative ceiling) during scan; 1M-file CPU scan closes ~3.5h < 6h SLA; dashboard delivers value in ≤2m.

## ADR-003 — Per-Volume Quarantine, OS Trash Delegation & Hardlink-Safe Accounting
- **Status:** Accepted (MVP)
- **Context:** A single `.aegis/quarantine` forces slow cross-volume copies and risks filling the boot drive. Separately, NTFS/APFS/ext4 hardlinks (`nlink > 1`) share storage blocks; counting both as duplicates overstates reclaimable space.
- **Decision:**
  1. Quarantine lives at `<VolumeRoot>/.aegis-quarantine/<batch_id>/` (hidden/system attr, auto-excluded from indexing) so deletes are `O(1)` atomic same-volume renames. OS Trash delegation allowed per user setting when same-volume semantics hold.
  2. If volume root is non-writable or unmounted: action fails cleanly (`failed:quarantine_unavailable`); user is offered an explicit journaled permanent-delete confirmation or copy-to-primary fallback *only* if `size < 25%` of destination free space — never silent permanent delete.
  3. Entries tracked in `B4.quarantine_entries` (`expires_at = +30d` TTL); expiry purge checks volume free-space guard.
  4. **Hardlinks (`nlink > 1`):** `B4.files` tracks `(volume_id, file_index, path_hash)` and `nlink`. Shared-inode hardlink groups report **0 reclaimable bytes** unless all-but-keeper links are removed.
- **Consequences:** Instant reversible deletes across multi-TB drives; 100% honest disk-reclaim reporting (`G6`).

## ADR-004 — Atomic Project Units (Indivisible Directories)
- **Status:** Accepted (MVP)
- **Context:** File-level semantic refiling can propose moving `README.md` or `data.csv` out of a Git repo or multi-file project, silently breaking builds.
- **Decision:** A directory is registered in `B4.atomic_units` if it contains any canonical boundary marker:
  `.git` · `package.json` · `pyproject.toml` · `Cargo.toml` · `go.mod` · `pom.xml` · `build.gradle` · `.xcodeproj` · `*.sln`/`*.csproj` · `.app`/`.photoslibrary` bundles · SQLite `-wal`/`-shm` companion triplets · LaTeX (`\documentclass` + `.bib`) · Premiere `.prproj` bundles · user-defined **`.aegisboundary`** marker file.
  - Proposals may move/archive the *entire* unit root, never pluck interior files outward.
  - Interior files remain fully indexed and searchable.
  - Per-file override (`files.unit_split_override = 1`) or unit override (`atomic_units.allow_split_override = 1`) must be explicitly user-confirmed and journaled. Innermost marker wins for nested units.
- **Consequences:** Eliminates destructive repo splitting with deterministic UI explainability.

## Metadata & Traceability
- **Acceptance:** Referenced by `A2`, `B4`, `C1`, `C4`; zero unresolved conflicts.
- **Open:** `OQ-12` (`pdfium` signing/notarization).
- **Traceability:** Upstream `A2` `G1–G6`, `P1–P7`; Downstream `B4 v2.3`, `C1 v1.1`, `C4 v1.0`, `F2`.
--- END OF FILE B8_ADRs.md ---
```

```markdown
--- START OF FILE C1_IPC_Action_Contracts.md ---

# C1 — IPC, Events & Action Contract (v1.1)

**ID:** C1 · **Version:** 1.1 · **Status:** Authoritative for MVP FE/BE Split
**Owner:** Staff Backend & Desktop Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B4_Data_Vector_Schema_v2.3.md`, `C4_Policy_Engine_Confidence_Spec.md`

> **v1.1 changes:** Added missing `quarantine.*`, `actions.execute`, `atomic_unit.*`, `volumes.list`, `audit.*`, and `findings.scan`; aligned `findings.action` verdict enum to `B4`; added `include_quarantined` default-false filter on `search.query`; fixed TS syntax and draft text in §2–§4.

---

## 1. Transport
- **Commands:** Tauri `invoke()` typed via `specta`/`ts-rs` Rust→TS codegen.
- **Events:** `tauri::event` versioned channels (`v: 1`).
- **Local API (V1+):** REST/GraphQL on `127.0.0.1:4777`, bearer token in OS keychain, loopback-only enforced.

## 2. Command Surface (~52 Commands)

```ts
// Scan, Volumes & Index
scan.start(roots?: string[], tiers?: (1|2|3)[]): JobId
scan.pause(job: JobId): void
scan.resume(job: JobId): void
scan.cancel(job: JobId): void
scan.status(job?: JobId): ScanStatus            // incl. coverage_pct, tier progress, skipped counts
volumes.list(): VolumeInfo[]
index.rebuild(root?: string): JobId
index.stats(): IndexStats                       // files, chunks, vectors, coverage

// Search (B6 §2: quarantined items excluded by default)
search.query(
  q: string,
  filters?: SearchFilters & { include_quarantined?: boolean },
  limit?: number
): SearchPage                                   // returns match_evidence + highlighted chunks
search.suggest(prefix: string): string[]

// Files, Atomic Units & Graph
file.get(id: FileId): FileDetail                // chunks preview, atomic_unit, graph neighborhood
project.list(): Project[]
project.get(id: ProjectId): ProjectDetail
atomic_unit.list(volumeId?: string): AtomicUnit[]
atomic_unit.override(
  target: { unitId: string } | { fileId: FileId },
  allowSplit: boolean
): void                                         // journaled in audit_log
graph.neighborhood(entityId: string, depth?: number): Subgraph
collection.list(): SmartQuery[]                 // V1.5
collection.create(def: SmartQuery): SmartQuery  // V1.5

// Duplicates
dedupe.report(groupBy?: 'exact'|'near'|'semantic'|'version'): DupeReport
dedupe.propose(keepers: Record<string, FileId>): BatchId

// Actions, Simulator & Quarantine
actions.propose(plan: RefilePlan): BatchId
actions.simulate(batch: BatchId): Simulation    // dry-run tree diff + policy verdicts (C4)
actions.approve(batch: BatchId, selection?: ActionId[]): void
actions.execute(batch: BatchId): ExecuteResult  // runs approved actions through policy gate
actions.reject(batch: BatchId, reason?: string): void
actions.rollback(batch: BatchId): RollbackResult
actions.resolve_conflict(actionId: ActionId, choice: 'restore_as_rename'|'skip'|'abort_batch'): RollbackResult
actions.list(status?: ActionStatus): Action[]
undo.last(n?: number): UndoResult

quarantine.list(volumeId?: string): QuarantineEntry[]
quarantine.restore(entryIds: string[]): BatchId
quarantine.purge(entryIds: string[], confirmedPermanent: boolean): BatchId

// Health, Findings & Playbooks
health.get(): HealthBreakdown                   // 5 dims + top 10 offending items per dim
playbook.disk_rescue(): DiskRescueReport        // FR-011.2(a)
findings.scan(path: string): JobId              // FR-011.2(b) Pre-Share Secret Audit
findings.list(filter?: FindingFilter): Finding[]
findings.action(
  id: FindingId,
  verdict: 'false_positive'|'resolved'|'accepted_risk'|'vaulted'
): void

// Rules, Audit & Settings
rules.list(): Rule[]
rules.create(def: RuleDef): Rule
rules.update(id: RuleId, def: RuleDef): Rule
rules.delete(id: RuleId): void
rules.graduate(actionId: ActionId): Rule        // FR-005.3 "Teach Aegis once"
audit.list(limit?: number, offset?: number): AuditEntry[]
audit.export(destPath: string): void

settings.get(scope?: string): SettingsMap
settings.set(key: string, value: unknown, scope?: string): void
onboarding.state(): OnboardingState
onboarding.complete(config: OnboardingConfig): void
models.status(): ModelStatus[]
models.download(tier: 'eco'|'standard'|'pro'): JobId
hw.profile(): 'eco'|'standard'|'pro'
doctor.run(): DoctorReport
```

## 3. `Action` Lifecycle State Machine (Synced to `B4 v2.3`)
- `action_batches.status`: `draft | approved | executing | executed | partial | rolled_back | failed`.
- `actions.status`: `proposed | approved | executed | failed | skipped | rolled_back | rejected | superseded | expired | unrecoverable_missing`.

```
proposed ──approve──▶ approved ──execute──▶ executed ──rollback──▶ rolled_back
   │                    │                      │
   ├──reject──▶ rejected┤                      ├──conflict──▶ rollback_conflict set;
   │                    └──expire──▶ expired   │   user resolves (restore_as_rename |
   │                                           │   skip | abort_batch)
   └──superseded──▶ superseded                 └──missing──▶ unrecoverable_missing
```

**Invariants:**
- `pre_blake3` + `pre_state_json` captured **before** mutation; `post_blake3` + `post_state_json` captured **after**.
- Partial unique index `idx_actions_file_status` enforces at most 1 live (`proposed | approved`) action per `file_id`.
- Deletes create `quarantine_entries` rows (`B4`); physical deletion never occurs outside `quarantine.purge(..., confirmedPermanent=true)` or TTL expiry.

## 4. Rollback Conflict Resolution (`FR-005.4`)
Before reversing an executed `actions` row, engine checks:
`exists(to_path) && blake3(to_path) == post_blake3 && !exists(from_path)` (and `to_path` metadata vs `post_state_json` for metadata ops).

| Condition | `rollback_conflict` | Resolution Options |
|---|---|---|
| All checks pass | `NULL` | Reverse op executed & journaled; `status = 'rolled_back'` |
| `to_path` modified after execution | `'modified'` | `restore_as_rename` (keep user's edited `to_path` and restore original snapshot to `<from_path>.restored-<ts>` if backup exists, or move edited copy to `<to_path>.edited-<ts>` before restoring `from_path`), `skip`, or `abort_batch` |
| `from_path` reoccupied | `'collision'` | `restore_as_rename` (restore original under `<from_path>.restored-<ts>`), `skip`, or `abort_batch` |
| `to_path` missing on disk | `'missing_target'` | `skip` (status → `unrecoverable_missing`, honest error — never falsely marked `rolled_back`) or `abort_batch` |

## 5. Events (`v: 1`) & Error Taxonomy
- **Events:** `ScanProgress`, `IndexUpdated`, `ActionProposed`, `FindingCreated`, `HealthUpdated`.
- **Error Taxonomy (`scan_jobs.errors_json` & IPC errors):**
  `permission_denied` · `cloud_placeholder` · `unsupported_fs` · `locked_file` · `path_too_long` · `parse_failed:<format>` · `watcher_lost` · `volume_offline` · `quarantine_unavailable` · `policy_blocked:<rule>`.
  Every IPC error returns `{ code: string, message: string, retryable: boolean, remediation?: string }`.

## 6. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** `specta`/`ts-rs` generates valid TypeScript bindings with 0 hand-maintained duplicates; all commands map 1:1 to `B4 v2.3` (`C1_B4_Alignment_Check.md`).
- **Assumptions:** Single local desktop user session per SQLite file lock.
- **Traceability:** Upstream `A2`, `B4`, `B6`, `C4`; Downstream Tauri Rust commands & React hooks.
--- END OF FILE C1_IPC_Action_Contracts.md ---
```

```markdown
--- START OF FILE C1_B4_Alignment_Check.md ---

# C1 ↔ B4 Alignment Verification (v1.1)

**Date:** 2026-11-20 · **Result:** FULLY ALIGNED (`C1 v1.1` ↔ `B4 v2.3`)

| C1 v1.1 Command Group | B4 v2.3 Table(s) & Columns | Status |
|---|---|---|
| `scan.*`, `volumes.list` | `scan_jobs`, `volumes` | ✓ Exact |
| `index.*` | `files`, `chunks`, `outbox_events`, `settings` | ✓ Exact |
| `search.query/suggest` | `chunks_fts`, `qdrant:chunks`, `files.state`, `files.sensitivity` | ✓ Exact |
| `file.get`, `project.*`, `graph.*` | `files`, `projects`, `entities`, `relationships`, `atomic_units` | ✓ Exact |
| `atomic_unit.list/override` | `atomic_units.allow_split_override`, `files.unit_split_override`, `audit_log` | ✓ Fixed in `B4 v2.3` + `C1 v1.1` |
| `dedupe.*` | `files` (`blake3`, `simhash_text`, `dhash_image`, `nlink`), `action_batches`, `actions` | ✓ Exact |
| `actions.*`, `undo.last` | `action_batches`, `actions` (`pre/post_blake3`, `pre/post_state_json`, `rollback_conflict`, `status`) | ✓ Fixed in `C1 v1.1` |
| `quarantine.list/restore/purge` | `quarantine_entries`, `volumes.quarantine_dir`, `actions`, `audit_log` | ✓ Fixed in `C1 v1.1` |
| `health.get`, `playbook.disk_rescue` | `files` aggregates, `security_findings`, `scan_jobs` | ✓ Exact |
| `findings.scan/list/action` | `security_findings` (`open\|false_positive\|accepted_risk\|vaulted\|resolved`), `security_finding_locations` | ✓ Fixed in `B4 v2.3` + `C1 v1.1` |
| `rules.*`, `rules.graduate` | `rules` (`source='graduated_proposal'`), `routing_exemplars` | ✓ Exact |
| `audit.list/export` | `audit_log` | ✓ Fixed in `C1 v1.1` |
| `settings.*`, `onboarding.*`, `models.*`, `hw.profile`, `doctor.run` | `settings`, `volumes`, `scan_jobs`, `audit_log`, `outbox_events`, `chunks_fts` | ✓ Exact |
| `collection.*` | Deferred to V1.5 (`settings` scope or future `collections` table) | ✓ Scoped |
--- END OF FILE C1_B4_Alignment_Check.md ---
```

```markdown
--- START OF FILE F1_F2_Eval_Safety_Plan.md ---

# F1/F2 — Evaluation, NFR Soak & Safety Test Plan (v2.2)

**ID:** F1/F2 · **Version:** 2.2 · **Status:** Required MVP Exit Gate
**Owner:** QA & ML Eval Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B4_Data_Vector_Schema_v2.3.md`, `B6_Threat_Model.md`, `C1_IPC_Action_Contracts.md`, `C4_Policy_Engine_Confidence_Spec.md`

> **v2.2 changes:** Aligned `F1` query corpus to `500` queries (`A2` G1); added **§F3 NFR Scale, Memory & Latency Soak Suite** (1M files / 5M chunks / ≤4GB RSS / <300ms p95); added `F2` tests for Pre-Rollback Conflict Reconciliation (`FR-005.4`) and Hardlink accounting (`ADR-003`).

---

## F1 — Functional Evaluation Corpus & Metrics Harness

### 1. Corpus Composition (~50k files, ~20GB, synthetic + OSS licensed)
| Slice | Count | Purpose |
|---|---|---|
| Synthetic office docs (PDF/DOCX/XLSX/PPTX/MD) | 15,000 | Extraction, chunking, classification, search |
| OSS Code repos (~30 projects) | 7,000 | `atomic_units`, tree-sitter, secret FP traps |
| Personal-style messy file tree | 10,000 | Refiling proposals, dedupe realism |
| Duplicate & hardlink injection | 8,000 | Exact/near/semantic/version-chain + `nlink > 1` math |
| Security corpus (secrets + PII + prompt-injection) | 4,000 | Recall, FP rate, `B6` injection canaries |
| EXIF-rich images | 5,000 | Perceptual dHash dedupe & EXIF entities |
| Cloud placeholders (stubbed attributes) | 500 | Hydration-guard verification |
| Adversarial paths | 500 | Unicode NFC/NFD, `CON`/`NUL`, >260 chars, ADS, junctions |

### 2. Metric Gates (Nightly & Pre-Release CI)
| Metric (`A2` §2) | Method | MVP Target |
|---|---|---|
| Search Hit@3 | **500 hand-labelled NL queries** (exact, semantic, temporal, entity, negative, filtered) | **≥75%** |
| Dedupe recall & reclaim honesty | Injected exact/near dupes + 500 hardlinked pairs (`nlink>1`) | **≥85% recall**; exact `0`-byte reclaim on hardlink pairs |
| Proposal acceptance | 5-human pilot + simulated user over messy slice | **≥60%** |
| Secret recall & FP rate | 500 planted tokens/keys + 3,500 FP traps | **≥95% recall**, **≤5% critical FP** |
| Coverage honesty | `reported_coverage` vs ground-truth enumerable count | **100%** |
| Rollback success | All executed batches reverted in clean + conflict VMs | **100%** |

---

## F2 — Safety, Policy & Fuzz Test Plan

### 1. Mandatory Mutation & Security Invariants
1. **No unjournaled mutation:** Execute action suite in Linux VM under `fanotify`/`strace`, macOS under `fs_usage`, Windows under ProcMon filter; assert 100% of FS writes match `actions` + `audit_log` entries.
2. **Never overwrite silently:** Fuzz destination collisions (file exists, dir exists, NTFS/APFS case-only diffs) → assert suffix-rename or abort.
3. **Atomic Units (`ADR-004`):** Fuzz Curator/rules proposals against repos & `.aegisboundary` fixtures → assert `0` interior mutations without explicit `unit_split_override` / `allow_split_override`.
4. **Per-volume quarantine (`ADR-003`):** Test multi-volume deletes → assert `O(1)` same-volume rename; test read-only/unmounted volume → assert `failed:quarantine_unavailable` + explicit prompt.
5. **Pre-Rollback Conflict Reconciliation (`FR-005.4`):** Test (a) externally modified `to_path`, (b) reoccupied `from_path`, (c) deleted `to_path` → assert pause + 3-way resolution dialog, never silent clobber or false `rolled_back` status.
6. **Crash recovery:** `kill -9` at every stage (scan, outbox sync, batch execution, rollback) → restart + `aegis doctor` leaves DB consistent.
7. **Cloud-placeholder guard:** Verify `0` payload reads (`read()` count == 0) on `SF_DATALESS` / `RECALL_ON_DATA_ACCESS` files.
8. **Prompt-injection & zero-egress (`B6` §3–§4):** Run injection corpus + offline scan/search under packet capture → `0` injection-triggered actions, `0` non-localhost packets, `0` raw secret strings in DB/logs.

---

## F3 — NFR Scale, Memory & Latency Soak Suite (Resolves `suite_reconciliation` I4)

Executed on dedicated `Standard` reference hardware (16GB RAM, 8-core CPU, NVMe SSD, **no discrete GPU**):

| Benchmark | Dataset | Pass Gate | Purpose |
|---|---|---|---|
| **1M-File Scan Throughput** | 1M synthetic/OSS files (~1.3M selective chunks) | Tier 1 **≤15m**; Total 3-Tier **≤6h** | Proves `A2` §4.2 throughput math |
| **5M-Chunk Memory Soak** | 5M chunks loaded in SQLite + `qdrant-edge` | Scan peak RSS **≤3.65GB** (target ≤2.5GB); Query burst **≤4.1GB** | Resolves `2.36GB vs 3.65GB` CI gate |
| **Hybrid Search Latency** | 500 queries over 5M chunks (`FTS5` + `qdrant-edge`) | **p95 <300ms** (excl. LLM parse) | Proves `FR-007` NFR |
| **Rename & Hardlink Stress** | 10k renames + 1k hardlinks across live watcher | `0` constraint errors; `100%` preserved `files.id` | Proves `B4 v2.3` `idx_files_os_path` |

## Metadata & Traceability
- **Acceptance:** All F1/F2/F3 gates green in release CI pipeline prior to MVP v1.0 tag.
- **Open:** `[OPEN]` `OQ-14` (Owner: QA Lead, Sprint 1): Provision dedicated 16GB Windows + macOS CI runners for F3 soak tests.
- **Assumptions:** Synthetic 1M-file tree matches empirical file-size/token distribution (~1.3 chunks/file avg).
- **Traceability:** Upstream `A2` `G1–G6`, `B4 v2.3`, `B6 v2.3`, `C1 v1.1`, `C4`; Downstream CI release gate.
--- END OF FILE F1_F2_Eval_Safety_Plan.md ---
```

```markdown
--- START OF FILE B6_Threat_Model.md ---

# B6 — Security Threat Model (v2.3)

**ID:** B6 · **Version:** 2.3 · **Status:** Baseline for MVP Security Review
**Owner:** Security Engineering Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B4_Data_Vector_Schema_v2.3.md`, `C1_IPC_Action_Contracts.md` (v1.1), `C4_Policy_Engine_Confidence_Spec.md`, `F1_F2_Eval_Safety_Plan.md`

> **v2.3 changes:** Added Model Artifact & GGUF/ONNX parser supply-chain row to STRIDE (§2); added `<VolumeRoot>/.aegis-quarantine/` to default indexing exclusions (§6.4); wired default quarantine/secret search exclusion to `C1 v1.1` `search.query`; added standard metadata/traceability sections.

---

## 1. Scope & Trust Boundaries
Protects user files, metadata, `aegis.db`, `qdrant-edge` vectors, `audit_log`, quarantine, models, API bearer token, keychain entries, and zero-egress privacy.
- **TB-1:** Filesystem (untrusted content) · **TB-2:** Rust/SQLite/Qdrant core · **TB-3:** LLM/Agent boundary · **TB-4:** Loopback API · **TB-5:** Opt-in cloud egress · **TB-6:** Plugin WASM runtime (V2).

## 2. STRIDE Analysis
| Threat | Component | Vector | Mitigation | Residual |
|---|---|---|---|---|
| **Spoofing** | Local API (TB-4) | Local process calls API | Loopback bind (`127.0.0.1`), rotating per-install bearer token in OS keychain | Low |
| **Tampering** | `aegis.db`, journal | Malware/corruption | Hash-chained `audit_log`, `pre/post_state_json`, `aegis doctor`, WAL checksums, signed releases | Medium (tamper-evident; V2 signed checkpoints) |
| **Tampering** | Filesystem ops | Automation bug | Journaled `action_batches` only, `C4` policy gate, dry-run, per-volume quarantine | Low |
| **Tampering / RCE** | **Model Artifacts & Parsers (TB-2/TB-3)** | **Tampered ONNX/GGUF download or malformed tensor/metadata exploit** | **HTTPS-only downloads, pinned SHA-256 checksums against signed release manifest, isolated `llama.cpp` subprocess with memory cap (`ADR-002`)** | **Low–Medium** |
| **Repudiation** | Actions/audit | Denied mutation | Hash-chained `audit_log` + `origin` + `actor` + exportable log | Low |
| **Info Disclosure** | Index at rest | Device theft | Optional SQLCipher (V1.5); default relies on OS FDE — documented honestly | Medium until V1.5 |
| **Info Disclosure** | Secrets stored | Aegis leaks secrets | Zero-storage: `HMAC-BLAKE3` fingerprint + masked snippet (`AKIA...9X2Q`) only | Low |
| **Info Disclosure** | Search results | Leaking quarantined/secret files | `C1` `search.query` enforces `state != 'quarantine'` by default; `sensitivity` filter enforced | Low |
| **DoS** | Scanner/Extractors | Zip bombs, FIFO/device files, deep trees | Bounded pool, archive depth ≤2 / ≤50MB caps, device-file skip, token-bucket throttle | Low |
| **Elevation** | Plugins (V2) | Malicious plugin | WASM sandbox, manifest permissions, proposals-only, signed marketplace | V2 |

## 3. Prompt-Injection Surface (TB-1 → TB-3)
File contents are untrusted data: (1) delimited data blocks in prompts, (2) agents emit `proposed` actions only — never direct FS/shell execution, (3) schema-validated outputs, (4) canonicalized path checks against roots & `atomic_units`, (5) `F1` injection canaries, (6) `Housekeeper` ceiling = propose-only.

## 4. Cloud-Egress Paths (TB-5)
All content/index egress **OFF by default**: opt-in Cloud LLM passes secret/PII redaction filter + logs token count; model downloads check pinned SHA-256 over HTTPS; opt-in telemetry sends counts/timings only. Verified offline in `F2`.

## 5. Threat Model Boundaries
Out of scope: attacker with live authenticated user-session privileges or compromised OS kernel/drivers.

## 6. Secrets-Handling Invariants
1. Raw secrets never stored in SQLite, logs, vectors, or crash dumps.
2. `security_findings` uses `HMAC-BLAKE3(keychain_salt, secret)` + masked snippet.
3. `.aegis` internal dir **and** all `<VolumeRoot>/.aegis-quarantine/` directories are excluded from indexing via immutable built-in ignore rules.

## 7. Audit Threat Model
`audit_log` is **tamper-evident** locally (`aegis doctor` verifies `prev_hash`/`hash` chain); V2 adds Ed25519-signed chain checkpoints. UI/docs must never claim "tamper-proof."

## 8. Security Release Gates & Traceability
- **Gates:** `F2` injection suite (0 actions), offline egress test (0 external packets), secrets grep test (0 raw secrets), `aegis doctor` detects 100% seeded chain tamper, loopback API rejects remote bind/bad token.
- **Open:** `[OPEN]` `OQ-15` (Owner: Security Lead, V1.5): Decide threat posture for `llama.cpp` subprocess OS sandboxing (AppContainer / Seatbelt / seccomp).
- **Assumptions:** Host OS full-disk encryption (BitLocker/FileVault/LUKS) is enabled by user prior to V1.5 SQLCipher shipping.
- **Traceability:** Upstream `A2` `G4–G5`, `B8`; Downstream `B4 v2.3`, `C1 v1.1`, `C4 v1.0`, `F2`.
--- END OF FILE B6_Threat_Model.md ---
```

```markdown
--- START OF FILE A2_PRD_v2.1_patch_notes.md ---

# A2 — PRD v2.1.1 Delta Alignment Patch

Apply these 4 surgical line updates to `A2_PRD_v2.1.md` so it stays 100% synchronized with `B4 v2.3`, `B8 v2.2`, `C1 v1.1`, and `B6 v2.3`:

1. **Header Related Docs:** Update `B4_Data_Vector_Schema_v2.2.md` → `B4_Data_Vector_Schema_v2.3.md`; add `C4_Policy_Engine_Confidence_Spec.md`, `D3_Critical_UX_Flows_v1.0.md`, `E2_E4_Delivery_Observability_Spec.md`, `G2_Glossary.md`.
2. **§3.1 Note on `FR-001.7`:** Add footnote under §3.1 table: `*FR-001.7 (Email-export ingestion: mbox/eml/pst) is deferred to P2 / V2 per §7 Roadmap.*`
3. **§3.3 `FR-003.2` Marker Alignment:** Replace inline marker list with:
   `"Dirs containing canonical markers per B8 ADR-004 (.git, package.json, Cargo.toml, .app, .aegisboundary, etc.) → atomic_units record; contained files link via atomic_unit_id."`
4. **§3.6 `FR-007` Default Security Filter:** Append to `FR-007` Acceptance Criteria:
   `"Files with state='quarantine' are excluded by default unless include_quarantined=true is explicitly passed (B6 §2)."`
5. **§3.7 Cross-Reference:** Change `per §Roadmap` → `per §7 Roadmap`.
--- END OF FILE A2_PRD_v2.1_patch_notes.md ---
```

---

## Part B: Missing P0/P1 Execution Documents

```markdown
--- START OF FILE C4_Policy_Engine_Confidence_Spec.md ---

# C4 — Policy Engine, Confidence Routing & Autonomy Specification (v1.0)

**ID:** C4 · **Version:** 1.0 · **Status:** Approved MVP Baseline
**Owner:** Staff Automation & Safety Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (`FR-005`), `B4_Data_Vector_Schema_v2.3.md`, `B8_ADRs.md` (`ADR-003`, `ADR-004`), `C1_IPC_Action_Contracts.md` (`v1.1`)

---

## 1. Purpose & Scope
Defines the deterministic evaluation rules governing how `ActionProposals` (from Agents, Rules, Dedupe, or User) pass through the **Policy Engine** before entering `approved` or `executed` states. Eliminates ambiguity between `autonomy_dial`, confidence thresholds, agent ceilings, and `atomic_units` guards.

**Audience:** Rust Automation Engine engineers, Agent Orchestrator engineers, Frontend Simulator devs, QA.

## 2. Autonomy Dial & Action Classes
Autonomy is configured **per action class** in `settings` (`scope = 'global' | 'root:<id>' | 'project:<id>'`), never as a single global override:

| Action Class | MVP (`v1.0`) Max Allowed Dial | V2+ Max Allowed Dial |
|---|---|---|
| `refile` (`move` / `rename` via Curator/Rules) | `Suggest` (Manual approval in Simulator/Inbox) | `Auto` (if `confidence ≥ 0.90` & user-enabled) |
| `archive` (`Warm → Cold` lifecycle) | `Manual` (tags only in MVP) | `Auto-approved` (V2) |
| `quarantine` (Dedupe / Housekeeper) | **`Suggest` (Never Auto)** | **`Suggest` (Never Auto)** |
| `purge` (Permanent delete) | **`Manual` (Explicit confirm only)** | **`Manual` (Explicit confirm only)** |
| `security_remediate` (`move_to_vault` / `encrypt`) | `Suggest` | `Suggest` |

> **MVP Invariant (`A2` §7):** In `v1.0`, **zero** agent-generated proposals auto-execute without explicit user approval in Simulator or Approvals Inbox. Graduated deterministic rules (`source = 'graduated_proposal'`) may pre-stage approved drafts only if user explicitly sets that rule's action to `Auto-approved` in V1.5+.

## 3. Precedence & Deterministic Decision Pipeline
Every `actions` row is evaluated in strict order (`1 → 6`). **First failing hard gate blocks execution** (`policy_blocked:<reason>`):

1. **Root Boundary Check:** Canonicalized `from_path` and `to_path` (if non-null) must reside inside active, `online` configured roots in `volumes` (`..` traversal and escaping symlinks rejected).
2. **Quarantine / Internal Exclusion Check:** Neither `from_path` nor `to_path` may target `.aegis` internals (except `quarantine`/`restore` ops targeting `<VolumeRoot>/.aegis-quarantine`).
3. **Atomic Unit Guard (`ADR-004`):** If `files.atomic_unit_id IS NOT NULL` AND `to_path` crosses outside `atomic_units.root_path`, **BLOCK** unless `atomic_units.allow_split_override == 1 OR files.unit_split_override == 1`.
4. **Agent Ceiling & Autonomy Check:** Compare `actions.agent` against `settings['autonomy.<action_class>']`. Even in V2 `Auto` mode, `Housekeeper` and `Security` agents remain capped at `Suggest`.
5. **Confidence Routing Band:**
   - `confidence ≥ 0.90`: **Auto-Eligible** (routed to "High-Confidence Ready" group in Simulator; eligible for V2 `Auto`).
   - `0.60 ≤ confidence < 0.90`: **Review Queue** (highlighted amber in Simulator with evidence required).
   - `confidence < 0.60`: **Hold / Flag Only** (never proposed as an automatic move; surfaced only as "Uncertain — Manual Sort" suggestion).
   - Deterministic `rules` (`agent IS NULL`) carry `confidence = 1.0`.
6. **Rule Conflict & Priority Resolution:** When multiple `rules` match a file:
   - Lower `rules.priority` integer wins (e.g., `10` overrides `100`).
   - Ties broken by specificity: `project:<id>` > `root:<id>` > `global`.
   - Remaining ties mark `status = 'proposed'` with `conflict_note` for manual pick (never nondeterministic).

## 4. Reference Evaluation Pseudocode
```rust
fn evaluate_policy(action: &Action, ctx: &PolicyContext) -> PolicyVerdict {
    if !ctx.within_active_roots(&action.from_path, action.to_path.as_deref()) {
        return PolicyVerdict::Block("outside_configured_roots");
    }
    if ctx.is_protected_internal(&action.from_path) {
        return PolicyVerdict::Block("protected_aegis_internal");
    }
    if ctx.crosses_atomic_unit(action) && !ctx.split_override_enabled(action.file_id) {
        return PolicyVerdict::Block("atomic_unit_boundary");
    }
    if action.action_type == ActionType::Quarantine && ctx.dial_for(action) == Autonomy::Auto {
        return PolicyVerdict::RequireApproval("quarantine_never_auto");
    }
    if action.confidence < 0.60 {
        return PolicyVerdict::HoldLowConfidence;
    }
    if ctx.is_mvp_mode() || ctx.dial_for(action) != Autonomy::Auto || action.confidence < 0.90 {
        return PolicyVerdict::RequireApproval("manual_or_review_gate");
    }
    PolicyVerdict::AutoEligible
}
```

## 5. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** Unit & property tests cover 100% of truth-table combinations (`dial × confidence × atomic_unit × agent_ceiling`).
- **Open:** None for MVP.
- **Assumptions:** `rules.priority` defaults to `100`; built-in safety rules use priority `1–20`.
- **Traceability:** Upstream `A2` `FR-005.1`–`FR-005.3`, `B8` `ADR-004`, `B6` §3; Downstream `C1` `actions.simulate/execute`, `D3` Simulator UX.
--- END OF FILE C4_Policy_Engine_Confidence_Spec.md ---
```

```markdown
--- START OF FILE D3_Critical_UX_Flows_v1.0.md ---

# D3 — Critical UX Flows & Accessibility Baseline (v1.0)

**ID:** D3 · **Version:** 1.0 · **Status:** MVP Design Baseline
**Owner:** Staff Product Designer · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `C1_IPC_Action_Contracts.md` (`v1.1`), `C4_Policy_Engine_Confidence_Spec.md`

---

## 1. Purpose & Scope
Specifies the interaction flows, edge-case states, honest-status copy, and WCAG 2.2 AA accessibility baseline (`FR-022`) for the 4 highest-risk MVP UX surfaces. Excludes visual brand tokens (color hexes/typography scale deferred to `D1` Design System).

**Audience:** Frontend/React engineers, Product Designers, QA.

## 2. Flow 1 — Onboarding, Hardware Profile & macOS TCC Recovery (`FR-011.1`)
```
[Welcome] ──▶ [Select Roots + Ignore Templates (node_modules, AppData)]
                    │
         (macOS only: Check Full Disk Access / Folder TCC)
           ├── Granted ──▶ [Detect Hardware Profile: Eco / Standard / Pro]
           └── Denied  ──▶ [Guided FDA Modal + "Continue with Limited Roots"]
                                │ (Logs skipped roots honestly in Coverage Report)
                                ▼
                    [Start Tier-1 Scan (≤2 min) → Unlock Health Dashboard]
```
- **Honest Coverage Rule:** Never show a "100% Complete" badge if `files_skipped_perm > 0` or `files_skipped_cloud > 0`. Display `"Indexed 94.2% (8,120 permission-skipped · 1,400 iCloud placeholders)"` with a clickable link to Remediation.

## 3. Flow 2 — Reorganisation Simulator & "Teach Aegis Once" (`FR-005.1`, `FR-005.3`)
1. **Diff View:** Left pane shows current folder tree; right pane shows proposed tree. Every row displays: `confidence` pill (`≥0.90` Green, `0.60–0.89` Amber, `<0.60` Grey-Locked), `"Why?"` popover (matched rule/project + top 3 evidence signals), and Atomic Unit lock icon if protected (`ADR-004`).
2. **Collision Preview:** Colliding destinations explicitly display `"Auto-rename → report (1).pdf"` (never overwrite).
3. **Approve & Graduate (`FR-005.3`):** Clicking **Approve** on a proposal triggers a non-blocking toast/inline option: *"Always apply this pattern? [Create Rule]"* → opens pre-filled `Rule` preview modal showing exact `condition_json` + matched preview count before saving via `rules.create` / `rules.graduate`.

## 4. Flow 3 — Pre-Rollback 3-Way Conflict Dialog (`FR-005.4`, `C1 §4`)
Triggered when `actions.rollback(batch)` returns one or more items with `rollback_conflict != NULL`:
- **Modal Header:** `"Cannot cleanly revert N of M files — external changes detected"`
- **Per-Conflict Row:** Shows `from_path`, `to_path`, conflict type (`Modified after Aegis moved it` | `Original location occupied` | `Moved file missing`), and 3 explicit radio actions (`Restore as renamed copy` | `Skip this file` | `Abort entire rollback`).
- **Safety Default:** Primary destructive-avoidance default = `Restore as renamed copy` for `modified`/`collision`; `Skip` (marks `unrecoverable_missing`) for `missing_target`.

## 5. Flow 4 — Crisis Playbooks (`FR-011.2`)
- **Disk Rescue:** Accessed from Dashboard card `"Reclaimable Storage"`. Groups into 3 expandable tabs: *Exact Duplicates (hardlink-safe)*, *Build Caches (`node_modules`, `target/`)*, *Stale Oversized Files (>500MB, >180d untouched)*. Primary CTA: `"Move Selected to Volume Quarantine"` (never permanent delete).
- **Pre-Share Secret Audit:** Folder picker → runs `findings.scan(path)` → renders deduplicated secrets sorted by `context_risk_score` with masked snippets (`AKIA...9X2Q`) and 1-click verdicts (`false_positive`, `accepted_risk`, `resolved`).

## 6. Accessibility & Keyboard Baseline (WCAG 2.2 AA, `FR-022`)
- Global Search hotkey (`Cmd/Ctrl + Shift + Space` or configurable) + `Cmd/Ctrl + K` command palette.
- Full keyboard navigation (`Tab`, `↑/↓`, `Enter` open, `Space` select, `Escape` close modal) across Search, Simulator Diff, and Approvals Inbox.
- All status badges pair color with icon + text label (never color-only); `aria-live="polite"` regions announce scan tier completion and rollback results.

## 7. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** Interactive Storybook/Playwright tests verify keyboard-only completion of Flows 1–4 and zero "100%" fake badges when skipped counts > 0.
- **Open:** `[OPEN]` `OQ-16` (Owner: Design Lead, Sprint 1): Finalize `D1` design token palette and light/dark contrast verification.
- **Traceability:** Upstream `A2` `FR-005`, `FR-011`, `FR-022`, `C1 v1.1`, `C4`; Downstream React/Tailwind components.
--- END OF FILE D3_Critical_UX_Flows_v1.0.md ---
```

```markdown
--- START OF FILE E2_E4_Delivery_Observability_Spec.md ---

# E2/E4 — Delivery, Packaging & Observability Specification (v1.0)

**ID:** E2/E4 · **Version:** 1.0 · **Status:** MVP Build Baseline
**Owner:** Release & Platform Engineering Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md`, `B4_Data_Vector_Schema_v2.3.md`, `B6_Threat_Model.md`, `B8_ADRs.md` (`ADR-001`, `ADR-002`)

---

## 1. Purpose & Scope
Defines the Tauri build/bundling pipeline for native Rust dependencies (`pdfium`, `ort`, `llama.cpp`), release signing & model verification, structured JSON logging (with zero-secret redaction), and the full `aegis doctor` integrity check matrix.

**Audience:** Release/DevOps engineers, Rust core engineers, Support/QA.

## 2. Native Bundling, Signing & Model Distribution (`E2`)
| Artifact | Packaging Strategy | Verification & Security |
|---|---|---|
| **Tauri Binary + Rust Core** | Static link `sqlite` (bundled), `tree-sitter`, `qdrant-edge`; bundle `pdfium` shared lib + `ort` dynamic provider (`DirectML` Win / `CoreML` macOS / CPU Linux) inside app resources. | Windows EV/Trusted Signing (SmartScreen); macOS Developer ID + Hardened Runtime + Notarization (`xcrun notarytool`); Linux AppImage + `.deb`. |
| **`llama.cpp` Worker** | Bundled as separate signed helper binary (`aegis-llm-worker`) spawned on-demand by Rust core (`ADR-002`) with OS memory/CPU caps. | Isolates GGUF parser crash/memory risk (`B6` §2); killed cleanly on 300s idle or app exit. |
| **ONNX & GGUF Models** | Downloaded post-install per hardware profile (`Eco`/`Standard`/`Pro`) into `<app_data>/models/`. | HTTPS-only against Ed25519-signed `models-manifest.json`; verify exact `SHA-256` before loading into `ort` or `llama.cpp`. |
| **Auto-Updater** | Tauri v2 updater with `stable` and `beta` channels + Ed25519 signature verification. | Staged rollout (5% → 25% → 100%); previous installer cached for 1-click downgrade. |

## 3. Observability & Structured Logging (`E4`)
- **Log Location:** `<app_data>/logs/aegis.jsonl` (daily rotation, max 7 files × 20MB).
- **Schema:** `{"ts":"2026-11-20T13:45:00Z","level":"INFO","component":"scanner","job_id":"...","event":"tier1_complete","duration_ms":4210,"files_seen":142000}`.
- **Zero-Secret & Privacy Redaction Invariant (`B6` §6):** Logger middleware **must never** emit raw file payloads or matched secret substrings; paths are omitted from any opt-in diagnostic bundle unless user checks *"Include sanitized local paths"*.
- **Local Analytics Metrics (SQLite/JSONL, always on-device):** Scan tier durations, RSS high-water mark, ONNX chunks/sec, search p50/p95 latency, outbox queue depth, approval/rollback counts.

## 4. `aegis doctor` Diagnostic & Repair Matrix (`FR-014` / `B4` / `B6`)
Running `doctor.run()` (UI Settings or CLI) executes 7 deterministic checks:

| # | Check | Verification Logic | Auto-Repair (`--fix` / UI button) |
|---|---|---|---|
| 1 | **Audit Chain Integrity** | Recompute `BLAKE3` over `audit_log` rows `1..N`; verify `prev_hash` links | **No auto-rewrite** (read-only alert + pinpoint first corrupt `seq`) |
| 2 | **SQLite Integrity** | `PRAGMA quick_check` + foreign-key check (`PRAGMA foreign_key_check`) | Vacuum / WAL checkpoint; flag manual restore if corrupt |
| 3 | **FTS5 Sync Drift** | Compare `COUNT(chunks)` vs `COUNT(chunks_fts)` | Rebuild FTS5 index (`INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')`) |
| 4 | **Outbox / Qdrant Lag** | Count `outbox_events WHERE processed_at IS NULL` + compare Qdrant point count | Replay pending `outbox_events` batches; prune orphan Qdrant points |
| 5 | **Quarantine Consistency** | Verify `quarantine_entries` active rows exist at `quarantine_path` on online volumes | Mark manually deleted external quarantine files as `purged_externally` |
| 6 | **Model Checksums** | Verify `SHA-256` of downloaded `.onnx`/`.gguf` files against manifest | Quarantine/delete corrupt model file + prompt re-download |
| 7 | **Watcher Health** | Check active `notify` / FSEvents / USN handles per online root | Restart dead watchers + trigger delta reconciliation sweep |

## 5. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** CI produces signed/notarized test builds on Win/macOS/Linux; `aegis doctor` catches 100% of seeded FTS drift, outbox lag, and tampered audit rows in `F2`.
- **Open:** `[OPEN]` `OQ-12` (Owner: Platform Lead, Sprint 0): Confirm Apple Developer ID + `pdfium` dylib `@rpath` notarization test pass.
- **Traceability:** Upstream `A2` §4–5, `B4 v2.3`, `B6 v2.3`, `B8` `ADR-001`–`002`; Downstream CI/CD workflows & `doctor.run()`.
--- END OF FILE E2_E4_Delivery_Observability_Spec.md ---
```

```markdown
--- START OF FILE G2_Glossary.md ---

# G2 — Canonical Glossary (v1.0)

**ID:** G2 · **Version:** 1.0 · **Status:** Active Reference
**Owner:** Principal Documentation Architect · **Last Updated:** 2026-11-20

| Term | Canonical Definition |
|---|---|
| **Action Batch** | One atomic Simulator plan or undo unit (`action_batches` row) grouping 1+ `actions`. |
| **Action Proposal** | Evidence-backed, previewable request (`actions` row) to mutate filesystem/index state — the sole permitted mutation path. |
| **Atomic Project Unit** | An indivisible directory (`atomic_units` row) marked by `.git`, package manifests, `.app`, or `.aegisboundary` whose interior files cannot be individually refiled outward without explicit override (`ADR-004`). |
| **Autonomy Dial** | Per-action-class setting (`Manual | Suggest | Auto-approved | Auto`) controlling execution gates (`C4`). |
| **Cloud Placeholder** | Remote-only synced file (`SF_DATALESS` / `RECALL_ON_DATA_ACCESS`) recorded without hydrating/downloading payloads (`FR-001.9`). |
| **Coverage Honesty** | Guarantee (`G6`) that skipped, permission-denied, locked, and placeholder files are counted and surfaced explicitly rather than faked as 100%. |
| **Digital Twin** | The queryable semantic model (`files` + `chunks` + `entities` + `relationships` + vector shards) of the user's computing environment. |
| **Graduated Rule** | Deterministic rule (`rules.source = 'graduated_proposal'`) created from a user-approved Action Proposal via "Teach Aegis once" (`FR-005.3`). |
| **Hardlink Alias** | Multiple directory entries sharing the same `(volume_id, file_index)` with `nlink > 1`; reports `0` reclaimable bytes unless all-but-keeper links are removed (`ADR-003`). |
| **Journal / Audit Log** | Append-only, hash-chained `audit_log` enabling tamper-evident verification and rollback traceability. |
| **Per-Volume Quarantine** | Hidden `<VolumeRoot>/.aegis-quarantine/` directory enabling `O(1)` atomic reversible deletes with 30-day TTL (`ADR-003`). |
| **Pre-Rollback Reconciliation** | Pre-undo verification of `to_path` hash/state and `from_path` availability (`FR-005.4`, `C1` §4). |
| **Tiered Progressive Indexing** | 3-stage scan (`Tier 1` metadata → `Tier 2` active roots → `Tier 3` cold background) delivering Day-1 value in ≤2m (`ADR-002`). |
--- END OF FILE G2_Glossary.md ---
```

```markdown
--- START OF FILE suite_reconciliation.md ---

# Suite Reconciliation & Living Index (v1.1)

**Date:** 2026-11-20 · **Status:** Synchronized MVP Suite (1 External File Pending Verification)

## 1. What Changed in v1.1 Reconciliation
Resolved 6 cross-document bugs uncovered during final audit:
1. `B4 v2.3`: Fixed `idx_files_os_path` hardlink bug (`UNIQUE(volume_id, file_index, path_hash)` + `idx_files_inode` + rename-vs-hardlink resolution); added `files.unit_split_override`, `idx_files_virtual`, `UNIQUE(file_id, seq)`, and `accepted_risk` status.
2. `B8 v2.2`: Reordered `ADR-001..004`, fixed `quarantine_entries`, restored hardlink rule in `ADR-003`, standardized `.aegisboundary` in `ADR-004`.
3. `C1 v1.1` & `C1_B4_Alignment_Check v1.1`: Added `quarantine.*`, `actions.execute`, `atomic_unit.*`, `volumes.list`, `audit.*`, `findings.scan`, `include_quarantined` search filter, aligned verdict enums, and fixed TS/draft text.
4. `F1/F2 v2.2`: Aligned `500`-query eval count to `A2`, added `F3` 1M-file/5M-chunk memory & latency soak suite, added `FR-005.4` conflict & hardlink tests.
5. `B6 v2.3`: Added model/GGUF supply-chain STRIDE row, `.aegis-quarantine` exclusions, and `search.query` default filter link.
6. Added missing P0/P1/P2 execution docs: **`C4`** (Policy Engine), **`D3`** (Critical UX Flows), **`E2/E4`** (Packaging & Observability), **`G2`** (Glossary), and **`A2 v2.1.1`** delta patch.

## 2. Unified Living Index (`G1`)
| ID | Document | Version | Status | File |
|---|---|---|---|---|
| **A2** | Product Requirements Document | `2.1` + `2.1.1` patch | ✅ Canonical | `A2_PRD_v2.1.md` + `A2_PRD_v2.1_patch_notes.md` |
| **B1/B2** | System & Sidecar Architecture | `1.0` | ⚠️ **`[OPEN]` Not yet verified in this session** | `B1_B2_Architecture.md` *(paste to audit vs `B8`/`B4`)* |
| **B4** | Data & Vector Specification | **`2.3`** | ✅ Canonical | `B4_Data_Vector_Schema_v2.3.md` |
| **B6** | Security Threat Model | **`2.3`** | ✅ Canonical | `B6_Threat_Model.md` |
| **B8** | Architecture Decision Records (`001–004`) | **`2.2`** | ✅ Canonical | `B8_ADRs.md` |
| **C1** | IPC, Events & Action Contracts | **`1.1`** | ✅ Canonical + Verified | `C1_IPC_Action_Contracts.md` + `C1_B4_Alignment_Check.md` |
| **C4** | Policy Engine & Confidence Spec | **`1.0`** | ✅ Canonical | `C4_Policy_Engine_Confidence_Spec.md` |
| **D3** | Critical UX Flows & A11y Baseline | **`1.0`** | ✅ Canonical | `D3_Critical_UX_Flows_v1.0.md` |
| **E2/E4** | Delivery, Packaging & Observability | **`1.0`** | ✅ Canonical | `E2_E4_Delivery_Observability_Spec.md` |
| **F1/F2** | Eval, NFR Soak & Safety Test Plan | **`2.2`** | ✅ Canonical | `F1_F2_Eval_Safety_Plan.md` |
| **G2** | Canonical Glossary | **`1.0`** | ✅ Canonical | `G2_Glossary.md` |

## 3. Next Action
1. **Paste `B1_B2_Architecture.md`** (and `solution_hierarchy.md` if needed) for a final alignment pass against `B4 v2.3`, `B8 v2.2`, and `C4 v1.0` — or begin **Sprint 0 scaffolding** directly from `B4 v2.3` DDL + `C1 v1.1` `specta` bindings.
--- END OF FILE suite_reconciliation.md ---
```