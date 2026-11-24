# Cross-Project Intelligence Implementation Plan

**Branch:** `feature/cross-project-intelligence` (cut from `master` at `e74b623`)  
**Capability:** #10 — Cross-Project Intelligence (`3.3`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-star rules

1. **K1 (Read-Only on Disk & Reversible Triage):** Cross-Project Intelligence is strictly read-only toward project files and folders on disk. Finding triage states (`open`, `acknowledged`, `resolved`, `dismissed`) and reviewer notes live inside SQLCipher-encrypted metadata and can be updated or reopened in one click.
2. **K2 (Quarantine & Missing Exclusion):** Multi-project aggregation, file loading, manifest scanning, and entity provenance strictly exclude deleted, quarantined, and missing files (`deleted_at IS NULL AND state NOT IN ('quarantine', 'missing')`).
3. **K3 / R9 (100% Local Zero-Network Inference):** All pairwise semantic comparisons, lexical similarity, conflict detection, manifest dependency parsing, and optional executive brief synthesis run offline purely in Rust with zero new dependencies (`r9_forbids_networking_crates_in_engine_and_models`).
4. **K4 (Backup Drive Exclusion):** Backup drives (`COALESCE(v.role, '') = 'backup'`) are excluded at SQL query time.
5. **K5 & R4 (On-Demand Inference Only):** Project synthesis and overlap/conflict/dependency detection run deterministically over pre-computed encrypted `collections`, `chunks`, `vec_chunks`, and `entities`/`relationships` at `0 MB` LLM/embedder RAM. Optional executive briefs invoke `OnDemandLlm` behind the `rag_running` single-flight guard strictly off the SQLite lock.
6. **R5 & R11 (No Screen Capture or Keystroke Monitoring):** Operates strictly from saved project directories, files, manifests, and extracted knowledge on disk — never captures active windows, keystrokes, clipboard, or screen state.
7. **R7 (Privacy & Sensitivity Gate):** Confidential/secret documents are excluded from project metrics, duplicate-effort comparisons, cross-project mentions, manifest scans, and LLM briefs when `!reveal_sensitive` (counting `excluded_sensitive_count` without leaking snippets or scores), and `include_sensitive = true` is rejected until Windows authentication unlocks sensitive previews.
8. **D1 (Encrypted Metadata Storage):** `cross_project_insight_state` lives inside SQLCipher-encrypted `aegis.db` via migration `0014_cross_project.sql`.

---

## Architecture & Steps

1. **Schema (`crates/aegis-core`)**
   - `migrations/0014_cross_project.sql` (`cross_project_insight_state`) + `db/mod.rs` v14 + `tests/drives.rs` (`1..=14`).
2. **Core Engine (`crates/aegis-core/src/cross_project/`)**
   - `mod.rs`: Contracts (`FindingKind`, `FindingSeverity`, `InsightStatus`, `DependencyKind`, `ProjectNodeSummary`, `CrossProjectFileRef`, `CrossProjectFinding`, `CrossProjectEdge`, `CrossProjectOptions`, `CrossProjectReport`, `CrossProjectBriefCitation`, `CrossProjectBrief`) + `Screen::CrossProject` (`engine/mod.rs`).
   - `manifests.rs`: Pure-Rust dependency parser for `package.json`, `Cargo.toml`, `requirements.txt`, and `pyproject.toml`.
   - `engine.rs`: Project loader, `DuplicateEffort` detector (`blake3`, `vec_chunks` cosine, lexical Jaccard fallback), `ConflictingDecision` detector across approved `Decision` entities, `VersionDivergence` detector across shared packages, 5-kind `CrossProjectEdge` dependency mapper (`SharedStakeholder`, `SharedDecision`, `CrossProjectAction`, `DocumentReference`, `SharedStack`), triage persistence (`set_insight_status`), and grounded `OnDemandLlm` brief preparation (`prepare_cross_project_brief`).
3. **Contracts & Desktop IPC (`contracts/mod.rs`, `src-tauri/`)**
   - `ts-rs` exports for all 12 `cross_project` types + `Screen::CrossProject`.
   - `commands/cross_project.rs`: `cross_project_scan`, `cross_project_set_insight_status`, `cross_project_synthesize_brief`.
4. **Frontend (`src/`)**
   - `src/bindings/client.ts` (`aegis.crossProject`), `src/screens/fixtures/crossProject.ts`, `src/screens/CrossProject.tsx` (`Overlaps & Conflicts`, `Dependency Map`, `Projects Overview`, `aegis.file.get` inspection, on-demand `Local LLM` brief), and `src/App.tsx` (`Cross-Project` tab).
5. **Verification & Merge**
   - `crates/aegis-core/tests/cross_project.rs` (`6` integration tests) + unit tests in `cross_project::manifests::tests` and `cross_project::engine::tests`.
   - Full `clippy`, `cargo test --workspace`, `R9`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, doc updates, and `--ff-only` merge into `master`.

---

## Verification Checklist

- [x] `cargo test -p aegis-core --lib` (`203 passed, 0 failed` in `0.54s`, incl. `cross_project::manifests::tests` and `cross_project::engine::tests`)
- [x] `cargo test -p aegis-core --test cross_project` (`6 passed, 0 failed` in `0.17s`)
- [x] `cargo test -p aegis-core --test drives` (`19 passed, 0 failed, 1 ignored` in `1.46s`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` (passed within `6/6` in `d1_encryption.rs`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (`0` warnings)
- [x] `cargo test --workspace` (**653 passed, 0 failed, 2 ignored**)
- [x] `cargo check -p aegis-desktop --features custom-protocol` (exit `0`, `49.24s`)
- [x] `pnpm build` (exit `0`, `dist/` built in `939ms`)
- [x] `C:\.aegis-quarantine` verified `0` items before and after
