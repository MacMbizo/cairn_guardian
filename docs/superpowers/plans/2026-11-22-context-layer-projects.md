# Context layer and Projects module — Plan 1 (protection and recognition)

> **For agentic workers:** Use superpowers:executing-plans (inline) or subagent-driven-development to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Filing never splits a project again, and Aegis shows every project on the PC with all its locations and how they relate. This is build-order steps 1 and 2 of the spec. Project plans that move things (step 3), sets (4), putting things back together (5), learning (6) and the other modules (7) get their own plans afterwards.

**Architecture:** A new `aegis_core::context` module is the shared layer:
- `profile`: per-folder statistics from the catalog
- `projects`: the Projects recogniser (the first module)
- `relate`: identity and relationship engine, shared by every future module
- `store`: persistence of collections, locations and the owner's answers
- `watch`: an engine watch that keeps recognition current

`filing::blueprint::find_groups` asks the context layer first. Nothing in this plan moves files.

**Tech Stack:** Rust, SQLite (migration 0006), Tauri 2 IPC, React/TS with ts-rs bindings. No new crates.

**Spec:** `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md` (§1A, §6, §7, §9, §12, §13, §17).

## Global Constraints

- **Read-only on disk.** Recognition reads only the catalog, plus README and manifest text the reader already stored. It never writes to the owner's folders.
- **Thresholds** (to be tuned in Task 10, then frozen in tests):
  - A folder is a project at confidence ≥ 0.60; filing protects it from there.
  - 0.60–0.79 shows "Is this a project?"; ≥ 0.80 is "sure".
  - Relationships use the table in spec §6: identical ≥ 98% both ways; subset ≥ 98% one way; diverged ≥ 40% of the smaller shared, with unique files on both sides; different material < 10% shared with the same identity.
- **Build output** never counts toward comparisons or signals, but still belongs to its folder: `node_modules`, `.git`, `target`, `dist`, `build`, `__pycache__`, `.venv`, `venv`, `.next`, `.turbo`, `*.pyc`.
- **Never:**
  - treat a known folder (Documents, Downloads, Pictures, Videos, Music, Desktop), a root, or a fresh-structure area as a project;
  - include the Backup drive.
- **Performance:**
  - A full recognition pass over 400,000 catalog rows finishes in under 60 s on the owner's PC, using SQL aggregates rather than per-row Rust loops over the whole catalog.
  - It runs at below-normal priority on the scheduler thread.
- **Shared rules:**
  - Owner-facing copy is plain English, with no internal names.
  - Write Rust and TS with the Write/Edit tools, not shell heredocs (they mangle backslashes).
  - Commit only bindings whose content changed (`git diff -w --ignore-cr-at-eol`).
  - Tests use temporary folders and in-memory stores.
  - The owner's `canonical/Aegis Guardian Full PRD.md` is never committed.

## Review Focus

