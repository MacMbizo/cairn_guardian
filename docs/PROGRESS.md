# Aegis Guardian: progress and what remains

Updated 2026-11-23. This is the whole-project view. `HANDOFF.md` covers the working detail for the next chat.

## 1. Where the code is

| Branch / Tag | State |
|---|---|
| `master` | Projects 1–3, Context-aware filing Plans 1–4 (projects, plans, sets/subjects, Design shelf separation, Put back together, learned aliases, weekly-note summary), fast-forward merged 2026-11-23. Not pushed. |
| `feature/put-back-together-and-learning` | Merged (ff-only) into `master` on 2026-11-23. Not pushed. |
| `feature/sets-and-photo-subjects` | Merged earlier into `master` (`43fc8bd`). Not pushed. |
| `feature/project-plans` | Tip matching pre-Plan-3 `master`. Not pushed. |
| `checkpoint/pre-stabilization-2026-11-23` | Annotated rollback tag at `084e3d5` (includes parked UI baseline `canonical/D1_Design_System_v1.0.md`, `canonical/D3_Critical_UX_Flows_v1.0.md`, `canonical/suite_reconciliation.md`, and `design-system/`). |

Verification on `master` (fast-forward from `feature/put-back-together-and-learning`):
- `cargo test --workspace`: **496 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), including all **6** tests in `crates/aegis-core/tests/reunite_and_learning.rs` (`0.30s`), all **11** tests in `crates/aegis-core/tests/sets.rs` (`0.62s`), `recognise_is_fast_on_a_big_catalog`, and `migration_0002_applies_on_fresh_and_v1_databases` (`[1..=8]`); **0** folders in `C:\.aegis-quarantine` before and **0** after.
- Live-DB dry run (`examples/filing_dry_run.rs` on a temporary read-only `VACUUM INTO` copy): **256** projects (`87` multi-location), **1** remaining ungathered design set (`MBIZO 1edited` on `D:`), **5** photo subjects, **53** project plans, **1** `put back together` proposal (`ipadcover [PhotoSubject]`: 7 files split earlier by batch `01a203ee` into `Pictures\2026\2026-08` returning to `C:\Users\Mac\Pictures\2026\ipadcover`), zero false positives on already-reunited `SmoothLite`/`Back Cement` or `mugs`, and weekly-note `projects` line `"87 projects have copies in 2–20 places. 1 group has files from earlier batches to put back together."`.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (0 warnings).
- `cargo check -p aegis-desktop --features custom-protocol`: clean.
- `pnpm build` (`tsc --noEmit && vite build`): clean (`dist/` built in `761ms`).

## 2. Done

### Programme (owner-approved order)

| # | Project | State |
|---|---|---|
| 1 | Drives & duplicates | Done, merged |
| 2 | Always-on engine: tray, scheduler, watches, reminders, weekly note | Done, merged |
| 3 | Auto-filing: blueprints (Tidy/Fresh), learn-then-take-over, naming, batches with undo | Done, merged 2026-11-22 |
| 3a | Context-aware filing, Plan 1: project recognition, filing never splits a project, Projects screen | Done, merged 2026-11-23 |
| 3b | Context-aware filing, Plan 2: project plans (move into `C:\Prj`, quarantine/versions/material, one batch per project with undo) | Done, merged 2026-11-23 (with stabilization pass) |
| 3c | Context-aware filing, Plan 3: sets, photo subjects, Design-vs-Software shelf separation, and inline UI reclassification | Done, merged 2026-11-23 |
| 3d | Context-aware filing, Plan 4: put back together (`file_history` reunion), `SameAs` learned aliases, weekly-note summary | Done, merged 2026-11-23 |
| 4 | System care | Next after remaining Context-aware filing modules (§16 step 7) + D1/Embeddings |
| 5 | Visual app (grows alongside the others) | Ongoing |

### Plan 4 in one paragraph

Earlier `filing:*` / `filing-auto:*` moves in `file_history` that scattered files out of a now-recognised project root, `DesignSet`, or `PhotoSubject` are surfaced per collection and volume as atomic **"Put back together"** proposals (`ReunitePlan` in `context::reunite`), tracing prior `MoveHome` directory moves, prioritizing `Project > DesignSet > PhotoSubject`, suffixing collisions `(2)` rather than overwriting, citing the split batch(es), updating `collection_members` / `LearnedSubject`, and supporting one-click undo (`undo_reunite` and Recent batches). Explicit `"Same project…"` answers (`Answer::SameAs`) teach normalized-name aliases (`learned_aliases` in `context::relate`) that match future similarly named folders while chronological `NotSameAs` vetoes/revokes them, and the weekly note (`WeeklyNote.projects` with `#[serde(default)]`) summarizes multi-location projects and pending reunite groups with a direct Review link to Projects.

