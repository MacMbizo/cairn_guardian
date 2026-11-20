# A2 — Aegis Guardian Product Requirements Document (v2.1.2)

**ID:** A2 · **Version:** 2.1.2 · **Status:** Approved MVP Baseline
**Owner:** Head of Product / Principal PM · **Last Updated:** 2026-11-20
**Supersedes:** `Aegis Guardian PRD v2.md` (v2.0)
**Related Docs:** `B8_ADRs.md` (v2.3, ADR-001–004), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `B1_B2_Architecture.md` (v2.3), `C1_IPC_Action_Contracts.md` (v1.2), `C4_Policy_Engine_Confidence_Spec.md` (v1.1), `D3_Critical_UX_Flows_v1.0.md` (v1.1), `E2_E4_Delivery_Observability_Spec.md` (v1.2), `F1_F2_Eval_Safety_Plan.md` (v2.3), `B6_Threat_Model.md` (v2.4), `G2_Glossary.md`
**Extracted from:** `critical_analysis-v2.md` §Document 3 (canonical per `suite_reconciliation.md`)

---

## 1. Purpose & Scope

Authoritative product requirements: reconciled MVP vs V2 targets, hardware-feasible constraints, testable acceptance criteria (FR-001–FR-022), OS/security invariants, and end-to-end traceability for Aegis Guardian.

**Audience:** Product, Engineering, Design/UX, QA, Security, Release Operations.

## 2. Reconciled Goals & Success Criteria (§3.1 + §16 Unified)

| Goal ID | Product Goal | North-Star / V2 Target | MVP (v1.0) Gate Target | Measurement Method |
|---|---|---|---|---|
| G1 | Find anything by meaning | >90% top-3 hit rate | **≥75% top-3 hit rate** | 500-query labeled eval corpus (MRR / Hit@3) |
| G2 | Surface & resolve duplication | ≥95% recall; ≥30% reclaimed on dirty drives | **≥85% exact+near recall** (hardlink-adjusted) | Synthetic + pilot dirty-drive corpus |
| G3 | Safe reviewable reorganisation | ≥80% proposal acceptance | **≥60% acceptance** | Simulator approval ratio telemetry |
| G4 | Complete auditable reversible mutations | 100% reversible; 0 unrecoverable | **100% reversible** (or conflict-prompted) | Chaos/crash-recovery + pre-rollback QA |
| G5 | Detect sensitive data | ≥99% secret recall | **≥95% recall; <5% critical FP rate** | Curated secrets/PII benchmark |
| G6 | Honest non-disruptive coverage | 100% coverage honesty | **100% coverage honesty; ≥99% crash-free** | Coverage delta checks (denied/cloud/locked counted explicitly) |

## 3. Functional Requirements & Testable Acceptance Criteria (MVP P0 Complete)

### 3.1 Discovery & Change Monitoring (FR-001, FR-010)