1. **A deep marker (`AIPrompt\template\prompt-pigeon\.git`).** The root must be `AIPrompt`, not the deep folder, and not `C:\Prj`. Test: `deep_marker_climbs_to_project_root_not_container` (Task 3).
2. **A container of projects (`Dev Set`, `C:\Prj\DevSet`) must never become one big project.** Test: `containers_are_not_projects` (Task 3).
3. **The 14 rejected moves from the owner's real batches are no longer proposed**, in both Tidy and Fresh. Test: `rejected_project_moves_are_not_proposed` (Task 4).
4. **Two copies where one is mostly build output** (`C:\Prj\paperPilot` has 14,648 files, mostly `node_modules`). Build output must not drown the comparison. Test: `build_output_is_ignored_in_relationships` (Task 5).
5. **The owner says "not the same project"; the next recognition pass must not join them again**, even after a rename or a drive-letter change. Test: `not_same_survives_rename_and_new_letter` (Task 7).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/aegis-core/migrations/0006_context.sql` | collections, locations, decisions |
| `crates/aegis-core/src/context/mod.rs` | types: `Collection`, `Location`, `Relationship`, `CollectionKind`; `recognise()` entry point |
| `crates/aegis-core/src/context/profile.rs` | per-folder statistics (`FolderProfile`) |
| `crates/aegis-core/src/context/projects.rs` | Projects recogniser: signals, containers, roots |
| `crates/aegis-core/src/context/relate.rs` | identity (names, manifests, content) and pair relationships |
| `crates/aegis-core/src/context/store.rs` | save/load collections and decisions |
| `crates/aegis-core/src/context/watch.rs` | engine watch (`ContextWatch`) |
| `crates/aegis-core/tests/context.rs` + `tests/fixtures_context/mod.rs` | tests and folder builders mirroring Dev Set / `C:\Prj` |
| `crates/aegis-core/src/filing/blueprint.rs` | `find_groups` consults the context layer |
| `crates/aegis-core/src/filing/batches.rs` | re-check waiting batches (superseded) |
| `crates/aegis-core/examples/filing_dry_run.rs` | prints recognised projects and relationships |
| `src-tauri/src/commands/context.rs` | IPC |
| `src/screens/Projects.tsx`, `src/screens/fixtures/projects.ts` | Projects screen (read-only cards + answers) |

---

### Task 1: Schema and types

**Files:** Create `migrations/0006_context.sql` and `src/context/{mod.rs,store.rs}`, plus `tests/context.rs`. Modify `db/mod.rs` (migration 6), `lib.rs`, and `tests/drives.rs` (versions list).

**Interfaces (Produces):**
- `#[derive(TS, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)] #[serde(rename_all="snake_case")] pub enum CollectionKind { Project }`. Further modules add variants later.
- `pub enum ProjectKind { Software, Design, Writing, Research, Other }` (TS).
- `pub enum Relationship { Home, Identical, OlderSubset, Diverged, DifferentMaterial, Version, Unrelated, Unknown }` (TS).
- `pub struct Location { id: String, collection_id: String, volume_id: String, root_path: String, relationship: Relationship, files: i64, bytes: i64, newest_mtime: i64, shared_files: i64, compared_by: String /* "content" | "name and size" */, is_repo: bool, online: bool }` (TS).
- `pub struct Collection { id: String, kind: CollectionKind, name: String, project_kind: Option<ProjectKind>, confidence: f64, state: String /* candidate|confirmed|left_as_is|not_a_project */, evidence: Vec<String>, locations: Vec<Location>, sub_projects: Vec<String> }` (TS).
- `store::save_collections(conn, &[Collection]) -> Result<()>` replaces previously computed rows. It never deletes decisions.
- `store::list_collections(conn, kind) -> Result<Vec<Collection>>`.
- `store::record_decision(conn, d: &Decision) -> Result<()>` and `store::decisions(conn) -> Result<Vec<Decision>>`, where `Decision { id, kind: DecisionKind, fingerprint_a: String, fingerprint_b: Option<String>, value: String, at: i64 }` and `DecisionKind { IsProject, SameAs, NotSameAs, LeaveAsIs, Name }`.

**Schema:**
- `collections(id PK, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at)`
- `collection_locations(id PK, collection_id REFERENCES collections ON DELETE CASCADE, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint, UNIQUE(volume_id, root_path))`
- `context_decisions(id PK, kind, fingerprint_a, fingerprint_b, value, at)`
- Index on `collection_locations(fingerprint)`.
- The legacy `projects` table stays untouched. Ruling to record: a new table avoids its NOT NULL `domain` semantics; `atomic_units.project_id` is left for Plan 2.

**Tests:**
- `collections_round_trip`
- `saving_again_replaces_computed_rows_but_keeps_decisions`

**Steps:**
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them; expect a failure to compile.
- [ ] Step 3: Implement.
- [ ] Step 4: Run `cargo test -p aegis-core --test context --test drives` and expect PASS.
- [ ] Step 5: Commit `feat(context): schema and types`.

### Task 2: Folder profiles

**Files:** Create `src/context/profile.rs` and `tests/fixtures_context/mod.rs`.