Loose design and print files across formats and folders (`SmoothLite`, `Back Cement`) are recognised as `DesignSet` design projects in `collection_members` (migration `0008_sets.sql`, kept separate from `collection_locations`), protected in both `Fresh` and `Tidy` Filing (and retired from stale batches), and gathered into `C:\Prj\Design\<Name>` via `StepKind::GatherFiles` with collision suffixing, TOCTOU digest checks, immediate destination protection, and one-click undo. Dedicated Design shelf separation (`kind_home` + `Ctx::is_kind_shelf`) keeps `Design` work at `C:\Prj\Design\<Name>` while software/code projects stay flat at `C:\Prj\<Name>` (leaving nested `<Software>\Design` folders intact), `Answer::SetKind` / `DecisionKind::ProjectKind` allow one-click reclassification from the card, confirmation row, or compact row, and the Projects screen adds a kind filter pill bar with counts, icons, and Design styling. Loose photo subjects (`ipadcover`, `fragrances`) route under `Fresh` to `Photos\<year>\<subject>` while keeping named photo albums (`Pictures\Undated\mugs`) intact, and approved subjects (`DecisionKind::LearnedSubject`) catch single future arrivals. `src-tauri/Cargo.toml` also enables `custom-protocol = ["tauri/custom-protocol"]` so standalone desktop release builds embed `dist/` rather than pointing at `localhost:1420`.

### Plan 2 in one paragraph

Every project gets a plan per drive. The main copy moves whole into `C:\Prj`:
- identical and older copies go to quarantine, once every file is proven in the main copy by BLAKE3 (otherwise the whole copy is kept as a version);
- copies changed on both sides go to `Versions\<date> <origin>`;
- other material goes to `Docs`, `Design` or `Material`.

One approval runs it as one batch, with one undo. Safety rules from the owner's real data:
- nothing inside another project moves;
- Python environments never move;
- other drives get no plans yet;
- only homes inside a projects folder move in;
- copies changed in the last 30 days stay.

It was rehearsed on copies of the real forexTrader and DrivingSchool (117,044 files): undo restored every file exactly.

### Documents

- Specs: `docs/superpowers/specs/`. The context-aware filing design is `2026-11-22-context-aware-filing-design.md`.
- Plans: `docs/superpowers/plans/`. Plan 2 has "as built" notes and rulings per task.
- North star and binding rules for every future design: `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`.

## 3. Waiting on the owner & recent decisions

**Decided 2026-11-23:**
- **Owner Decision D1 approved:** SQLCipher whole-store encryption + `sqlite-vec` inside the encrypted SQLite store (keeping vectors encrypted at rest within the 350 MB K5 budget) + DPAPI/TPM-sealed engine key (`R2`, Windows Hello only for sensitive views) + mandatory offline 24-word/hex recovery key; lands before embeddings and hybrid search. Owner also confirmed keeping all north-star exclusions and reshapings (`R1–R11`, explicitly including `R2` key sealing and `R6` AI Folderless Mode).
- **Orphaned quarantine folders removed:** Deleted all `57` pre-existing orphaned synthetic test folders from `C:\.aegis-quarantine` after confirming `quarantine_entries = 0` in the live database (`0` remaining).

