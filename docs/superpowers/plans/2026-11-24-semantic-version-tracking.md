# Semantic Version Tracking Implementation Plan

**Branch:** `feature/semantic-version-tracking` (cut from `master` at `d1b3d3a`)  
**Capability:** #7 — Semantic Version Tracking (`3.4`, PRD `FR-004.2`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-Star Constraints & Invariants

1. **K1 (Propose, Approve, Undo):** Reading/grouping/diffing version chains is strictly read-only. Archiving older revisions into `Versions\` (`Move`) and quarantining byte-identical redundant copies (`Quarantine`) route exclusively through `propose_version_archive` / `propose_version_quarantine_dupes` -> `build_policy_context` -> `propose_batch` with full simulation, approval, execution, and undo.
2. **K2 & FR-004.2 (Quarantine Exclusion & No Auto-Delete of Semantic Variants):** Scans and diffs exclude `deleted_at IS NOT NULL OR state IN ('quarantine', 'missing')`. Semantic/near-duplicate versions (`cosine >= 0.92`) are NEVER auto-deleted or quarantined as duplicates; only byte-identical `blake3` duplicates may enter quarantine, always honoring `MIN_COPIES = 2`, `survivor_file_id` verification, and 30-day per-volume quarantine.
3. **K3 / R9 (100% Local Zero-Network Engine):** Pure-Rust pattern normalization, Union-Find clustering, LCS text diffing, cosine similarity over `vec_chunks`, and optional on-demand local GGUF LLM diff summarization via existing `aegis-llm` (`r9_forbids_networking_crates_in_engine_and_models`).
4. **K4 (Backup Drives Never Read):** Queries exclude volumes with `COALESCE(v.role, '') = 'backup'` at SQL query time.
5. **K5 (On-Demand Inference Only):** Scan and deterministic diffing use zero LLM RAM. Optional natural-language diff summaries reuse existing `OnDemandLlm` only when the owner clicks `"Explain changes (Local LLM)"`, unloading per `K5` (`> 2.5 GB` immediate, `<= 2.5 GB` 60s idle / window close), and caching non-sensitive results in `version_diff_summaries`.
6. **R7 (Privacy & Sensitivity Gate):** Locked sensitive files (`confidential` / `secret` when `!reveal_sensitive`) never participate in `vec_chunks` similarity clustering, never expose `semantic_similarity_prev`, return `redacted_sensitive: true` with empty hunks and masked summary in `diff_version_pair`, refuse `versions_summarize_diff` with `PermissionDenied`, and never cache sensitive summaries in `version_diff_summaries`.
7. **D1 (Encrypted Metadata Storage):** `version_chain_prefs` and `version_diff_summaries` live inside SQLCipher-encrypted `aegis.db` via migration `0011_version_tracking.sql`.

---

## Architecture & Steps

1. **Schema & Vector Helpers (`crates/aegis-core`)**
   - `migrations/0011_version_tracking.sql` + `db/mod.rs` registration + `tests/drives.rs` migration list update (`1..=11`).
   - `db/vec.rs`: `le_bytes_to_f32s`, `cosine_similarity`, and `load_file_vectors` reading normalized per-file vectors from encrypted `vec_chunks` + `chunk_embeddings_meta`.
2. **Core Engine (`crates/aegis-core/src/versions/`)**
   - `mod.rs`: Contracts (`VersionLinkReason`, `VersionScanOptions`, `VersionNode`, `VersionChain`, `VersionScanReport`, `DiffLineKind`, `DiffLine`, `DiffHunk`, `VersionDiffSummary`, `VersionDiffReport`) and `Screen::Versions` (`engine/mod.rs`).
   - `patterns.rs`: Filename/prefix/suffix/date/`Versions\` normalization, boilerplate stem exclusion, and relative-path exclusion (`strip_prefix(&mount_point)`).
   - `chains.rs`: Multi-signal Union-Find version chain builder (`FilenamePattern`, `VersionsFolder`, `FileHistory`, `SemanticSimilarity`), exact `blake3` duplicate flagging, timeline ordering, pin/dismiss persistence.
   - `diff.rs`: `diff_version_pair` (chunk or safe online plain-text LCS diffing with prefix/suffix trimming, conceptual section & term delta badges, binary/metadata fallback, `R7` gate), `build_diff_llm_prompt`, and cached summary store.
   - `actions.rs`: `propose_version_archive` (`Move`) and `propose_version_quarantine_dupes` (`blake3`-verified -> `propose_dupe_quarantine_pairs`).
   - `contracts/mod.rs`: `ts-rs` exports for all 10 version types + `Screen::Versions`.
3. **Desktop IPC (`src-tauri`)**
   - `commands/versions.rs`: `versions_scan`, `versions_diff`, `versions_summarize_diff`, `versions_pin_latest`, `versions_dismiss`, `versions_undismiss`, `versions_propose_archive`, `versions_propose_quarantine_dupes`.
4. **Frontend (`src/`)**
   - `src/bindings/client.ts` (`aegis.versions`), `src/screens/fixtures/versions.ts`, `src/screens/VersionTracking.tsx` (timeline + side-by-side / unified diff + local LLM explain + `aegis.file.get` inspect + `K1` actions), `src/App.tsx` (`Version Tracking` tab).
5. **Verification & Merge**
   - `crates/aegis-core/tests/semantic_versions.rs` (7 integration tests).
   - `clippy`, `cargo test --workspace`, `R9`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, doc updates, and `--ff-only` merge into `master`.

---

## Verification Checklist

- [x] `cargo test -p aegis-core --lib` (`163 passed, 0 failed`, incl. `db::vec::tests` and `versions::patterns::tests`)
- [x] `cargo test -p aegis-core --test semantic_versions -- --nocapture` (`7 passed, 0 failed` in `0.24s`)
- [x] `cargo test -p aegis-core --test drives` (`19 passed, 0 failed, 1 ignored`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` (`1 passed`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (`0` warnings)
- [x] `cargo test --workspace` (**594 passed, 0 failed, 2 ignored**)
- [x] `cargo check -p aegis-desktop --features custom-protocol` (exit `0`)
- [x] `pnpm build` (exit `0`, `dist/` built in `793ms`)
- [x] `C:\.aegis-quarantine` verified `0` items before and after
