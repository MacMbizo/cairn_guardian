# AI Folderless Mode (`2.1`, Reshaped `R6`) Implementation Plan

**Branch:** `feature/folderless-mode` (cut from `master` at `f9f0404`)  
**Capability:** #11 — AI Folderless Mode (`2.1`, reshaped `R6`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-star rules

1. **R6 (Transparent Intent-to-Rule Compilation):** Plain-language organizational intents compile into **visible, editable, deletable filing rules** (`folderless_rules`) that the owner can read, tweak, preview, enable/disable, or delete. A model never silently places or renames files unseen; even optional Local LLM refinements populate the visible rule draft for explicit review before saving or staging.
2. **K1 (Propose, Approve, Undo):** Zero direct filesystem edits happen during compilation, preview, or rule creation. Applying a saved rule stages a standard `ActionType::Move` batch via `propose_batch` (`PolicyContext`-verified) inside Simulator for owner approval and single-click `rollback_batch`.
3. **K2 (Quarantine & Live File Guards):** Rule previews and batch staging strictly exclude deleted, quarantined, and missing files (`deleted_at IS NULL AND state NOT IN ('quarantine', 'missing')`), protect `atomic_unit_id IS NOT NULL` repositories/units from splitting, and never delete files.
4. **K3 / R9 (100% Local Zero-Network Inference):** Deterministic NLP intent compilation, content-anchor extraction (dates, fiscal quarters, organizations, amounts), and optional `OnDemandLlm` refinement run locally in pure Rust with zero new crates (`r9_forbids_networking_crates_in_engine_and_models`).
5. **K4 (Backup Drive Exclusion):** Backup volumes (`COALESCE(v.role, '') = 'backup'`) are never read as sources or targeted as destinations.
6. **K5 & R4 (On-Demand Inference Only):** Deterministic intent compilation, rule preview, and batch staging run at `0 MB` LLM RAM. Optional "Refine with Local LLM" loads `OnDemandLlm` behind the `rag_running` single-flight guard strictly off the SQLite lock.
7. **R7 (Privacy & Sensitivity Gate):** Confidential/secret files are excluded from rule previews and staging (`excluded_sensitive_count`) unless `include_sensitive == true && reveal_sensitive == true` (Windows authentication unlock).
8. **D1 (Encrypted Rule Storage):** `folderless_rules` lives inside SQLCipher-encrypted `aegis.db` via migration `0015_folderless_intents.sql`.

---

## Architecture & Steps

1. **Schema (`crates/aegis-core`)**
   - `migrations/0015_folderless_intents.sql` (`folderless_rules`) + `db/mod.rs` v15 + `tests/drives.rs` (`1..=15`).
2. **Core Engine (`crates/aegis-core/src/folderless/`)**
   - `mod.rs`: Contracts (`FolderlessRule`, `FolderlessRuleInput`, `CompiledIntentDraft`, `FolderlessMatchPreview`, `FolderlessPreviewReport`, `FolderlessBatchSummary`) + `Screen::Folderless` (`engine/mod.rs`).
   - `compiler.rs`: Plain-language intent compiler (`compile_intent_deterministic`, `build_intent_llm_prompt`, `apply_llm_refinement`) extracting scope, kinds/exts/doc_types/categories/orgs/keywords/age, target root, ordered dynamic path templates (`{year}`, `{quarter}`, `{month}`, `{org}`, `{project}`, `{doc_type}`, `{ext}`), and optional date-prefixed rename templates.
   - `engine.rs`: Safe destination validation (`validate_dest_root`), SQLCipher rule CRUD, content-anchor extractor (date -> quarter/year/month, `filing::orgs` + Knowledge Graph org/project, monetary amount detection), dry-run matcher (`preview_folderless_rule`), `PolicyContext`-verified batch staging (`propose_folderless_batch`), and recent batch revert tracking (`list_folderless_batches`).
3. **Contracts & Desktop IPC (`contracts/mod.rs`, `src-tauri/`)**
   - `ts-rs` exports for all 6 `folderless` types + `Screen::Folderless`.
   - `commands/folderless.rs`: 8 async Tauri commands including off-lock `folderless_compile_intent` (`use_llm`) and `folderless_propose_batch` (`ActionProposed` + `wake`).
4. **Frontend (`src/`)**
   - `src/bindings/client.ts` (`aegis.folderless`), `src/screens/fixtures/folderless.ts`, `src/screens/FolderlessMode.tsx` (Plain-language intent compiler, editable rule fields & token pills, dry-run preview with extracted content anchors & `aegis.file.get` inspection, Simulator `K1` staging CTA, and recent batch 1-click undo), and `src/App.tsx` (`Folderless Rules` tab).
5. **Verification & Merge**
   - `crates/aegis-core/tests/folderless_mode.rs` (`6` integration tests) + unit tests in `folderless::compiler::tests` & `folderless::engine::tests`.
   - Full `clippy`, `cargo test --workspace`, `R9`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, doc updates, and `--ff-only` merge into `master`.

## Verification Checklist

- [x] `cargo test -p aegis-core --test folderless_mode` — `6 passed; 0 failed` (`0.20s`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` — passed inside workspace run
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — `0` warnings (`17.40s`)
- [x] `cargo test --workspace` — `667 passed; 0 failed; 2 ignored` (`211` `aegis-core` unit + `436` integration across 23 test binaries + `11` `aegis-desktop` + `5` `aegis-embed` + `4` `aegis-llm`; ignored: `usn_refresh_on_system_temp`, `live_llama32_grounded_rag_answer`)
- [x] `cargo check -p aegis-desktop --features custom-protocol` — passed (`45.96s`)
- [x] `pnpm build` (`tsc --noEmit && vite build`) — `2362` modules built in `1.49s`
- [x] `C:\.aegis-quarantine` item count: `0`
