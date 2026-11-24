# Forgotten Asset Recovery Implementation Plan

**Branch:** `feature/forgotten-asset-recovery` (cut from `master` at `1db3c43`)  
**Capability:** #6 — Forgotten Asset Recovery (`4.3`) per `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`

---

## North-Star Constraints

1. **K1 (Reversible Recovery via Simulator Batches):** Every asset recovery move routes through `propose_asset_recovery` -> `build_policy_context` -> `propose_batch` (`origin: "forgotten_asset_recovery"`), requiring explicit simulation, approval, execution, and full undo.
2. **K2 (Quarantine & Missing Exclusion):** Recovery scans read `files` where `deleted_at IS NULL AND state NOT IN ('quarantine', 'missing')`, excluding backups, containers, and atomic units.
3. **K3 / R9 (100% Local Zero-Network Inference & Header Parsing):** Pure-Rust binary/SVG header parsing (`PNG`, `JPEG`, `GIF`, `BMP`, `WebP`, `PSD/PSB`, `SVG`) with zero external binaries or networking crates (`r9_forbids_networking_crates_in_engine_and_models`).
4. **K4 (Backup Drives Never Read):** Queries exclude volumes with `COALESCE(v.role, '') = 'backup'` at SQL query time.
5. **K5 (On-Demand Memory):** Optional hybrid search reuses the existing on-demand `OnDemandEmbedder` only when `vec_chunks` exists and `query` is non-empty, releasing the SQLite lock prior to embedding.
6. **R7 (Privacy & Sensitivity Gate):** Sensitive files (`confidential` / `secret`) hide snippets (`SENSITIVE_MASK` when chunk exists, else `None`) and exclude hidden chunk text from queries unless Windows CredUI sensitive unlock is active.
7. **D1 (Encrypted Metadata Storage):** `asset_meta` and `asset_dismissals` live inside SQLCipher-encrypted `aegis.db` via migration `0010_forgotten_assets.sql`.

---

## Architecture & Steps

1. **Schema & Core Engine (`crates/aegis-core`)**
   - `migrations/0010_forgotten_assets.sql` + `db/mod.rs` registration + `Screen::Assets` (`engine/mod.rs`).
   - `assets/formats.rs`: `AssetFamily` (`8` families across `80+` formats) and header-only `VisualHeader` parser.
   - `assets/recovery.rs` & `assets/mod.rs`: `scan_forgotten_assets`, `enrich_asset_metadata`, `dismiss_forgotten_asset`, `undismiss_forgotten_asset`, and policy-checked `propose_asset_recovery`.
   - `contracts/mod.rs`: TS exports for all 9 `assets` types and `Screen::Assets`.
2. **Desktop IPC (`src-tauri`)**
   - `commands/assets.rs`: `assets_scan`, `assets_enrich_metadata`, `assets_dismiss`, `assets_undismiss`, `assets_propose_recovery`.
3. **Frontend (`src/`)**
   - `src/bindings/client.ts` (`aegis.assets`), `src/screens/fixtures/assets.ts`, `src/screens/ForgottenAssets.tsx`, `src/App.tsx` (`Forgotten Assets` tab).
4. **Verification & Merge**
   - `crates/aegis-core/tests/forgotten_assets.rs` (`6` integration tests).
   - Full `clippy`, `cargo test --workspace`, `R9`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` verification, doc updates, and `--ff-only` merge into `master`.

---

## Verification Checklist

- [x] `cargo test -p aegis-core --lib` (`149 passed, 0 failed`, incl. `formats::tests`)
- [x] `cargo check -p aegis-desktop --features custom-protocol` (exit `0`)
- [x] `cargo test -p aegis-desktop --lib` (`11 passed, 0 failed`)
- [x] `pnpm build` (exit `0`, `dist/` built in `792ms`)
- [x] `cargo test -p aegis-core --test forgotten_assets -- --nocapture` (`6 passed, 0 failed`)
- [x] `cargo test -p aegis-core --test d1_encryption r9_forbids_networking_crates_in_engine_and_models` (`1 passed`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (`0` warnings)
- [x] `cargo test --workspace` (**573 passed, 0 failed, 2 ignored**)
- [x] `C:\.aegis-quarantine` verified `0` items before and after
