# Context layer and Projects module: Plan 2 (project plans)

> **For agentic workers:** Implement this plan task by task with superpowers:executing-plans (inline) or subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each recognised project gets one plan on its card that accounts for every copy:
- where the project lives (its home, inside the projects home `C:\Prj`);
- what happens to each other copy, and why.

One approve runs the whole plan as one batch, with one undo. This is build-order step 3 of the spec. Sets (step 4), putting things back together (5), learning and the weekly-note line (6), and the other modules (7) get their own plans.

**Architecture:**
- A new `aegis_core::context::plan` module turns a saved `Collection` into a `ProjectPlan`: an ordered list of folder steps.
- `context::plan::propose` stores a plan as an ordinary `action_batches` row (origin `project:<collection id>`) with folder actions (`target_kind = 'dir'`). It never writes a new kind of batch.
- Execution reuses `execute_batch`, `after_execute` and `rollback_batch`, with three hardenings that project plans need and the existing executor lacks:
  1. **Step order.** Actions carry an explicit `step`, so anything that frees a name runs before the move that needs it.
  2. **Content check before quarantine.** A copy is quarantined only when every one of its files is proven to be in home by BLAKE3.
  3. **Stale-plan check for folders.** A folder that changed since the plan was made stops the plan.
- A project batch is all-or-nothing. If any step fails, the steps already done are rolled back and the card says why in plain words.

**Tech Stack:** Rust, SQLite (migration 0007), Tauri 2 IPC, React/TS with ts-rs bindings. No new crates.

**Spec:** `docs/superpowers/specs/2026-11-22-context-aware-filing-design.md` (§6, §7.4, §8, §9 2nd bullet, §13 unplugged drives, §14, §17 "Moves stay on one volume" and "Folder moves").

## Global Constraints

- **Nothing moves without an approved plan.** Recognition and planning stay read-only on disk. Only `execute_batch` on an approved project batch touches the owner's folders.
- **One volume per move.** Every step's source and destination are on the same volume, using a rename, never copy-then-delete.
  - A plan step that would cross volumes is a bug: `plan` refuses it and the test suite asserts it.
  - Copies on other drives are listed on the card and get their own plan on that drive. A PC copy never causes an external copy to be quarantined, or the reverse.
- **Never lose work** (spec anti-goal 1):
  - A copy goes to quarantine only if every non-build file in it has a byte-identical file (BLAKE3) in home at execute time. Hashes missing from the catalog are computed then.
  - If any file fails the check, the **whole copy** goes to the versions area instead. It is never split, and the owner is told why.
  - Build output (`node_modules`, `target`, `dist`, `build`, `__pycache__`, `.venv`, `venv`, `.next`, `.turbo`, `*.pyc`) is not checked. It can be rebuilt. Record this as a ruling.
- **Repositories** (spec §7.4):
  - A repo is never a quarantine target. Its copy relationship becomes diverged or version, never identical or older-subset quarantine.
  - Nothing is moved into a repo's tree. Material and versions for a repo home go to the sibling `<repo name> - material` folder (plain hyphen; see the rulings).
  - A repo may move whole into the projects home. The card warns when it holds a virtual environment (`pyvenv.cfg`), or when a non-build file changed in the last 30 days.
- **Names in the projects home:**
  - A collision with a different project gets a disambiguator, never ` (2)`: `<name> (<kind> <year of newest file>)`, e.g. `wellness (app 2025)`.
  - A collision with a copy of the same project is resolved by ordering: that copy is quarantined or versioned first.
- **Plans wait** when any location they touch is on an unplugged drive, or when a recognition pass is running.
- **Shared rules** (from Plan 1):
  - Owner-facing copy is plain English, with no internal names.
  - Write Rust and TS with Write/Edit, not heredocs.
  - Commit only bindings whose content changed (`git diff -w --ignore-cr-at-eol`).
  - Tests use temporary folders and in-memory stores.
  - Never commit `canonical/Aegis Guardian Full PRD.md` or `canonical/local-intelligence-system-architecture.md`.
  - Make a checkpoint commit before each task that changes the executor (Tasks 3–4).