| ID | Requirement | Pri | Acceptance Criteria (Given/When/Then) |
|---|---|---|---|
| FR-001.1 | Multi-root scanning (internal, external, NAS/SMB, cloud-sync) | P0 | User adds/toggles roots; mounted NTFS/APFS/ext4/SMB root persists in `volumes` and starts Tier-1 scan within 3s. |
| FR-001.2 | `.aegisignore` + OS junk + build-cache exclusions | P0 | `.aegisignore` matching paths produce 0 rows in `files`/`chunks`; build-cache dirs (`node_modules`, `target/`, `.venv/`, `.gradle/`) skip interior `files`/`chunks` rows while recording one lightweight `cache_dirs` summary row for `FR-011.2` Disk Rescue; rule edits apply on next incremental sweep. |
| FR-001.3 | Graceful permission & lock handling | P0 | `EACCES`/`SharingViolation` never halts scan; path logged in `scan_jobs.errors_json`, `files_skipped_perm` increments, Coverage Report shows exact count. |
| FR-001.4 | Symlink/junction loop protection | P0 | Cyclic links skipped by default; visited-inode set prevents infinite loops. |
| FR-001.5 | Lazy BLAKE3 background hashing | P0 | Tier-1 enumeration completes without payload reads; hashing in throttled worker queue. |
| FR-001.6 | OS file-identity across renames/moves | **P0** | Rename/move within volume preserves `files.id`, `chunks`, embeddings, `relationships` via `(volume_id, file_index)` — no re-embedding. |
| FR-001.8 | Offline NAS/unmounted resilience | P1 | Mid-scan disconnect → `volumes.status='offline'`, `scan_jobs` checkpointed, files NOT marked missing until confirmed online. |
| FR-001.9 | Cloud placeholder safety | **P0** | `SF_DATALESS` / `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` files recorded `cloud_state='placeholder_remote'`; never opened for hash/extract unless explicitly opted in. |
| FR-001.10 | Hardlink-aware discovery | **P0** | `nlink>1` stored accurately; UI/dedupe never report shared hardlink bytes as independent reclaimable storage. |
| FR-010 | Watchers + MFT/USN/FSEvents + 15m polling fallback | P0 | Create/modify/rename/delete reflects in SQLite ≤60s (500ms debounce); restart reconciles missed events via USN/FSEvents or sampled checksum. |

*FR-001.7 (Email-export ingestion: mbox/eml/pst) is deferred to P2 / V2 per §7 Roadmap.*

### 3.2 Content Intelligence (FR-002)

| ID | Requirement | Pri | Acceptance Criteria |
|---|---|---|---|
| FR-002.1 | Rust-native MVP extractors (pdfium-render, DOCX/XLSX/PPTX, tree-sitter, MD/TXT/CSV/JSON, EXIF, zip/7z listing) | P0 | Valid fixtures for all MVP formats → non-empty text+metadata <2s/doc median, no Python spawned; corrupt files log parse error, coverage=partial. |
| FR-002.2 | Archive member indexing (`archive://`) | P0 | Zip/7z members (depth ≤2, ≤50MB) → `files` rows with `container_file_id`+`virtual_uri`; encrypted archives log status without crash. |
| FR-002.3 | Opt-in OCR & media transcription | P1/P2 | Off by default; enabling shows CPU/battery cost warning; OCR output tagged `source=ocr`. |

### 3.3 Knowledge Graph & Atomic Units (FR-003)

| ID | Requirement | Pri | Acceptance Criteria |
|---|---|---|---|
| FR-003.1 | Entity & temporal relationship extraction | P0 | Projects/Folders/People/Repos/Topics into `entities`/`relationships` with `valid_from`; 2-hop neighbourhood query <50ms p95. |
| FR-003.2 | Atomic Project Unit detection (ADR-004) | P0 | Dirs containing canonical markers per B8 ADR-004 (`.git`, `package.json`, `Cargo.toml`, `.app`, `.aegisboundary`, etc.) → `atomic_units` record; contained files link via `atomic_unit_id`. |

### 3.4 Duplicate Intelligence (FR-004)

| ID | Requirement | Pri | Acceptance Criteria |
|---|---|---|---|
| FR-004.1 | Exact (BLAKE3) + near-dup (dHash/SimHash) clustering | P0 | Identical files → 1 cluster, 1 keeper (shallowest clean path + newest access), exact hardlink-adjusted reclaimable bytes; resized JPEGs and ≥85% Jaccard-shingle text cluster accurately. |
| FR-004.2 | Semantic dup & version chains | P0/P1 | Cosine ≥0.92 in "Semantic Similar" tab (never auto-deleted); `_v1`/`_final` chains render ordered timeline UI. |

### 3.5 Refiling, Safety & Rollback (FR-005, FR-012, FR-013)

