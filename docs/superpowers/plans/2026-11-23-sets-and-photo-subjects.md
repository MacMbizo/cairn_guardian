# Context layer and Projects module: Plan 3 (sets and photo subjects)

**Status:** Built and verified on `feature/sets-and-photo-subjects` (2026-11-23). Not merged to `master` yet; nothing pushed.

**Goal:** Recognise cohesive loose-file groups outside folder projects so that:
- loose design/print artwork across formats and folders (`SmoothLite`, `Back Cement`) becomes a `DesignSet` design project card, stays protected from file-by-file Filing (`Fresh` and `Tidy`), and gathers into `<projects home>\<Name>` as one reversible plan batch;
- loose photos sharing a subject (`ipadcover`, `fragrances`) route under `Fresh` to `Photos\<year>\<subject>` instead of `<year-month>`, while named photo albums (`Pictures\Undated\mugs`) stay intact;
- approved subjects (`LearnedSubject`) catch single future arrivals in `Downloads`.

**Tech Stack:** Rust, SQLite (migration `0008_sets.sql`), Tauri 2 IPC, React/TypeScript with `ts-rs` bindings. No new crates.

**Spec:** `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md` (§10, §14, §16 step 4, §17).

## North-star rules

This plan touches these binding rules (`docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` §3):
- **K1 (propose, approve, undo):** Every `DesignSet` gather (`StepKind::GatherFiles`) and every `PhotoSubject` move runs through ordinary approved `action_batches` + `file_history` with a deterministic TOCTOU `plan_digest_with_members` check and one-click undo (`plan::undo` / `filing::batches::after_rollback`). Nothing moves outside `execute_batch`.
- **K2 (deletes ask, 30-day quarantine):** Sets only gather or file into destinations; nothing is quarantined or deleted (empty destination cleanup after undo only removes a folder newly emptied by rollback strictly inside a configured root).
- **K3 (local only):** Uses deterministic filename/folder/catalog heuristics only; no network calls, no cloud dependencies, and no external models.
- **K5 (background budget):** Reads zero file payloads on disk; uses a single combined catalog SQL query and an $O(1)$ ancestor-prefix `project_roots` lookup inside the existing `ContextWatch` pass (`recognise_is_fast_on_a_big_catalog` passes within the `< 15s` budget), adding no resident thread or watch.
- **D1 compatibility:** Stores set metadata exclusively inside the single SQLite database (`collections`, `collection_members`, `context_decisions`), directly compatible with the owner-approved SQLCipher + `sqlite-vec` unified store migration.

## Architecture & File Structure

| File | Responsibility |
|---|---|
| `crates/aegis-core/migrations/0008_sets.sql` | Migration 8: `collections.fingerprint` and `collection_members` table (kept strictly separate from `collection_locations` so planner nesting checks never collide) |
| `crates/aegis-core/src/db/mod.rs` | Registers migration `0008_sets` |
| `crates/aegis-core/src/drives/registry.rs` | Rebases `collection_members.path` on drive re-lettering |
| `crates/aegis-core/src/context/profile.rs` | Expands `ASSET_EXTS` to 9 design formats |
| `crates/aegis-core/src/context/mod.rs` | `pub mod sets;`, `CollectionKind::{DesignSet, PhotoSubject}`, `Relationship::SetFiles`, `DecisionKind::LearnedSubject`, `SetMember`, `protected_files`, `photo_subjects` |
| `crates/aegis-core/src/context/store.rs` | `save_collections_with_members`, `list_collections_in`, `synthesize_set_locations`, `collection_fingerprint`, `list_members`, live-path `list_members_with_collection` (`MemberWithCollection`) |
| `crates/aegis-core/src/context/sets.rs` | `find_sets`, `find_sets_for_roots` (`FoundSets`), multi-format artwork stem matching, false-positive filters, `DesignSet`/`PhotoSubject` classification, `LearnedSubject` + existing design project matching |
| `crates/aegis-core/src/context/recognise.rs` | `RecogniseReport::{design_sets, photo_subjects}`, promotes learned design folders, wires `find_sets` and `save_collections_with_members` |
| `crates/aegis-core/src/context/answers.rs` | `answer_set` handling `IsProject` (+ `LearnedSubject`), `LeaveAsIs`, `Rename`, `KeepHere`, `NotSameAs` |
| `crates/aegis-core/src/db/actions.rs` | Writes `file_history` for executed exact-destination `TargetKind::File` moves (`record_executed_in_tx`) |
| `crates/aegis-core/src/context/plan.rs` | `StepKind::GatherFiles`, `plan_digest_with_members`, `plan_design_set`, collision-safe `propose`, immediate destination protection in `follow_moves`, clean restore in `unfollow_moves` + `after_undo` |
| `crates/aegis-core/src/filing/blueprint.rs` | Cross-root hoisted `protected_files` (`Fresh` & `Tidy`) + `photo_subjects` (`Fresh` routing to `Photos\<year>\<subject>` with unclassified `.png` `mtime` fallback); exports `pub(crate) generic_photo_folder` |
| `crates/aegis-core/src/filing/batches.rs` | Syncs `collection_members.path` in `after_execute`/`after_rollback`, records `"photo_subject"` & `LearnedSubject`, supersedes waiting `DesignSet` moves in `recheck_open_batch` |
| `crates/aegis-core/examples/filing_dry_run.rs` | Prints `Design sets`, `Photo subjects`, and `DesignSet` plans |
| `src-tauri/src/commands/context.rs` | Surfaces `Project` + `DesignSet` in `context_projects`, `context_plans`, `context_run_plan` |
| `src/screens/Projects.tsx`, `src/screens/fixtures/projects.ts` | UI support for `set_files`, `gather_files`, inline Move/Undo, hiding `Change main copy` on `design_set`, `SmoothLite` fixture + 6 updated `ts-rs` bindings |
| `crates/aegis-core/tests/sets.rs`, `tests/drives.rs` | 10 integration tests covering recognition, false positives, protection, routing/no-loop, planning/collisions/undo, TOCTOU/rollback, answers, learning, volume isolation/re-lettering; updates migration list `[1..=8]` |