## Review Focus

1. **The forexTrader card** (spec §8.4): the `C:\Prj\forexTrader` older copy is quarantined first, then the Dev Set home moves to `C:\Prj\forexTrader`. One undo restores both. Test: `forextrader_plan_frees_the_name_then_moves_home` (Task 4).
2. **A quarantine target with one file that isn't in home** (edited after the plan was made, or never there): the whole copy goes to `Versions`, and nothing is quarantined. Test: `copy_with_unique_file_goes_to_versions_not_quarantine` (Task 3).
3. **A step fails halfway.** For example, a program holds a file open in the home folder, so its rename fails. The quarantine already done is rolled back, the batch ends `rolled_back` with a plain reason, and no ` (2)` folder is left behind. Test: `failed_step_rolls_back_the_whole_plan` (Task 4).
4. **A repo home.** Material from Dev Set goes to `C:\Prj\<repo> - material`, never into the repo. A repo copy is never quarantined. Tests: `repo_home_gets_companion_folder`, `repo_is_never_quarantined` (Task 2).
5. **After execute and after undo,** the catalog, `file_history`, the saved collection and the owner's answers all point at the new (or restored) paths. The next recognition pass gives the same project with the same decisions. Test: `decisions_survive_execute_and_undo` (Task 5).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/aegis-core/migrations/0007_project_plans.sql` | `actions.step`; `collections.plan_state`; home and keep-here decisions need no schema (decision kinds) |
| `crates/aegis-core/src/context/plan.rs` | `ProjectPlan`, `PlanStep`, `plan_for(collection, …)`, `propose`, ordering of cards |
| `crates/aegis-core/src/context/mod.rs` | new `DecisionKind::{Home, KeepHere}`; `Answer::{ChangeHome, KeepHere}` in `answers.rs` |
| `crates/aegis-core/src/db/actions.rs` | order by `step`; folder stale check; content check before folder quarantine; all-or-nothing for `project:` batches |
| `crates/aegis-core/src/filing/batches.rs` | `after_execute`/`after_rollback` for folder quarantine; `recent_batches` includes `project:` batches |
| `crates/aegis-core/src/filing/blueprint.rs` | Fresh leaves loose files of project containers in place (spec §9, 2nd bullet) |
| `crates/aegis-core/tests/project_plans.rs` | tests; reuses `tests/fixtures_context` |
| `crates/aegis-core/examples/filing_dry_run.rs` | prints each project's plan |
| `src-tauri/src/commands/context.rs` | IPC: plan, approve, change home, keep here |
| `src/screens/Projects.tsx`, `src/screens/fixtures/projects.ts` | plan lines and buttons on the card; "Plans" order |

---

### Task 1: Schema, decisions and answers

**Files:**
- Create `migrations/0007_project_plans.sql`.
- Modify `db/mod.rs` (migration 7), `tests/drives.rs` (versions list), `context/mod.rs`, `context/answers.rs` and `context/store.rs`.

**Schema:**
- `ALTER TABLE actions ADD COLUMN step INTEGER NOT NULL DEFAULT 0`.
- `ALTER TABLE collections ADD COLUMN plan_state TEXT`, holding NULL, `waiting`, `proposed:<batch id>`, `done:<batch id>` or `undone:<batch id>`. Recognition must carry it over on save, like decisions.

**Interfaces (Produces):**
- `DecisionKind::Home`: `fingerprint_a` is the collection's chosen home location; `value` is empty.
- `DecisionKind::KeepHere`: `fingerprint_a` is a location the owner wants left where it is.
- `Answer::ChangeHome { location_id }` and `Answer::KeepHere { location_id, keep: bool }`.
- Recognition applies them:
  - a `Home` decision overrides `choose_home` when its fingerprint matches a location;
  - `KeepHere` marks the location so `plan_for` gives it no step.

**Tests:**
- `change_home_survives_recognition`
- `keep_here_survives_recognition`
- `plan_state_survives_save`