**Interfaces (Produces):**
- `pub struct FolderProfile { path: String, volume_id: String, depth: usize, files: i64 /* excl. build output */, bytes: i64, newest_mtime: i64, by_kind: HashMap<Kind, i64>, source_files: i64, doc_signals: Vec<String> /* matched project-document names */, asset_signals: Vec<String>, manifests: Vec<(String, String)> /* (file, declared name) */, has_readme: bool, has_git: bool, atomic_unit: bool, child_dirs: Vec<String>, loose_files: i64 }`.
- `pub fn profiles(store: &SqliteStore, roots: &[String]) -> Result<Vec<FolderProfile>>`. It covers every folder under the roots and rolls up the subtree for `files`, `bytes`, `newest_mtime` and `by_kind`. It runs one SQL pass grouped by parent directory, then a Rust roll-up over folders (not files).
- Signal word lists are constants, taken from spec §7.1:
  - project documents: `PRD`, `spec`, `requirements`, `TASKS`, `roadmap`, `architecture`, `persona`, `prompt`, `rules`, `README`, `CHANGELOG`, `set_files`, `brief`, `design doc`;
  - assets: `logo`, `favicon`, `icon`, `mockup`, `wireframe`, `banner`, `sleeve`, `CMYK`, `dpi`;
  - asset extensions: `.fig`, `.psd`, `.ai`, `.svg`.

**Fixture builders** (`fixtures_context`):
- `dev_set(root)` recreates the shape of `Projectsv2\Dev Set`:
  - `forexTrader` with `images\favicon_io\*.png`, `set_files_v2(backup)\*.png` and `engine\pyproject.toml`;
  - `paperPilot` with `images\logo_v3.jpg` and `set_files\TASKS.md`;
  - `discipline_app\set_files\PRD_additions.md` plus `.tsx` files;
  - `chatbot` with md, docx and png;
  - `AIPrompt\template\prompt-pigeon\.git`;
  - `Nutrition` with `*.md`.
- `prj(root)` recreates `C:\Prj`:
  - `forexTrader` as a subset of the Dev Set copy;
  - `DrivingSchool` diverged;
  - `paperPilot` with `package.json` and 300 `node_modules` files;
  - `DevSet\mccShepherd\all docs\archive\PRD_additions.md`.
- `ordinary(root)` holds non-projects: `Downloads\misc` with loose installers and pdfs, `Pictures\2026\2026-10` with photos, and `Documents\CVs` with docx CVs.

**Tests:**
- `profile_counts_exclude_build_output`
- `profile_finds_doc_and_asset_signals`
- `profile_rolls_up_subtrees`

**Steps:** same five steps; commit `feat(context): folder profiles`.

### Task 3: Projects recogniser

**Files:** Create `src/context/projects.rs`.

**Interfaces (Produces):**
- `pub struct ProjectCandidate { root: String, volume_id: String, confidence: f64, evidence: Vec<String>, project_kind: ProjectKind, sub_projects: Vec<String>, is_repo: bool }`.
- `pub fn find_projects(profiles: &[FolderProfile], known_roots: &[String]) -> Vec<ProjectCandidate>`. It implements spec §7.1 (signals and confidence) and §7.2 (climb to the root, stop below a container, nest sub-projects).
- `pub fn is_container(p: &FolderProfile, child_scores: &[f64]) -> bool`: true with ≥ 2 project-like children and few loose files of its own, or when the folder is a root.

**Tests:**
- `deep_marker_climbs_to_project_root_not_container`: the root is `AIPrompt`, with `prompt-pigeon` as a sub-project.
- `markerless_dev_set_folders_are_projects`: `chatbot`, `discipline_app`, `Nutrition` and `paperPilot` (in Dev Set) are each ≥ 0.60, with readable evidence.
- `containers_are_not_projects`: `Dev Set`, `Projectsv2`, `C:\Prj` and `C:\Prj\DevSet` are never projects; `mccShepherd` is one.
- `ordinary_folders_are_not_projects`: `Downloads\misc` and `Pictures\2026\2026-10` are not projects. `Documents\CVs` scores below 0.60; it belongs to the later "Owner's folders" module.
- `kind_is_inferred`: forexTrader is `Software`; an all-artwork folder is `Design`.

**Steps:** same five steps; commit `feat(context): recognise projects`.