**Still waiting on the owner:**
1. Choose a new app name (another "eagis" / Aegis app already exists).
2. Next active roadmap sequence (owner-approved 2026-11-23): Context-aware filing remaining modules (§16 step 7: Software packages, Owner's folders, Old copies, Resources, Series) → **1. D1 Encryption + Embeddings & Hybrid Search** → **Project 4: System care**.

## 4. What remains

### Context-aware filing (spec §16 build order)

| Step | What | State |
|---|---|---|
| 1–2 | Protection, Projects list | Done (Plan 1) |
| 3 | Project plans | Done (Plan 2) |
| 4 | Sets, photo subjects, Design-vs-Software shelf separation, and UI kind filtering | Done, merged 2026-11-23 (Plan 3) |
| 5–6 | Put back together (`file_history` reunion), `SameAs` learned name aliases, and weekly-note projects summary line | Done, merged 2026-11-23 (Plan 4) |
| 7 | Other modules, each with a short design: Software packages, Owner's folders, Old copies (drive D: plans belong here), Resources, Series | Next up |

### Programme

- **Project 4, System care:**
  - temp files, caches, installers;
  - unused apps (uninstall through Windows' own uninstaller only; **no registry cleaning**);
  - startup items;
  - SMART drive health;
  - disk-space trends (daily space history is already recorded).
- **Owner checks that need the installed build:**
  - `pnpm tauri build` and install;
  - notifications under "Aegis Guardian" open the right screen;
  - Ctrl+Shift+Space opens Search;
  - starts in the tray after a reboot;
  - at most 350 MB in the background.
- **Filing:**
  - dry run with drive D: plugged in (deferred item 14);
  - try Filing on the PC with the Fresh structure.
- **Phase 1 numbers with D: re-plugged:**
  - catalog time and fingerprint time;
  - file count and duplicate groups;
  - refresh mode and time.

### Vision (north-star order, each with its own short design)

1. Embeddings + hybrid search (FTS5 + vectors). Needs D1 first.
2. Local RAG over it (models on demand only).
3. Dead files and workspace health score (largely in System care).
4. Forgotten asset recovery.
5. Semantic version tracking (builds on the `Versions` folders).
6. Knowledge extraction.
7. Research librarian and cross-project intelligence (work from files on disk, not the screen).
8. Folderless mode: plain-language requests become visible filing rules.
9. Self-healing workspace.
10. Chief Digital Officer agent and digital estate (no destructive dead-man's switch).

## 5. Known issues and follow-ups

**Noticed 2026-11-23 (and stabilization pass):**
- ~~The owner's running `aegis-desktop` used about 2 cores continuously and about 1 GB of memory (budget: 350 MB).~~ **Fixed (`b9899b2`, `6a4afde`):** capped SQLite `cache_size` to `-16384` (16 MB/conn) + `shrink_memory()` after heavy batches, filtered `is_live_excluded_path` and directory `Modified` events in `engine_host`, drained up to 2,000 queued FS events per wakeup, and removed per-file `canonicalize` syscalls during expansion while handling Windows case-only folder renames. *(Owner live runtime check on installed build remains under §4.)*
- ~~Older test files (not the plan tests) still write into the real `C:\.aegis-quarantine`.~~ **Fixed (`6a4afde`):** fixed `ensure_volume_for_path` so `ON CONFLICT DO UPDATE` preserves `quarantine_dir`, added `guard_test_quarantine()` refusing non-temp quarantine for in-memory test stores, and isolated `downloads` and `project_plans` tests (`57` before / `57` after full workspace suite).
- ~~Two timing tests are sensitive to machine load.~~ Both `recognise_is_fast_on_a_big_catalog` and `migration_speed_and_fts_sync` now pass in full workspace runs after the background churn fix.
- **Fixed (`c63e967`):** six Plan 2 bugs — blocked occupant guard, `MoveAside` collision suffixing, K1 `expected_digest` check on `context_run_plan`, `"location"` inside `propose` evidence, honest `rollback_batch` conflict reporting, `plan::undo` `move_home` ordering, and `volume_id`-scoped `rebase_children`/folder quarantine/restore with `outbox_events` and `cache_dirs`/`atomic_units` cleanup.
- Project plans' "files moved" counts catalogued files only (`node_modules` and `.venv` contents aren't catalogued).
- Copies of a project on a drive without its home have no relationship to each other; they wait for the Old copies module.

**Filing follow-ups:**
- Search filters only apply to a typed query, and changing a filter needs another press of Search.
- The filing watch only notices arrivals on scopes that have had a batch approved.
- Code exercise folders inside course folders count as small projects, so those courses are not moved to `Videos\Courses`.

**Deferred from Project 2:**
- The pending-navigation slot can land on an old screen.
- The hard-coded shortcut label.
- Saving an invalid shortcut unregisters the working one.
- The `mark_interrupted_jobs` race at startup.
- The installer-name first-word match.
- Rename events counted twice by the tripwire.
- Copy fixes: the pause day, the janitor title, where only-copy files are.
- Halt cards "(0 of N)".
- A busy error aborts a tick.
- The 24 h grace clock resets on restart.
- Action Center click after the popup has gone.

**Deferred from Project 1:**
- **Resolved (`6a4afde`):** `.aegis-partial` on-disk BLAKE3 re-hash before rename, `path_hash` `volume_id` scoping (`retire_copy`, `resolve_policy_target_meta`, `copy_survivor_for`), duplicate keeper radio `keeper_index` wiring end-to-end, and extended `rebase_volume_paths` (`collection_locations`, `file_history`, `scan_jobs`, `root_samples`, `settings` roots, pending `actions`).
- **Still open:**
  - A `backup_jobs` row stays `running` after an error.
  - The first top-up isn't shown for approval.
  - There are two verified-copy implementations.
  - A large USN replay runs in one transaction.

## 6. Owner decisions on record (keep)

- Learn-then-take-over autonomy. Deletes always ask and go to the 30-day quarantine.
- One Backup drive (mirror, 90-day window). The Backup drive is never read.
- Reminders, not interruptions. The "I didn't know I needed this" factor.
- All projects, software or not, live in `C:\Prj`. A git repo may move whole when its plan is approved.
- Projects are one module of a whole-PC context layer, not the whole system.
- Default licence AGPL-3.0.
- Never commit the owner's `canonical/` documents.
