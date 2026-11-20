# Suite Reconciliation & Living Index (v1.3)

**Date:** 2026-11-20 · **Status:** Synchronized MVP Suite — 13/13 indexed documents verified green (plus this index = 14 files in `canonical/`)

## 1. What Changed in v1.3 Reconciliation
Cleaned up `canonical/` (moved superseded `B4_Data_Vector_Schema_v2.2.md` and applied `A2_PRD_v2.1_patch_notes.md` to `deprecated/`, unwrapped `V1.5_Feature_Backlog.md`) and patched high-severity architectural/schema contradictions across the suite:
1. `B4 v2.3.1`: Fixed non-unique `idx_actions_file_status` (`UNIQUE(file_id) WHERE file_id IS NOT NULL AND status IN ('proposed','approved')`); added `cache_dirs` table + `target_kind ('file'|'dir'|'atomic_unit')` on `actions`/`quarantine_entries`; switched `files.file_index` to `BLOB` (64/128-bit) + added `volumes.has_stable_inode`/`case_sensitive` + `idx_files_live_path`; rewrote §3.1 Rename-vs-Hardlink algorithm for atomic-save swaps, case-only renames, inode reuse, and `exFAT`/`SMB` fallback; fixed `security_finding_locations` PK (`(finding_id, file_id, line_number)`); specified canonical length-prefixed `BLAKE3` framing for `audit_log`; mandated SQLite `files.state != 'quarantine'` authoritative search post-filter; documented soft-delete purge & MIH/LSH near-dup indexing.
2. `C4 v1.1`: Added `safe_build_cache` carve-out in Step 3 for `cache_dirs`; marked `security_remediate` as V2-reserved; fixed §4 pseudocode to check both `from_path` and `to_path` against `.aegis` internals, enforce `ActionType::Purge` explicit confirmation, and enforce `Housekeeper`/`Security` agent ceilings.
3. `C1 v1.2` & `C1_B4_Alignment_Check v1.2`: Added missing MVP commands (`roots.*`, `ignore.*`, `scan.coverage_report`, `file.set_lifecycle`, `quarantine.space_summary`); enforced pre-flight batch rollback verification before disk mutation; removed fictional pre-edit content backup on `modified` conflicts (`revert_keeping_edits` / `copy_back_keeping_both`).
4. `A2 v2.1.2` & `B1/B2 v2.3`: Promoted `V1.5-01` (Quarantine Space vs. Freed Space UI) to v1.0 MVP; reconciled `cache_dirs` vs. `.aegisignore`; separated Target (`≤4.10GB`) vs. Conservative (`≤5.30GB`) interactive LLM burst RSS; aligned ~1.3M selective chunk count + Sprint-0 `ort` CPU microbenchmark; documented `BLOB` `file_index` + `exFAT`/`SMB` fallback and MIH/LSH near-dup indexing.
5. `B6 v2.4`, `B8 v2.3`, `D3 v1.1`, `E2/E4 v1.2`, `F1/F2 v2.3`, `V1.5 v1.1`: Scoped loopback REST API auth gate to V1.5; documented `safe_build_cache` & directory targets in ADRs; aligned `D3` conflict radios & Disk Rescue space header; added `aegis doctor` Check #8 (orphan relationships & stale `cache_dirs`); synced `F2`/`F3` tests and removed dangling `I4`/`I7`/`P1–P7` references.

## 2. Unified Living Index (`G1`)
| ID | Document | Version | Status | File |
|---|---|---|---|---|
| **A2** | Product Requirements Document | **`2.1.2`** | ✅ Canonical & Synced | `A2_PRD_v2.1.md` |
| **A2-V1.5** | V1.5 Feature Backlog & Delta Spec | **`1.1`** | ✅ Canonical & Synced | `V1.5_Feature_Backlog.md` |
| **B1/B2** | System & Runtime Architecture | **`2.3`** | ✅ Canonical & Synced | `B1_B2_Architecture.md` |
| **B4** | Data & Vector Specification | **`2.3.1`** | ✅ Canonical & Synced | `B4_Data_Vector_Schema_v2.3.md` |
| **B6** | Security Threat Model | **`2.4`** | ✅ Canonical & Synced | `B6_Threat_Model.md` |
| **B8** | Architecture Decision Records (`001–004`) | **`2.3`** | ✅ Canonical & Synced | `B8_ADRs.md` |
| **C1** | IPC, Events & Action Contracts | **`1.2`** | ✅ Canonical & Synced | `C1_IPC_Action_Contracts.md` |
| **C1↔B4** | IPC ↔ Schema Alignment Check | **`1.2`** | ✅ Verified Aligned | `C1_B4_Alignment_Check.md` |
| **C4** | Policy Engine & Confidence Spec | **`1.1`** | ✅ Canonical & Synced | `C4_Policy_Engine_Confidence_Spec.md` |
| **D3** | Critical UX Flows & A11y Baseline | **`1.1`** | ✅ Canonical & Synced | `D3_Critical_UX_Flows_v1.0.md` |
| **E2/E4** | Delivery, Packaging & Observability | **`1.2`** | ✅ Canonical & Synced | `E2_E4_Delivery_Observability_Spec.md` |
| **F1/F2** | Eval, NFR Soak & Safety Test Plan | **`2.3`** | ✅ Canonical & Synced | `F1_F2_Eval_Safety_Plan.md` |
| **G2** | Canonical Glossary | **`1.0`** | ✅ Canonical | `G2_Glossary.md` |

## 3. Next Action
All 13 indexed documents (14 files in `canonical/`) verified and synchronized (v1.3 reconciliation). **Cleared for Sprint 0 scaffolding:** `B4 v2.3.1` SQLite migrations → `C1 v1.2` `ts-rs` IPC bindings → `C4 v1.1` Policy Engine unit tests → `F2`/`F3` safety & scanner invariants.