| ID | Requirement | Pri | Acceptance Criteria |
|---|---|---|---|
| FR-005.1 | Analyse→Simulate→Approve→Execute→Audit | P0 | No FS write outside `action_batches` execution; Simulator renders full before/after diff, collision policy (auto-rename suffix, never overwrite), per-file rationale+confidence. |
| FR-005.2 | Atomic Unit Protection Invariant (ADR-004) | P0 | Engine blocks proposals moving file out of `atomic_unit` unless `atomic_units.allow_split_override=1` or `files.unit_split_override=1` is explicitly confirmed. |
| FR-005.3 | Proposal-to-Rule Graduation | P0 | Approving a proposal offers "Always apply this pattern" → writes enabled deterministic rule (`source='graduated_proposal'`), previewable before activation. |
| FR-005.4 | Pre-Rollback State Reconciliation | P0 | Undo verifies `to_path` existence, `pre/post_blake3`, destination availability; unmodified revert atomically; modified/colliding files pause for per-file resolution (Restore as copy / Skip / Abort). |
| FR-012/013 | Global Undo Stack + Per-Volume Quarantine + Space Clarity (ADR-003) | P0 | Delete = O(1) rename to `<VolumeRoot>/.aegis-quarantine` (`expires_at=+30d`), writes `quarantine_entries`+`audit_log` hash entry, restores cleanly from Quarantine UI; Dashboard/Disk Rescue split **`Staged in Quarantine (30d)`** vs. **`Actually Freed on Disk`** (`quarantine.space_summary`) with 10% volume soft-quota warning (`V1.5-01` promoted to v1.0). |

### 3.6 Health, Search & Security (FR-006–FR-008)

| ID | Requirement | Pri | Acceptance Criteria |
|---|---|---|---|
| FR-006 | Explainable 5-dim Health score (0–100) | P0 | Clicking any sub-score opens exact top-10 offending files/folders with direct fix actions. |
| FR-007 | Hybrid Search (FTS5 BM25 + qdrant-edge ANN + RRF) | P0 | Top-20 results, highlighted chunks, "why-it-matched" badge, **<300ms p95** excl. LLM parse; filters `project: type: after: before: sensitivity:`. Files with `state='quarantine'` are excluded by default unless `include_quarantined=true` is explicitly passed (B6 §2). |
| FR-008 | Context-weighted deduplicated secret scanning | P0 | Detects API keys, .pem/.key, passwords, PII; identical secrets across N copies → 1 `security_findings` row (HMAC fingerprint, raw secret never stored); path-context risk weighting (Downloads > test fixtures). |

### 3.7 Lifecycle, Onboarding & Playbooks (FR-009, FR-011)

| ID | Requirement | Pri | Acceptance Criteria |
|---|---|---|---|
| FR-009 | Lifecycle states (Active→Warm→Cold→Archive) | P0 | Manual tagging + Search "Resurrection" prompt on opening archived file; automated Warm→Cold rules = V2. |
| FR-011.1 | Onboarding & 3-Tier Progressive Scan | P0 | Detects hardware profile, guides macOS TCC, honest Tier-1/2/3 ETAs, dashboard + exact-dupe quick wins **≤2min** after scan start. |
| FR-011.2 | Crisis Playbooks | P0 | Two one-click guided views on existing queries: (a) Disk Rescue (top reclaimable dupes + build caches + stale oversized), (b) Pre-Share Secret Audit (targeted folder scan). |

*V1.5–V3 items (FR-014 CLI, FR-015 Smart Collections, FR-016 Plugin SDK, FR-017 Scheduled Housekeeping, FR-018 Storage Analytics, FR-019 Digest, FR-020 Multi-device Sync, FR-021 Team Graph, FR-022 i18n/A11y) remain scoped per §7 Roadmap.*

## 4. Verified Resource & Throughput Budgets

### 4.1 Memory Budget During Active Scan (Standard 16GB)