## Rulings tuned on the owner's 363k-file catalog

Dry runs against a temporary `VACUUM INTO` copy of the owner's catalog initially found `8` design sets and `33` photo subjects, exposing noise from SDK/software trees on `D:`, music companion art, torrent movie posters, duplicate backup filenames, and unrelated long ebook titles. The following deterministic rules eliminated the noise without losing any genuine set on `C:`:

1. **Software, SDK, audio, and movie companion exclusions:** Skip `.9.png`, trailing `<digits>x<digits>` icon/sprite dimensions, `NON_USER_PATH_SEGMENTS` (`plugins`, `resources`, `device-art-resources`, `drawable`, `mipmap`, `wp-content`, `node_modules`, `site-packages`, `activator`, `crack`, `cracks`, `program files`), plus non-root, non-general, non-generic-photo subfolders containing audio tracks, movie companions (`>= 1` video and `<= 3` photos), or software bundle subtrees (`.dll`/`.exe`/`.jar`/etc.).
2. **Boilerplate stopwords & positional filter:** Filter media/assignment boilerplate (`albumart`, `albumartsmall`, `folder`, `cover`, `fore`, `shadow`, `power`, `volume`, `product`, `suite`, `www`, `docfile`, `drawing`, `class`, `ass`, `assignment`, `assignments`) and skip standalone positional words (`back`, `front`, `side`, etc.) after 2-word refinement so `"Back Cement"` stays while standalone `"back"` drops.
3. **Distinct filenames & multi-format cohesion:** Require `>= 3` distinct filenames when unlearned (blocks duplicate copies like `tom.jpg` / `Folder.jpg`), and when a `DesignSet` has no design-source extension and no brand words, require concise multi-format stems (`<= 5` words unless design-source), `>= 3` multi-format filenames (or `>= 2` stems), and `>= 60%` cohesive filenames (eliminates `Black`, `Excel`, `Solar`, `Wireshark Qsn` while preserving `Back Cement`).
4. **Loop & cross-root correctness:** `list_members_with_collection` reads live `files.path` and `after_execute`/`after_rollback` sync `collection_members.path` so filed `PhotoSubject` items never ping-pong back; `blueprint::plan_files` hoists `protected_files`/`photo_subjects` across all scope roots so pre-`recognise` fallback covers `Pictures` + `Downloads`; `follow_moves` registers newly gathered `DesignSet` destinations as protected `project` locations at once.

## Live-DB Dry Run & Verification

- **Dry run (temp `VACUUM INTO` copy of live DB, deleted after inspection):**
  - `254` projects (`87` multi-location), **`3` design sets** (`SmoothLite` 31 files across `Pictures`/`Downloads`/`2026-10` -> `C:\Prj\SmoothLite`; `Back Cement` 8 files -> `C:\Prj\Back Cement`; `MBIZO 1edited` 3 files on `D:`), **`5` photo subjects** (`ipadcover` 12, `Mcgs Splash` 12, `Copilot` 7, `Designer` 4, `fragrances` 5), named album `Pictures\Undated\mugs` preserved intact (`Photo album: 15`), `53` total project/set plans (`4.6s`).
- **Workspace verification gate:**
  - `cargo test --workspace`: **486 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), including all 10 tests in `crates/aegis-core/tests/sets.rs` (`0.51s`), `recognise_is_fast_on_a_big_catalog`, and `migration_0002_applies_on_fresh_and_v1_databases`; `C:\.aegis-quarantine` verified at `0` before and `0` after (the 57 pre-existing orphaned test folders with 0 DB `quarantine_entries` references were removed at the start of Plan 3).
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings).
  - `pnpm build` (`tsc --noEmit && vite build`): clean.