**Steps:**
- [ ] Step 1: Write the tests.
- [ ] Step 2: Run them and expect a failure to compile.
- [ ] Step 3: Implement.
- [ ] Step 4: Run `cargo test -p aegis-core --test context --test project_plans --test drives` and expect PASS.
- [ ] Step 5: Commit `feat(context): plan schema, home and keep-here answers`.

### Task 2: Planning (pure, no disk writes)

**Files:** Create `context/plan.rs`.

**Interfaces (Produces):**
- `pub enum StepKind { MoveHome, Quarantine, ToVersions, ToMaterial, MakeFolder }`.
  - `MakeFolder` creates the companion folder, or `Versions`, when it doesn't exist yet.
- `pub struct PlanStep { step: u32, kind: StepKind, location_id: String, from: String, to: String, reason: String /* plain words */, warnings: Vec<String> }` (TS).
- `pub struct ProjectPlan { collection_id, volume_id, home_after: String, steps: Vec<PlanStep>, waits_for: Option<String> /* "New Volume is unplugged" */, summary: String }` (TS).
- `pub fn projects_home(store, volume_id) -> Result<Option<String>>`.
  - On the PC, it is the container holding the most projects (today `C:\Prj`). A setting overrides it.
  - On other drives, it is the Fresh `Projects` folder if it exists, else `None`. With `None`, the home stays where it is, and only same-drive copies get steps.
- `pub fn plan_for(store, c: &Collection) -> Result<Vec<ProjectPlan>>`: one plan per volume the collection lives on.

**Rules (spec §8.1–8.3, constraints above):**

| Copy relationship | Step |
|---|---|
| home outside projects home | `MoveHome` to `<projects home>\<name>` |
| identical / older subset, not a repo | `Quarantine` |
| identical / older subset, repo | `ToVersions` |
| diverged | `ToVersions` → `<home or companion>\Versions\<YYYY-MM-DD> <where it came from>` (date of newest file; "where" = parent folder name, e.g. `Dev Set`) |
| different material | `ToMaterial` → `<home or companion>\Docs`, `Design` or `Material` by its kinds (≥ 70% docs → Docs, ≥ 70% images/design → Design, else Material) |
| version (series) | `ToVersions` with `<YYYY-MM-DD> <original name>` |
| unknown / unrelated / kept here / sub-project | no step |

- Step order:
  1. quarantines and versions of copies that sit at the name `MoveHome` needs;
  2. `MakeFolder`;
  3. `MoveHome`;
  4. the remaining copies.
- Destinations that collide with another project get the disambiguator. Destinations that collide inside one plan are made unique with the existing `unique_in_batch`.
- Paths are computed against the post-`MoveHome` home, so a `ToVersions` target is inside the new home location.

**Tests:**
- `forextrader_plan_shape`: quarantine `C:\Prj\forexTrader` (step 1), move the Dev Set home to `C:\Prj\forexTrader` (step 2).
- `drivingschool_diverged_goes_to_versions`
- `paperpilot_material_goes_to_docs`
- `single_location_outside_home_moves_in`
- `home_already_in_projects_home_has_no_move`
- `repo_home_gets_companion_folder`
- `repo_is_never_quarantined`
- `name_collision_with_other_project_is_disambiguated`
- `no_step_crosses_volumes`
- `unplugged_location_makes_plan_wait`
- `left_as_is_and_not_a_project_get_no_plan`
- `keep_here_location_gets_no_step`

**Steps:** the same five steps; commit `feat(context): project plans`.

### Task 3: Executor hardening

Make a checkpoint commit first. These changes touch `execute_batch`, which every batch uses.

**Files:** Modify `db/actions.rs` and `filing/batches.rs`.

**Behaviour:**
- **Order:** actions run `ORDER BY step ASC, created_at ASC, id ASC`. Existing batches all have `step = 0`, so their order is unchanged.
- **Stale folder:** for `target_kind = 'dir'` actions on a `project:` batch:
  - the action stores the folder's non-build file count and newest mtime at propose time (in `actions.params_json`, or the nearest existing column; builder's call);
  - at execute time the folder must exist and match. Otherwise the plan stops with "`<folder>` changed since this plan was made. Aegis will make a fresh plan."
