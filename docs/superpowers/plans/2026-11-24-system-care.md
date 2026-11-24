# Project 4: System Care

**Status:** Done — built, verified, and fast-forward merged into `master` on 2026-11-24 (not pushed).

**Goal:** Deliver calm, owner-first Windows System Care inside Aegis Guardian: reclaiming space from temporary files, crash dumps, browser/dev/build caches, Downloads tidy-ups, uninstall leftover folders, and unreferenced dead files; reviewing installed apps (via each app's own uninstaller or Windows Apps settings); read-only startup inventory; SMART / storage reliability health monitoring; daily drive and folder space trends; and an upgraded storage-and-integrity Workspace Health Score.

## North-star rules

This plan upholds the binding rules in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`:
- **K1 (propose, approve, undo) & K2 (30-day quarantine):** Every cleanup removal (temp files, crash dumps, browser/dev/build caches, Downloads items, dead large files, uninstall leftover folders) goes through `actions_propose -> approve_batch -> execute_batch` into per-volume `.aegis-quarantine` with 30-day retention and one-click undo (`rollback_batch`). Nothing is ever deleted directly from disk or removed without owner approval.
- **K3 (local only) & R9 (no network crates):** Zero outbound HTTP calls, zero online vulnerability/reputation lookups, zero external telemetry.
- **K5 (background budget <= 350 MB):** `SmartHealthWatch` runs every 6h via a single bounded read-only CIM/Storage probe (`CREATE_NO_WINDOW`) with zero resident background state.
- **R1 (no direct MFT parsing):** All file scanning uses standard user-mode directory walks and the existing USN-tracked SQLite catalog.
- **R3 (no registry cleaning / safe app removal):** Registry access is **strictly read-only** (`KEY_READ` only). Aegis **never** cleans, repairs, compacts, or deletes Windows Registry keys or `Run`/`StartupApproved` entries. Installed apps launch only their own interactive Windows uninstaller (`UninstallString` / `ms-settings:appsfeatures`), leftover user-profile folders go exclusively through 30-day quarantine approval (`K1`/`K2`), and startup items link to Windows' own Startup Apps settings (`ms-settings:startupapps`).
- **R8 (no multi-pass shredding):** Standard move-to-quarantine and normal expiry only.
- **North Star §2.2 #4 (Dead file detection) & #5 (Workspace health score):** Identifies stale unlinked large files outside projects and atomic units (`DeadFiles`) and computes the 5-dimension storage-and-integrity health score (`storage`, `backup`, `duplication`, `caches`, `organization`).

## Architecture & Verification Checklist

1. **Step 1 (`crates/aegis-core`):**
   - `care/` module (`mod.rs`, `cleanup.rs`, `apps.rs`, `startup.rs`, `smart.rs`, `trends.rs`, `watch.rs`) + `allowed_cleanup_roots` / `allowed_leftover_parents`.
   - Narrow `PolicyContext` allow-list (`is_care_quarantine_allowed`) enforcing `system_care` + `Quarantine` only within designated cleanup subtrees and safe leftover folders (`RequireApproval("quarantine_never_auto")` always enforced).
   - Tightened 2-word check in `engine::janitor::installer_matches`.
   - `Screen::Care`, `"drive_smart"` note label, `SmartHealthWatch`, and upgraded storage/integrity `health_breakdown`.
2. **Step 2 (`src-tauri`):** `commands/care.rs` (10 off-mutex/UI commands using registry-backed `app_id` uninstaller lookup and Windows Settings deep links) + `SmartHealthWatch` registration in `engine_host.rs`.
3. **Step 3 (`src/`):** TS bindings (`ts-rs`), `aegis.care` client API, `src/screens/SystemCare.tsx` (Overview & Health, Cleanup, Apps & Leftovers, Startup Items, Drive Health, Disk Trends), fixtures, and `System Care` tab in `App.tsx`.
4. **Step 4–5:** `crates/aegis-core/tests/system_care.rs` (6 integration tests with strict `TempDir` quarantine isolation), `cargo clippy --workspace --all-targets -- -D warnings`, full `cargo test --workspace`, `cargo check -p aegis-desktop --features custom-protocol`, `pnpm build`, quarantine `0` before/after, doc updates, and fast-forward merge into `master`.

## Verification

- `cargo test --workspace`: **543 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp` in `drives.rs`), including `6/6` in `crates/aegis-core/tests/system_care.rs` (`0.09s`), `6/6` in `hybrid_search.rs` (`0.19s`), `6/6` in `d1_encryption.rs` (`0.77s`), `5/5` in `aegis-embed` (`10.50s`, incl. live `nomic_gguf_live_semantic_check`), and `11/11` in `aegis-desktop`.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings).
- `cargo check -p aegis-desktop --features custom-protocol`: clean (`exit 0`).
- `pnpm build` (`tsc --noEmit && vite build`): clean (`dist/` built in `796ms`).
- `C:\.aegis-quarantine`: `0` folders before and `0` after.
