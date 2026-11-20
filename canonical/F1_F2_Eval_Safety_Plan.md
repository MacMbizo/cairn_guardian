# F1/F2 — Evaluation, NFR Soak & Safety Test Plan (v2.3)

**ID:** F1/F2 · **Version:** 2.3 · **Status:** Required MVP Exit Gate
**Owner:** QA & ML Eval Lead · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (v2.1.2), `B1_B2_Architecture.md` (v2.3), `B4_Data_Vector_Schema_v2.3.md` (v2.3.1), `B6_Threat_Model.md` (v2.4), `C1_IPC_Action_Contracts.md` (v1.2), `C4_Policy_Engine_Confidence_Spec.md` (v1.1)

> **v2.3 changes:** Removed dangling `suite_reconciliation I4` citation; aligned `F3` interactive burst gate to `≤4.1GB target (≤5.3GB conservative ceiling)` matching `B1/B2 v2.3`; added Sprint-0 CPU embedding microbenchmark gate and `exFAT`/atomic-save/case-rename/`cache_dirs` cases to `F2` & `F3`.
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
3. **Atomic Units & Safe Cache Carve-Out (`ADR-004`, `C4 v1.1`):** Fuzz Curator/rules proposals against repos & `.aegisboundary` fixtures → assert `0` interior mutations without explicit `unit_split_override` / `allow_split_override`, while registered `cache_dirs` (`node_modules`, `target/`) allow `Suggest`-mode quarantine without leaving `files` rows.
4. **Per-volume quarantine (`ADR-003`):** Test multi-volume deletes (files + `cache_dirs`) → assert `O(1)` same-volume rename; test read-only/unmounted volume → assert `failed:quarantine_unavailable` + explicit prompt.
5. **Pre-Rollback Conflict Reconciliation (`FR-005.4`, `C1 v1.2`):** Test (a) externally modified `to_path` (`revert_keeping_edits` / `copy_back_keeping_both`), (b) reoccupied `from_path` (`restore_as_rename`), (c) deleted `to_path` (`unrecoverable_missing`), and pre-flight abort before partial batch mutation → never silent clobber or false `rolled_back` status.
6. **Crash recovery & `aegis doctor`:** `kill -9` at every stage (scan, outbox sync, batch execution, rollback) → restart + `aegis doctor` leaves DB consistent, repairs FTS drift, and catches orphan relationships.
7. **Cloud-placeholder guard:** Verify `0` payload reads (`read()` count == 0) on `SF_DATALESS` / `RECALL_ON_DATA_ACCESS` files.
8. **Prompt-injection, search safety & zero-egress (`B6` §3–§4):** Run injection corpus + offline scan/search (including quarantined items while outbox lags) under packet capture → `0` injection-triggered actions, `0` quarantined search leaks, `0` non-localhost packets, `0` raw secret strings in DB/logs.

---

## F3 — NFR Scale, Memory & Latency Soak Suite (`B1/B2` §3)

Executed on dedicated `Standard` reference hardware (16GB RAM, 8-core CPU, NVMe SSD, **no discrete GPU**), anchored by a Sprint-0 `ort` INT8 CPU embedding throughput microbenchmark:

| Benchmark | Dataset | Pass Gate | Purpose |
|---|---|---|---|
| **Sprint-0 `ort` CPU Microbenchmark** | 10k real 384-dim INT8 chunks | Measure chunks/sec on 4 CPU workers (`≥120 chunks/s` target) | Validates 6h SLA assumptions early (`A2` §4.2) |
| **1M-File Scan Throughput** | 1M synthetic/OSS files (~1.3M selective chunks) | Tier 1 **≤15m**; Total 3-Tier **≤6h** | Proves `A2` §4.2 throughput math |
| **5M-Chunk Memory Soak** | 5M chunks loaded in SQLite + `qdrant-edge` | Scan peak RSS **≤3.65GB** (target ≤2.36GB); Interactive LLM burst **≤4.1GB target (≤5.3GB conservative ceiling)** | Verifies `B1/B2` §3 RAM ceiling |
| **Hybrid Search Latency** | 500 queries over 5M chunks (`FTS5` + `qdrant-edge`) | **p95 <300ms** (excl. LLM parse) | Proves `FR-007` NFR |
| **Rename, Hardlink & Atomic-Save Stress** | 10k renames + 1k hardlinks + atomic-save replacements + case-only renames + `exFAT` remount (`has_stable_inode=0`) | `0` constraint errors; `100%` preserved `files.id` on stable volumes | Proves `B4 v2.3.1` §3.1 & `idx_files_os_path`/`idx_files_live_path` |

## Metadata & Traceability
- **Acceptance:** All F1/F2/F3 gates green in release CI pipeline prior to MVP v1.0 tag.
- **Open:** `[OPEN]` `OQ-14` (Owner: QA Lead, Sprint 1): Provision dedicated 16GB Windows + macOS CI runners for F3 soak tests.
- **Assumptions:** Synthetic 1M-file tree matches empirical file-size/token distribution (~1.3 chunks/file avg).
- **Traceability:** Upstream `A2` (`v2.1.2` `G1–G6`), `B1/B2 v2.3`, `B4 v2.3.1`, `B6 v2.4`, `C1 v1.2`, `C4 v1.1`; Downstream CI release gate.