- **Content check before folder quarantine:**
  - every non-build file under the source must have a BLAKE3 match among the home's files. Compute missing hashes with a public wrapper over `hash_file_blake3`. Cloud placeholders count as "not proven".
  - On failure, the action is rewritten to `ToVersions` (a move to the planned versions path), with the reason "N files here aren't in the main copy, so it was kept as a version".
- **Folder quarantine bookkeeping:** `after_execute` marks every catalogued file under a quarantined folder as quarantined, so it no longer appears in search, duplicates or recognition. `after_rollback` restores those files.
- **Purge:** confirm that expiry purge removes a quarantined folder recursively and updates its catalog rows. Add the code if it doesn't.

**Tests:**
- `actions_run_in_step_order`
- `existing_batches_keep_their_order`
- `copy_with_unique_file_goes_to_versions_not_quarantine`
- `copy_fully_in_home_is_quarantined_and_restorable`
- `changed_folder_stops_the_plan`
- `quarantined_folder_files_leave_search_and_come_back_on_undo`
- `quarantined_folder_purges_recursively`

**Steps:** the same five steps; commit `feat(actions): ordered, checked folder steps`.

### Task 4: All-or-nothing project batches

**Files:** `context/plan.rs` (`propose`, `approve_and_run`) and `db/actions.rs`.

**Interfaces (Produces):**
- `pub fn propose(store, plan: &ProjectPlan) -> Result<String /* batch id */>`.
  - It re-checks that the plan is current (no recognition running, locations unchanged).
  - It sets `plan_state = proposed:<id>`.
  - It reuses an existing proposed batch for the same collection instead of making a second one.
- `pub fn run(store, batch_id) -> Result<PlanOutcome { done: bool, reason: Option<String>, moved_files: i64, freed_bytes: i64 }>`.
  - It runs `approve_batch`, then `execute_batch`. If any action failed or was skipped, it runs `rollback_batch` at once and returns the first failure's plain reason.
  - Then `after_execute` (or `after_rollback`), then it sets `plan_state`.
- Plain reasons for common failures:
  - access denied or sharing violation: "A program is using files in `<folder>`. Close it and try again.";
  - destination exists: "`<folder>` appeared in the way. Aegis will make a fresh plan.";
  - drive gone: "`<drive>` was unplugged."

**Tests:**
- `forextrader_plan_frees_the_name_then_moves_home`: run it, then undo, then check the disk and the catalog.
- `failed_step_rolls_back_the_whole_plan`: hold a file open in home with a Windows share-deny handle (`std::fs::OpenOptions` plus `share_mode(0)`) so the rename fails.
- `no_suffixed_folder_is_ever_left`
- `proposing_twice_reuses_the_batch`
- `project_batches_show_in_recent_batches_and_undo`

**Steps:** the same five steps; commit `feat(context): run project plans as one batch`.

### Task 5: Keeping the context layer true after moves

**Files:** `context/recognise.rs`, `context/store.rs`, `context/plan.rs`.

**Behaviour:**
- After `run` and after an undo, rebase the collection's saved locations to their new paths at once, so the card is right before the next recognition pass. Then mark the catalog changed so `ContextWatch` re-recognises.
- Moved and versioned copies become `Version` or `DifferentMaterial` parts inside home. They are not separate locations any more, and not new projects.
- Fill `atomic_units.project_id` for units inside a project (a spec §14 item left over from Plan 1).

**Tests:**
- `decisions_survive_execute_and_undo`
- `versions_inside_home_are_not_new_projects`
- `atomic_units_get_project_id`

**Steps:** the same five steps; commit `feat(context): locations follow executed plans`.

### Task 6: Loose files in project containers (spec §9, 2nd bullet)

**Files:** `filing/blueprint.rs`.

**Behaviour:** under Fresh, loose files directly inside a project container (`Dev Set`, `Projectsv2`, `C:\Prj`) stay, with the reason "In your projects folder: left for project plans". Tidy is unchanged.

**Tests:**
- `fresh_leaves_container_loose_files`
- `tidy_still_tidies_container_loose_files`