| Component | Active Scan Cap | Interactive Burst | Notes |
|---|---|---|---|
| Tauri WebView + React | ≤350MB | ≤450MB | Virtualized lists; bg window throttled |
| Rust Core + Extractors | ≤600MB | ≤450MB | Bounded pool (cores/2, max 4) |
| SQLite WAL + page cache | ≤256MB | ≤256MB | `PRAGMA cache_size=-262144` |
| qdrant-edge HNSW + mmap payloads | ≤700MB | ≤700MB | INT8-quantized vectors |
| ONNX Embedding | ≤450MB | ≤450MB | Shared `ort` session |
| Local LLM (1.5–3B Q4) | **0MB (unloaded)** | ≤1800MB | On-demand; evict @300s idle |
| **TOTAL** | **≤2.36GB** | **≤4.10GB target** (≤5.30GB conservative) | Passes ≤3.65GB conservative scan SLA (`B1/B2` §3, `F3`) w/ ~1.6GB target headroom |

*Risk note (`B1/B2` §3, `F3`): treat 2.36GB as target scan RSS and 3.65GB as the conservative scan ceiling (≤4.10GB target interactive burst / ≤5.30GB if 1.8GB LLM burst overlaps 1.5GB conservative `qdrant-edge` cap). CI soak on real 5M-chunk corpus decides.*

### 4.2 Ingestion Throughput (1M files, 8-core laptop)

| Tier | Scope | Mechanism | Target |
|---|---|---|---|
| Tier 1 | 1M files metadata | NTFS MFT/USN or parallel `jwalk` + volumes/atomic_units/cache_dirs | ≤15min (≤90s typical 150k SSD) |
| Tier 2 | ~100k active docs | Rust extraction + selective chunking (~250k chunks @120/s INT8 CPU) | ≤40min |
| Tier 3 | ~900k cold/code/media | Metadata+BLAKE3+selective summary embedding (~1.05M chunks @120/s throttled; **~1.3M selective chunks total** across 1M files per `B1/B2` §3 & `F3`) | ≤2.5h — **total ~3.5h < 6h SLA** |

*Sprint-0 Benchmark Gate: validate real-hardware `ort` INT8 CPU embedding throughput (chunks/sec) during Sprint 0 before locking final 6h SLA tuning.*

## 5. OS Adapter & Permission Matrix

| Concern | Windows 10/11 | macOS 12+ | Linux Ubuntu 22.04+ |
|---|---|---|---|
| Fast discovery/recovery | NTFS USN Journal (elevated) / `jwalk` fallback | FSEvents stream IDs + `getattrlistbulk` | `inotify` + `jwalk` |
| Permissions | ACL skip + `files_skipped_perm` | Guided Full Disk Access/TCC; denied → Coverage | POSIX mode skip + `files_skipped_perm` |
| Cloud placeholders | `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` | `SF_DATALESS` (`st_flags`) | GVFS/rclone FUSE heuristics |
| File identity | VolumeSerialNumber + `BLOB` FileIndex (64/128-bit `FILE_ID_128`); `exFAT`/`SMB` uses `has_stable_inode=0` fallback | `st_dev`+`st_ino` (`BLOB`) | `st_dev`+`st_ino` (`BLOB`) |

*Precondition (`B1/B2` §5): Tier-1 timing assumes SSD/NVMe primary storage; HDD/SMB roots scale linearly and must be stated in NFR preconditions. `exFAT`/`SMB` mounts with non-persistent inodes fall back to `(volume_id, path_hash)` + `(size, mtime, blake3)` reconciliation (`B4` §3.1).*

## 6. Traceability Matrix (Personas → Execution)

