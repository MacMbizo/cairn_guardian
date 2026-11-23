# Context layer Plan 5: Step 7 modules (Software packages, Owner's folders, Resources, Series, Old copies & external drive plans)

**Status:** Built, verified, and fast-forward merged into `master` (2026-11-24). Nothing pushed.

**Goal:** Complete `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md` §1A and §16 step 7 by adding the five remaining context modules plus safe same-volume external-drive project planning (§17):
- **Software packages** (`context::software`): protect programs, utilities, driver packs, and multi-file installers from being split by extension (`DDU v18.1.5.7`, `Armoury_Crate_Uninstall_Tool`, `drivers`, Office/Acrobat installers) and route whole into `Software\Tools\<name>` or `Software\Installers\<name>` under Fresh.
- **Owner's folders** (`context::owners`): protect purpose-named document folders whose contents agree with the folder name (`CVs`, `PM`, `Ass. Bursar`, `Finance Manager`, `PhD`, `Last30Days`, `DocAtlas Notes`), routing whole into the matching life area under Fresh (or protecting in place when holding a local `.db` or already organized).
- **Resources** (`context::resources`): protect downloaded skills, UI kits, and design packs (`SKILL.md`, `DESIGN.md`, `glassmorphism`, `stitch_pwa_screen_interface_design (2)`) and route whole into `Documents\Resources\<topic>`.
- **Series** (`context::series`): group episodic TV/video files and companion subtitles (`S01E02`, `Avatar_Seven_Havens_S01E03`, `[►Average~Joe~US◄]~S02E02`, `24 S07E01`) and route into `Videos\Shows\<Show>\Season <n>` under Fresh.
- **Old copies** (`context::old_copies`): recognise old PC/backup copies (`D:\MSIDocuments`, `D:\MSIDownloads`, `Backup of ...`), compare against live folders using `context::relate`, propose same-volume identical/older copies for verified 30-day quarantine (with `Archive` fallback) and other copies for `ToArchive` (`<Archive>\<device-or-year>\<name>`), while never quarantining across drives.
- **External drive project & design-set plans** (§17): enable per-drive same-volume plans on external drives (e.g. `New Volume D:`) with strict same-drive quarantine safety (relating same-drive copies against the best copy on that drive, never against another drive) and offline `waits_for` guards.

**Tech Stack:** Rust, SQLite (existing schema `0006`–`0008`, zero new migrations), Tauri 2 IPC, React/TypeScript (`ts-rs` bindings). No new crates.

**Spec:** `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md` (§1A, §16 step 7, §17).

## North-star rules

This plan upholds the binding rules in `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` (§3):
- **K1 (propose, approve, undo):** Every folder move, episode route, archive move, or quarantine runs strictly through `Placement`/`ProjectPlan` → `propose_batch` → user approval → `execute_batch` → `rollback_batch`, with TOCTOU `digest` verification before running a plan and `folder_state` + `unproven_files` checks at execute time.
- **K2 (deletes ask, 30-day quarantine):** Nothing deletes outright. Same-volume identical or older copies propose 30-day quarantine only after execute-time BLAKE3 verification (`unproven_files`), falling back automatically to `Archive`/`Versions`. Cross-volume copies (e.g., an external drive copy matching the PC) are never quarantined.
- **K3 (local only):** Zero network calls or external models; purely local catalog SQL and deterministic heuristics.
- **K5 (background budget):** Binary/installer/resource/database signals piggyback on the single SQL pass in `profile::profiles_for` without extra queries per folder, keeping full-catalog recognition fast and memory-bounded.
- **D1 compatibility:** Reads and writes only existing single-store SQLite tables (`collections`, `collection_locations`, `collection_members`, `context_decisions`, `project_plan_runs`, `actions`, `action_batches`), directly compatible with SQLCipher whole-store encryption.

## Scope & Priority Order

In `context::recognise`:
1. **Old copies** (`depth == 1`) claim backup trees like `D:\MSIDownloads` / `D:\MSIDocuments` before project recognition so nested repos inside old backups aren't lifted to claim the whole backup folder as one project.
2. **Projects** run over remaining profiles, with `software::looks_like_software_package` excluding binary utilities/installers (even when they carry a `readme.txt` like `DDU v18.1.5.7`).
3. **Software packages** run outside claimed `OldCopy` + `Project` trees (so a software bundle inside a project stays part of that project).
4. **Owner's folders** run outside claimed `OldCopy` + `Project` + `SoftwarePackage` trees.
5. **Resources** run outside all four above.
6. **Design sets & Photo subjects** (`sets::find_sets`) receive all five folder modules' roots as exclusions.
7. **Series** run outside all folder modules' roots.