**Steps:** the same five steps; commit `fix(filing): leave project containers to project plans`.

### Task 7: Dry run prints plans

**Files:** `examples/filing_dry_run.rs`.

**Behaviour:** for every project with at least one step, print its plan in the order the cards would be offered (spec §8.5: identical, then subsets, then single moves, then diverged and versions). Then print totals:
- moves;
- quarantines, with the space to be freed;
- versions;
- plans waiting.

The tool keeps the live-database refusal, and it never executes.

**Steps:**
- [ ] Step 1: Implement.
- [ ] Step 2: Run it on a copy of the owner's database, made with the SQLite backup API.
- [ ] Step 3: Check the spec §2 projects by eye: forexTrader, DrivingSchool, AIPrompt, paperPilot, wellness. Fix wrong plans test-first.
- [ ] Step 4: Commit `feat(context): dry run prints project plans`.

### Task 8: Card with plan (UI)

**Files:** `src-tauri/src/commands/context.rs`, `src-tauri/src/lib.rs`, `src/bindings/client.ts`, `src/screens/Projects.tsx`, `src/screens/fixtures/projects.ts`.

**First:** show the owner two or three mockups of the card with its plan lines (spec §8.4) and the approve flow. Build the one they pick.

**IPC:**
- `context_plan(collection_id) -> Vec<ProjectPlan>`
- `context_run_plan(collection_id, volume_id) -> PlanOutcome`. It proposes and runs in one call, is single-flight per collection, and the button shows "Moving…" at once.
- `context_answer` gains `ChangeHome` and `KeepHere`.
- Undo goes through the existing `filing_undo`.

**UI:**
- Under each location, a plan line in plain words: "→ moves whole to C:\Prj\forexTrader (your projects home)", "→ quarantine, 30 days: everything here is in the main copy".
- Warnings shown on the card: repo with recent changes, virtual environment to recreate.
- Buttons:
  - **Approve plan**;
  - **Change home** (pick a location);
  - **Keep this copy here** (per location);
  - the existing **These aren't the same project** and **Leave as is**.
- A **Plans** filter that orders cards as in spec §8.5.
- After a run, the card says "Done: N files moved, X GB freed, [Undo]". A failed run shows its reason and the line "Nothing was changed."
- A waiting plan says why ("New Volume is unplugged").

**Steps:**
- [ ] Step 1: Show the mockups and record the owner's pick in the ledger.
- [ ] Step 2: Implement.
- [ ] Step 3: Run `pnpm build` and `cargo check -p aegis-desktop`.
- [ ] Step 4: Verify in the browser preview with fixtures (Playwright), then delete `.playwright-mcp`.
- [ ] Step 5: Commit `feat(ui): project plans on the card`.

### Task 9: Live rehearsal, verification and handoff

**Steps:**
- [ ] Step 1: Real-data rehearsal on scratch copies.
  - Copy both forexTrader copies and both DrivingSchool copies (read-only source) into a scratch folder laid out like `C:\Prj` and `Dev Set`.
  - Catalog them into a scratch database (`AEGIS_DATA_DIR`).
  - Run each plan, check the disk, then undo and check the disk again.
  - Never execute against the owner's real folders.
- [ ] Step 2: Run `cargo test --workspace`, `cargo clippy --workspace --all-targets` and `pnpm build`.
- [ ] Step 3: Update `HANDOFF.md`:
  - dry-run totals;
  - the rehearsal result;
  - which cards the owner can safely approve first (identical copies);
  - Plan 3 (sets and photo subjects, spec §10) as next.
- [ ] Step 4: Commit.

## Rulings to record in the ledger (owner may overrule)

- Companion folder is `<repo name> - material` (plain hyphen, easy to type), not an em dash.
- A copy that fails the content check is versioned **whole**, never split between quarantine and versions.
- Build output inside a quarantined copy is not content-checked (it can be rebuilt).
- A project batch is all-or-nothing: a failed step rolls back the plan rather than leaving it half done.
- On drives with no `Projects` folder, the home stays in place, and only same-drive copies are planned.