### Task 4: Protection in filing

**Files:** Modify `filing/blueprint.rs` (`find_groups`) and `filing/batches.rs`. Create `context::protected_roots(store, roots) -> Result<Vec<(String, String /* project name */)>>`, which reads saved collections and falls back to computing them when none are saved.

**Behaviour (spec §9):**
- Every file under a protected root gets a `Group { code: true, to: None, reason: "Part of project <name>: it stays together" }`, in Tidy and Fresh alike.
- Filing rules never apply under a protected root.
- `next_batch` and the open-batch path re-check waiting batches. Items now under a protected root are set to `superseded`.

**Tests:**
- `rejected_project_moves_are_not_proposed`: build both fixtures, plan Tidy and Fresh, and assert that none of these 14 `from` paths gets a `to`:
  - the forexTrader favicons and logo;
  - `fx-favicon-32x32.png` in `F1` and in `set_files_v2(backup)`;
  - `paperPilot\images\logo_v3.jpg`;
  - `mccShepherd\all docs\others\G5_…md`;
  - `mccShepherd\all docs\archive\PRD_additions.md`;
  - `discipline_app\set_files\PRD_additions.md`;
  - `paperPilot\set_files\TASKS.md`;
  - `C:\Prj\forexTrader\images\FX Sentinel - logo(F)_v2.png`;
  - and the rest listed in the spec.
- `waiting_batch_items_inside_projects_are_superseded`
- `loose_files_still_filed`: a loose invoice in Downloads is still proposed.

**Steps:** same five steps; commit `fix(filing): never split a project`.

> After Task 4 the owner can safely resume approving batches. Say so in the progress note.

### Task 5: Identity and relationships

**Files:** Create `src/context/relate.rs`.

**Interfaces (Produces):**
- `pub fn normalized_name(name: &str) -> String`: spec §7.3 normalization.
- `pub fn content_summary(store, volume_id, root) -> Result<ContentSummary>`. `ContentSummary` holds a map of non-trivial, non-build files: relative path → (size, mtime, blake3?).
- `pub fn same_identity(a: &ProjectCandidate, b: &ProjectCandidate, sa: &ContentSummary, sb: &ContentSummary, decisions: &[Decision]) -> Option<String /* evidence */>`
- `pub fn relate(home: &ContentSummary, other: &ContentSummary) -> (Relationship, i64 /* shared */, String /* compared_by */)`. Matching uses the hash when both sides have one, and relative path + size otherwise.
- `pub fn choose_home(locations) -> usize`: spec §8.1. A git repo wins; then newest activity; then most files.

**Tests**, one per relationship, with numbers shaped like spec §2:
- `identical_copies`
- `older_subset_like_forextrader`
- `diverged_like_drivingschool`
- `different_material_like_paperpilot`
- `name_only_match_is_possible_not_assumed`
- `build_output_is_ignored_in_relationships`
- `version_series_inv_inv2_inv3`

**Steps:** same five steps; commit `feat(context): identity and relationships`.

### Task 6: Recognition pass and engine watch

**Files:** `context/mod.rs` (`recognise`) and `context/watch.rs`. Register the watch in `src-tauri/src/engine_host.rs` next to `FilingWatch` (line ~368).

**Interfaces (Produces):**
- `pub fn recognise(store: &mut SqliteStore) -> Result<RecogniseReport { projects: usize, multi_location: usize, took_ms: u64 }>`. It covers all PC roots and every catalogued non-Backup volume (offline volumes from the catalog, marked `online: false`), groups candidates into collections by identity, sets relationships against the chosen home, applies decisions (Task 7), and saves.
- `ContextWatch: Watch`. It runs `recognise` when the catalog changed since the last run (a `watch_state` stamp), at most once per 10 minutes. It produces no findings in this plan.

**Tests:**
- `recognise_groups_dev_set_and_prj_copies`: forexTrader has 2 locations, with the newer superset as home and the `C:\Prj` copy as `OlderSubset`.
- `offline_drive_locations_are_listed`
- `backup_drive_is_ignored`
- `recognise_is_fast_on_a_big_catalog`: 200,000 synthetic rows inserted directly; must finish in under 15 s in debug.