## Architecture & Outcomes

| Component | Key Additions |
|---|---|
| `crates/aegis-core/src/context/{mod,store,profile}.rs` | `CollectionKind::{SoftwarePackage, OwnersFolder, Resource, Series, OldCopy}`, `ClaimedIndex`, `module_folder_groups`, `series_episodes`, and 7 piggybacked binary/installer/resource/db columns in `FolderProfile` |
| `crates/aegis-core/src/context/{software,owners,resources,series,old_copies}.rs` | Five Step 7 recognisers (`INTERNAL_SW_DIRS` composite-installer preservation, `.db`-safe Owner's folders, `SKILL.md`/`DESIGN.md`/kit resources, episodic video+subtitle pairing, and depth-1 `OldCopy` backup matching) |
| `crates/aegis-core/src/context/{recognise,sets,answers,reunite}.rs` | Full 7-stage module cascade, `RecogniseReport` Step 7 counts, shared `build_folder_module_collections`, `BUILD_DIRS` Rust post-filter, exhaustive 8-kind `answers`/`reunite` support |
| `crates/aegis-core/src/context/plan.rs` & `filing/{blueprint,batches}.rs` | `StepKind::ToArchive`, same-volume quarantine/Archive for `OldCopy`, external-drive per-volume plans, `Planner` `offline_dirs`/`projects_homes`/`summaries` caches, `Fresh`/`Tidy` module folder & `Shows/<Show>/Season <n>` routing, `OldCopy` preservation over inner `atomic_units` |
| UI, IPC & CLI dry run | `src-tauri/src/commands/context.rs`, `src/screens/{Projects.tsx,fixtures/projects.ts}`, `src/bindings/{CollectionKind,RecogniseReport,StepKind}.ts`, `examples/filing_dry_run.rs` |

## Verification

- **Live-DB dry run (`examples/filing_dry_run.rs` on read-only `VACUUM INTO` copy):**
  - Recognition (`127.3s` debug): `239` projects (`84` multi), `1` design set (`MBIZO 1edited`), `5` photo subjects, `18` software packages (`Microsoft Office 2024 Pro Plus LTSC...` whole at `102` files), `9` owner's folders (`Ass. Bursar`, `CVs`, `DocAtlas Notes`, `Finance Manager`, `Invoices & receipts`, `Last30Days`, `PhD`, `PM`, `Audit 2022`), `4` resources (`antigravity-ui-motion-design-expert`, `glassmorphism`, `stitch_pwa_screen_interface_design (2)`, `Templates`), `52` series seasons, `2` old copies (`MSI Documents` `4256` files, `MSI Downloads` `7111` files).
  - Project plans (`249` plans in `18.8s` debug, `52.5x` faster than uncached `988.0s`): `234` homes moving, `25` copies to quarantine (`1.47 GB`), `83` kept as versions, `3` material brought in, `198` waiting (`New Volume` unplugged).
  - Blueprints keep `"Old copy": 9747` whole on `New Volume` (`MSIDocuments 7602`, `MSIDownloads 7183` intact in `Fresh`) and protect `"Software package": 287`, `"Owner's folder": 31`, `"Resource": 5` on `This PC`. Temporary DB removed (`0` remaining).
- **Post-edit test & build gate:**
  - `cargo test --workspace`: **503 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), including all `7` tests in `crates/aegis-core/tests/context_modules.rs`, `46` in `project_plans.rs`, `97` in `filing.rs`, `35` in `context.rs`, `11` in `sets.rs`, `6` in `reunite_and_learning.rs`, `116` `aegis-core` unit tests, `11` `aegis-desktop` unit tests, `1` `aegis-embed` test; `0` items in `C:\.aegis-quarantine` before and after.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings).
  - `cargo check -p aegis-desktop --features custom-protocol`: clean.
  - `pnpm build` (`tsc --noEmit && vite build`): clean (`dist/` built in `881ms`).