| Persona | Core JTBD | Primary FRs | Schema Tables | UI Screens | Metric | Verification |
|---|---|---|---|---|---|---|
| Developer | Repo intelligence, secret scan, clean node_modules | FR-001.2, FR-002.1, FR-003.2, FR-005.2, FR-008 | `atomic_units`, `files`, `security_findings` | Dashboard (Disk Rescue), Security Inbox, Search | G5, 0 broken repos | Repo-boundary fuzz + secrets corpus |
| Researcher | Semantic PDF search, near-dupe papers | FR-002.1, FR-004.1, FR-007 | `chunks`, `chunks_fts`, `qdrant:chunks` | Search, Duplicates | G1, <300ms p95 | 500-query eval suite |
| Trader/Knowledge Worker | Statement refiling, safe undo, health | FR-005.1/.3/.4, FR-006, FR-013 | `actions`, `rules`, `quarantine_entries`, `audit_log` | Simulator, Approvals, Dashboard | G3, G4 | Crash-during-batch + pre-rollback conflict tests |
| Household Admin | Photo dedupe, drive rescue, family PII | FR-001.10, FR-004.1, FR-011.2, FR-013 | `files`(dhash_image,nlink), `quarantine_entries` | Onboarding, Duplicates, Disk Rescue | G2 | Multi-volume hardlink + USB quarantine test |

## 7. Roadmap

- **v1.0 MVP:** funnel indexing, Rust extraction set, hybrid search, basic graph, dedupe (exact+near+hardlink-aware), simulator+rollback, quarantine/undo, secrets triage, core UI, hardware tiers.
- **v1.5:** background service, hotkey/context menus, CLI, scheduled housekeeping, smart collections, storage analytics, 2D graph, DB encryption, built-in domains, OS fast-paths, optional Python runtime.
- **v2.0:** graduated autonomy, agent framework GA, lifecycle automation, vault, plugin SDK+marketplace, multi-device sync, cloud-LLM opt-in w/ redaction.
- **v3.0:** Twin Viewer, predictive housekeeping, knowledge OS, enterprise tier.

## 8. Open Questions

- OQ-01 (CEO/Product, Pre-Beta): licensing model.
- OQ-02 (Product/Legal, Milestone 1): license audit for bundled ONNX/GGUF models (prefer Apache-2.0/MIT over Llama community license).
- OQ-03 (Product, V1.5): macOS vs Linux parity order post-Windows.
- OQ-04 (Product, V2): enterprise/team commercial validation.
- OQ-12 (Platform Lead, Sprint 0): code-signing/notarization pipeline for `pdfium` dynamic lib.
- OQ-13 (Data Architect, Milestone 2): `routing_exemplars` expiry (18mo) vs recency decay.

## 9. Assumptions

- A-01: users accept ~2min Tier-1 wait before interactive dashboard.
- A-02: Rust extractors cover ≥95% of real-world MVP docs; exotic formats report `partial` until V1.5 plugins.
- ≥15% free disk headroom on primary volume.

## 10. Change Log

| Version | Date | Summary |
|---|---|---|
| 1.0 | — | Vision v1.0 |
| 2.0 | Pre-MVP | Comprehensive PRD v2.0 |
| 2.1 | 2026-11-20 | Reconciled goals/metrics, solved 4GB/1M-file math, file-ID→P0, ADR behaviors, full P0 acceptance criteria, OS matrix, traceability |
| 2.1.1 | 2026-11-20 | Synced `FR-005.2` per-file `unit_split_override` (`B4 v2.3` / `B8 v2.2`); applied `A2_PRD_v2.1_patch_notes.md` (Related Docs, FR-001.7 footnote, FR-003.2 markers, FR-007 quarantine filter, §7 ref) |
| 2.1.2 | 2026-11-20 | Clarified `cache_dirs` vs `.aegisignore` (`FR-001.2`), promoted `V1.5-01` Quarantine Space vs. Freed Space UI (`FR-012/013`), aligned §4.1/4.2 chunk & burst numbers with `B1/B2 v2.3` + `F3`, added `BLOB` 128-bit `file_index` + `exFAT`/`SMB` fallback in §5, and replaced dangling `I4`/`I7` refs |