**Steps:** same five steps; commit `feat(context): recognition pass and watch`.

### Task 7: The owner's answers

**Files:** `context/store.rs`, `context/mod.rs`.

**Behaviour (spec §12):**
- A location fingerprint is `(normalized name, manifest names, top-20 content hashes or path+size keys)`, so it is stable across renames and drive letters.
- How each answer applies:
  - `IsProject = no` drops a candidate;
  - `IsProject = yes` raises it to confirmed;
  - `SameAs` / `NotSameAs` join or split identities;
  - `LeaveAsIs` keeps protection but marks the card settled;
  - `Name` renames the collection.
- `pub fn answer(store, collection_id, location_id: Option<&str>, answer: Answer) -> Result<()>`.

**Tests:**
- `is_project_no_drops_it_and_protection`: the folder's files become fileable again.
- `not_same_survives_rename_and_new_letter`
- `same_as_joins_name_mismatch`: `NutriP` joined with `Nutrition`, with the alias remembered.
- `leave_as_is_keeps_protection`

**Steps:** same five steps; commit `feat(context): owner's answers`.

### Task 8: Dry-run tool

**Files:** `examples/filing_dry_run.rs`.

**Behaviour:**
- After classification, the tool runs `recognise` on the copy and prints:
  - per container, its projects with confidence, kind and first evidence;
  - every multi-location project with each location's relationship and shared-file count;
  - the number of files now protected.
- It keeps the live-database refusal.

**Steps:**
- [ ] Step 1: Implement.
- [ ] Step 2: Run on a copy of the owner's database.
- [ ] Step 3: Check that the output names the projects in spec §2.
- [ ] Step 4: Commit `feat(context): dry run prints projects`.

### Task 9: Projects screen (read-only)

**Files:**
- Create `src-tauri/src/commands/context.rs`, `src/screens/Projects.tsx` and `src/screens/fixtures/projects.ts`.
- Modify `src/screens/Filing.tsx` (a "Projects" button and view), `src/bindings/client.ts` and `src-tauri/src/lib.rs`.

**First:** show the owner a mockup of the card (spec §8.4, without the plan lines, since nothing moves yet). Proceed with their pick.

**IPC:**
- `context_projects() -> Vec<Collection>`
- `context_answer(collection_id, location_id?, answer)`
- `context_recognise_now() -> RecogniseReport`. It is single-flight, and the button shows a running state at once.

**UI:**
- A list of projects, with multi-location ones first.
- Each card shows:
  - name, kind, a "sure" or "Is this a project?" label, and evidence;
  - each location with a plain-words relationship ("older copy: everything here is also in the main copy", "changed on both sides: 13 files differ", "different material: docs here, code there", "unplugged").
- Buttons: "Not a project", "These aren't the same project" (per location), "Rename", "Leave as is".
- There is no move button in this plan. Instead the card notes: "Plans to bring these together come next."

**Steps:**
- [ ] Step 1: Show the mockup.
- [ ] Step 2: Implement.
- [ ] Step 3: Run `pnpm build` and `cargo check -p aegis-desktop`.
- [ ] Step 4: Verify in the browser preview with fixtures (Playwright), then delete `.playwright-mcp`.
- [ ] Step 5: Commit `feat(ui): projects screen`.

### Task 10: Live dry run, tuning and handoff

**Steps:**
- [ ] Step 1: Copy the owner's database with the SQLite backup API and run the dry-run tool.
- [ ] Step 2: Check against reality. Every Dev Set and `C:\Prj` folder in spec §2 must be recognised correctly, with no ordinary folder recognised as a project. Count both kinds of error.
- [ ] Step 3: Tune the thresholds. Each tuning change is a test first (RED), then the fix (GREEN). Freeze the final numbers in tests.
- [ ] Step 4: Run `cargo test --workspace`, clippy and `pnpm build`.
- [ ] Step 5: Update `HANDOFF.md`. Say that filing is safe to resume, give the dry-run numbers, and name Plan 2 (project plans, spec §8) as next.
- [ ] Step 6: Commit.
